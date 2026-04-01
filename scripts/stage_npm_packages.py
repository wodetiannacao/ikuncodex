#!/usr/bin/env python3
"""Stage one or more Codex npm packages for release."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path
import sys


REPO_ROOT = Path(__file__).resolve().parent.parent
BUILD_SCRIPT = REPO_ROOT / "codex-cli" / "scripts" / "build_npm_package.py"
INSTALL_NATIVE_DEPS = REPO_ROOT / "codex-cli" / "scripts" / "install_native_deps.py"
WORKFLOW_NAME = ".github/workflows/rust-release.yml"
LOCAL_SDK_BIN_ROOT = REPO_ROOT / "sdk" / "python" / "src" / "codex_app_server" / "bin"

_SPEC = importlib.util.spec_from_file_location("codex_build_npm_package", BUILD_SCRIPT)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError(f"Unable to load module from {BUILD_SCRIPT}")
_BUILD_MODULE = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_BUILD_MODULE)
PACKAGE_NATIVE_COMPONENTS = getattr(_BUILD_MODULE, "PACKAGE_NATIVE_COMPONENTS", {})
WINDOWS_ONLY_COMPONENTS = getattr(_BUILD_MODULE, "WINDOWS_ONLY_COMPONENTS", {})
PLATFORM_PACKAGE_METADATA = getattr(_BUILD_MODULE, "PLATFORM_PACKAGE_METADATA", {})
resolve_package_publish_name = getattr(
    _BUILD_MODULE, "resolve_package_publish_name", lambda package: package
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--release-version",
        required=True,
        help="Version to stage (e.g. 0.1.0 or 0.1.0-alpha.1).",
    )
    parser.add_argument(
        "--package",
        dest="packages",
        action="append",
        required=True,
        help="Package name to stage. May be provided multiple times.",
    )
    parser.add_argument(
        "--workflow-url",
        help="Optional workflow URL to reuse for native artifacts.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="Directory where npm tarballs should be written (default: dist/npm).",
    )
    parser.add_argument(
        "--vendor-src",
        type=Path,
        default=None,
        help=(
            "Optional prebuilt vendor root to reuse directly. When provided, staging skips "
            "the native artifact installation step and copies binaries from this directory."
        ),
    )
    parser.add_argument(
        "--keep-staging-dirs",
        action="store_true",
        help="Retain temporary staging directories instead of deleting them.",
    )
    return parser.parse_args()


def collect_native_components(packages: list[str]) -> set[str]:
    components: set[str] = set()
    for package in packages:
        components.update(PACKAGE_NATIVE_COMPONENTS.get(package, []))
        components.update(WINDOWS_ONLY_COMPONENTS.get(package, []))
    return components


def expand_release_packages(packages: list[str]) -> list[str]:
    """Ensure the main ikuncodex package is always released with every platform child package."""

    expanded = list(packages)
    if "codex" in expanded:
        for platform_package in PLATFORM_PACKAGE_METADATA:
            if platform_package not in expanded:
                expanded.append(platform_package)
    return expanded


def resolve_release_workflow(version: str) -> dict:
    stdout = subprocess.check_output(
        [
            "gh",
            "run",
            "list",
            "--branch",
            f"rust-v{version}",
            "--json",
            "workflowName,url,headSha",
            "--workflow",
            WORKFLOW_NAME,
            "--jq",
            "first(.[])",
        ],
        cwd=REPO_ROOT,
        text=True,
    )
    workflow = json.loads(stdout or "null")
    if not workflow:
        raise RuntimeError(f"Unable to find rust-release workflow for version {version}.")
    return workflow


def resolve_workflow_url(version: str, override: str | None) -> tuple[str, str | None]:
    if override:
        return override, None

    if shutil.which("gh") is None:
        raise RuntimeError("GitHub CLI `gh` is not installed.")

    workflow = resolve_release_workflow(version)
    return workflow["url"], workflow.get("headSha")


def install_native_components(
    workflow_url: str | None,
    components: set[str],
    vendor_root: Path,
) -> None:
    if not components:
        return

    cmd = [str(INSTALL_NATIVE_DEPS)]
    if workflow_url:
        cmd.extend(["--workflow-url", workflow_url])
    if LOCAL_SDK_BIN_ROOT.exists():
        cmd.extend(["--local-sdk-bin-root", str(LOCAL_SDK_BIN_ROOT)])
    for component in sorted(components):
        cmd.extend(["--component", component])
    cmd.append(str(vendor_root))
    run_command(cmd)


def run_command(cmd: list[str]) -> None:
    if cmd and cmd[0].endswith(".py"):
        cmd = [sys.executable, *cmd]
    print("+", " ".join(cmd))
    subprocess.run(cmd, cwd=REPO_ROOT, check=True)


def main() -> int:
    args = parse_args()

    output_dir = args.output_dir or (REPO_ROOT / "dist" / "npm")
    output_dir.mkdir(parents=True, exist_ok=True)

    runner_temp = Path(os.environ.get("RUNNER_TEMP", tempfile.gettempdir()))

    packages = expand_release_packages(list(args.packages))
    native_components = collect_native_components(packages)

    vendor_temp_root: Path | None = None
    vendor_src: Path | None = args.vendor_src.resolve() if args.vendor_src else None
    resolved_head_sha: str | None = None

    final_messsages = []

    try:
        if native_components and vendor_src is None:
            workflow_url: str | None = None
            try:
                workflow_url, resolved_head_sha = resolve_workflow_url(
                    args.release_version, args.workflow_url
                )
            except RuntimeError as exc:
                print(
                    "Falling back to local/native-light staging because workflow resolution "
                    f"was unavailable: {exc}"
                )
            vendor_temp_root = Path(tempfile.mkdtemp(prefix="npm-native-", dir=runner_temp))
            install_native_components(workflow_url, native_components, vendor_temp_root)
            vendor_src = vendor_temp_root / "vendor"

        if resolved_head_sha:
            print(f"should `git checkout {resolved_head_sha}`")

        for package in packages:
            staging_dir = Path(tempfile.mkdtemp(prefix=f"npm-stage-{package}-", dir=runner_temp))
            publish_name = resolve_package_publish_name(package)
            pack_output = output_dir / f"{publish_name}-{args.release_version}.tgz"

            cmd = [
                str(BUILD_SCRIPT),
                "--package",
                package,
                "--release-version",
                args.release_version,
                "--staging-dir",
                str(staging_dir),
                "--pack-output",
                str(pack_output),
            ]

            if vendor_src is not None:
                cmd.extend(["--vendor-src", str(vendor_src)])

            try:
                run_command(cmd)
            finally:
                if not args.keep_staging_dirs:
                    shutil.rmtree(staging_dir, ignore_errors=True)

            final_messsages.append(f"Staged {publish_name} at {pack_output}")
    finally:
        if vendor_temp_root is not None and not args.keep_staging_dirs:
            shutil.rmtree(vendor_temp_root, ignore_errors=True)

    for msg in final_messsages:
        print(msg)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())

#
# 编号（如：1）：修改
# 主要修改内容：为 npm staging 流程增加了无 gh 环境下的本地回退路径，并自动转发本地 SDK 二进制目录。
# 修改目的：让当前机器即使没有 GitHub CLI，也能先产出可发布的 ikuncodex npm 包进行本地验证和后续发布。
#
# 编号（如：2）：修改
# 主要修改内容：让 staging 输出文件名跟随最终 npm 发布名，而不是继续沿用内部包 key。
# 修改目的：避免拆包发布后生成的 tgz 文件名与实际 npm 包名不一致，降低手工发布时的混淆风险。
#
# 编号（如：3）：修改
# 主要修改内容：新增 --vendor-src 参数，允许 staging 直接复用已准备好的 vendor 根目录。
# 修改目的：避免拆包发布时反复依赖 gh 下载旧 workflow 构件，让当前机器可以稳定复用已验证的本地 vendor 产物。
#
# 编号（如：4）：修改
# 主要修改内容：当 staging 主包 codex 时自动补齐全部平台子包，并将最终输出信息统一为真实发布名。
# 修改目的：避免维护者误只发布主包导致安装后缺少 native 子包，同时减少内部包 key 与真实 npm 包名混淆。
#
