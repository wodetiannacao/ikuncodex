#!/usr/bin/env python3
"""Stage and optionally package the ikuncodex npm module."""

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
CODEX_CLI_ROOT = SCRIPT_DIR.parent
REPO_ROOT = CODEX_CLI_ROOT.parent
RESPONSES_API_PROXY_NPM_ROOT = REPO_ROOT / "codex-rs" / "responses-api-proxy" / "npm"
CODEX_SDK_ROOT = REPO_ROOT / "sdk" / "typescript"
CODEX_NPM_NAME = "ikuncodex"
IKUNCODEX_REPOSITORY = {
    "type": "git",
    "url": "git+https://github.com/wodetiannacao/ikuncodex.git",
    "directory": "codex-cli",
}
IKUNCODEX_HOMEPAGE = "https://github.com/wodetiannacao/ikuncodex/tree/main/codex-cli"
IKUNCODEX_BUGS = {"url": "https://github.com/wodetiannacao/ikuncodex/issues"}
PLATFORM_PACKAGE_METADATA: dict[str, dict[str, object]] = {
    "ikuncodex-linux-x64": {
        "target": "x86_64-unknown-linux-musl",
        "os": ["linux", "android"],
        "cpu": ["x64"],
        "description": "Prebuilt ikuncodex native binaries for Linux x64.",
    },
    "ikuncodex-linux-arm64": {
        "target": "aarch64-unknown-linux-musl",
        "os": ["linux", "android"],
        "cpu": ["arm64"],
        "description": "Prebuilt ikuncodex native binaries for Linux ARM64.",
    },
    "ikuncodex-darwin-x64": {
        "target": "x86_64-apple-darwin",
        "os": ["darwin"],
        "cpu": ["x64"],
        "description": "Prebuilt ikuncodex native binaries for macOS Intel.",
    },
    "ikuncodex-darwin-arm64": {
        "target": "aarch64-apple-darwin",
        "os": ["darwin"],
        "cpu": ["arm64"],
        "description": "Prebuilt ikuncodex native binaries for macOS Apple Silicon.",
    },
    "ikuncodex-win32-x64": {
        "target": "x86_64-pc-windows-msvc",
        "os": ["win32"],
        "cpu": ["x64"],
        "description": "Prebuilt ikuncodex native binaries for Windows x64.",
    },
    "ikuncodex-win32-arm64": {
        "target": "aarch64-pc-windows-msvc",
        "os": ["win32"],
        "cpu": ["arm64"],
        "description": "Prebuilt ikuncodex native binaries for Windows ARM64.",
    },
}
TARGET_TO_PLATFORM_PACKAGE = {
    str(metadata["target"]): package for package, metadata in PLATFORM_PACKAGE_METADATA.items()
}
PACKAGE_PUBLISH_NAMES: dict[str, str] = {
    "codex": CODEX_NPM_NAME,
    "codex-responses-api-proxy": "@openai/codex-responses-api-proxy",
    "codex-sdk": "@openai/codex-sdk",
    **{package: package for package in PLATFORM_PACKAGE_METADATA},
}

PACKAGE_NATIVE_COMPONENTS: dict[str, list[str]] = {
    "codex": [],
    "codex-responses-api-proxy": ["codex-responses-api-proxy"],
    "codex-sdk": [],
    **{package: ["codex", "rg"] for package in PLATFORM_PACKAGE_METADATA},
}
WINDOWS_ONLY_COMPONENTS: dict[str, list[str]] = {
    "codex": [],
    "ikuncodex-win32-x64": ["codex-windows-sandbox-setup", "codex-command-runner"],
    "ikuncodex-win32-arm64": ["codex-windows-sandbox-setup", "codex-command-runner"],
}
COMPONENT_DEST_DIR: dict[str, str] = {
    "codex": "codex",
    "codex-responses-api-proxy": "codex-responses-api-proxy",
    "codex-windows-sandbox-setup": "codex",
    "codex-command-runner": "codex",
    "rg": "path",
}
COMPONENT_EXPECTED_FILENAMES: dict[str, str] = {
    "codex": "codex",
    "codex-responses-api-proxy": "codex-responses-api-proxy",
    "codex-windows-sandbox-setup": "codex-windows-sandbox-setup.exe",
    "codex-command-runner": "codex-command-runner.exe",
    "rg": "rg",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Build or stage the ikuncodex npm package.")
    parser.add_argument(
        "--package",
        choices=("codex", "codex-responses-api-proxy", "codex-sdk", *PLATFORM_PACKAGE_METADATA),
        default="codex",
        help="Which npm package to stage (default: codex).",
    )
    parser.add_argument(
        "--version",
        help="Version number to write to package.json inside the staged package.",
    )
    parser.add_argument(
        "--release-version",
        help="Version to stage for npm release.",
    )
    parser.add_argument(
        "--staging-dir",
        type=Path,
        help=(
            "Directory to stage the package contents. Defaults to a new temporary directory "
            "if omitted. The directory must be empty when provided."
        ),
    )
    parser.add_argument(
        "--tmp",
        dest="staging_dir",
        type=Path,
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--pack-output",
        type=Path,
        help="Path where the generated npm tarball should be written.",
    )
    parser.add_argument(
        "--vendor-src",
        type=Path,
        help="Directory containing pre-installed native binaries to bundle (vendor root).",
    )
    return parser.parse_args()


def resolve_package_publish_name(package: str) -> str:
    """Return the actual npm publish name for an internal staging package key."""

    return PACKAGE_PUBLISH_NAMES.get(package, package)


def main() -> int:
    args = parse_args()

    package = args.package
    version = args.version
    release_version = args.release_version
    if release_version:
        if version and version != release_version:
            raise RuntimeError("--version and --release-version must match when both are provided.")
        version = release_version

    if not version:
        raise RuntimeError("Must specify --version or --release-version.")

    staging_dir, created_temp = prepare_staging_dir(args.staging_dir)

    try:
        stage_sources(staging_dir, version, package)

        vendor_src = args.vendor_src.resolve() if args.vendor_src else None
        native_components = PACKAGE_NATIVE_COMPONENTS.get(package, [])
        platform_target = None
        if package in PLATFORM_PACKAGE_METADATA:
            platform_target = str(PLATFORM_PACKAGE_METADATA[package]["target"])

        if native_components:
            if vendor_src is None:
                components_str = ", ".join(native_components)
                raise RuntimeError(
                    "Native components "
                    f"({components_str}) required for package '{package}'. Provide --vendor-src "
                    "pointing to a directory containing pre-installed binaries."
                )

            copy_native_binaries(
                vendor_src,
                staging_dir,
                package,
                native_components,
                target_filter={platform_target} if platform_target else None,
            )

        if release_version:
            emit_release_hints(staging_dir, version, package)
        else:
            print(f"Staged package in {staging_dir}")

        if args.pack_output is not None:
            output_path = run_npm_pack(staging_dir, args.pack_output)
            print(f"npm pack output written to {output_path}")
    finally:
        if created_temp:
            # Preserve the staging directory for further inspection.
            pass

    return 0


def prepare_staging_dir(staging_dir: Path | None) -> tuple[Path, bool]:
    if staging_dir is not None:
        staging_dir = staging_dir.resolve()
        staging_dir.mkdir(parents=True, exist_ok=True)
        if any(staging_dir.iterdir()):
            raise RuntimeError(f"Staging directory {staging_dir} is not empty.")
        return staging_dir, False

    temp_dir = Path(tempfile.mkdtemp(prefix="ikuncodex-npm-stage-"))
    return temp_dir, True


def emit_release_hints(staging_dir: Path, version: str, package: str) -> None:
    staging_dir_str = str(staging_dir)
    if package == "codex":
        print(
            f"Staged version {version} for release in {staging_dir_str}\n\n"
            "Verify the CLI:\n"
            f"    node {staging_dir_str}/bin/codex.js --version\n"
            f"    node {staging_dir_str}/bin/codex.js --help\n"
            f"    npm install -g {staging_dir_str}\n"
            "    ikuncodex --help\n\n"
        )
    elif package in PLATFORM_PACKAGE_METADATA:
        print(
            f"Staged platform package {package} version {version} in {staging_dir_str}\n\n"
            "Verify the vendor payload:\n"
            f"    ls {staging_dir_str}/vendor\n\n"
        )
    elif package == "codex-responses-api-proxy":
        print(
            f"Staged version {version} for release in {staging_dir_str}\n\n"
            "Verify the responses API proxy:\n"
            f"    node {staging_dir_str}/bin/codex-responses-api-proxy.js --help\n\n"
        )
    else:
        print(
            f"Staged version {version} for release in {staging_dir_str}\n\n"
            "Verify the SDK contents:\n"
            f"    ls {staging_dir_str}/dist\n"
            "    node -e \"import('./dist/index.js').then(() => console.log('ok'))\"\n\n"
        )


def stage_sources(staging_dir: Path, version: str, package: str) -> None:
    if package == "codex":
        bin_dir = staging_dir / "bin"
        bin_dir.mkdir(parents=True, exist_ok=True)
        shutil.copy2(CODEX_CLI_ROOT / "bin" / "codex.js", bin_dir / "codex.js")
        rg_manifest = CODEX_CLI_ROOT / "bin" / "rg"
        if rg_manifest.exists():
            shutil.copy2(rg_manifest, bin_dir / "rg")

        # Stage the codex-cli README so npm users see ikuncodex-specific install
        # and command examples instead of the repo root documentation.
        readme_src = CODEX_CLI_ROOT / "README.md"
        if readme_src.exists():
            shutil.copy2(readme_src, staging_dir / "README.md")

        package_json_path = CODEX_CLI_ROOT / "package.json"
    elif package in PLATFORM_PACKAGE_METADATA:
        stage_platform_package(staging_dir, version, package)
        return
    elif package == "codex-responses-api-proxy":
        bin_dir = staging_dir / "bin"
        bin_dir.mkdir(parents=True, exist_ok=True)
        launcher_src = RESPONSES_API_PROXY_NPM_ROOT / "bin" / "codex-responses-api-proxy.js"
        shutil.copy2(launcher_src, bin_dir / "codex-responses-api-proxy.js")

        readme_src = RESPONSES_API_PROXY_NPM_ROOT / "README.md"
        if readme_src.exists():
            shutil.copy2(readme_src, staging_dir / "README.md")

        package_json_path = RESPONSES_API_PROXY_NPM_ROOT / "package.json"
    elif package == "codex-sdk":
        package_json_path = CODEX_SDK_ROOT / "package.json"
        stage_codex_sdk_sources(staging_dir)
    else:
        raise RuntimeError(f"Unknown package '{package}'.")

    with open(package_json_path, "r", encoding="utf-8") as fh:
        package_json = json.load(fh)
    package_json["version"] = version

    if package == "codex":
        # Keep the main package lightweight and let npm install exactly one
        # platform child package via optionalDependencies.
        package_json["optionalDependencies"] = {
            package_name: version for package_name in sorted(PLATFORM_PACKAGE_METADATA)
        }
    elif package == "codex-sdk":
        scripts = package_json.get("scripts")
        if isinstance(scripts, dict):
            scripts.pop("prepare", None)

        dependencies = package_json.get("dependencies")
        if not isinstance(dependencies, dict):
            dependencies = {}
        dependencies[CODEX_NPM_NAME] = version
        package_json["dependencies"] = dependencies

    with open(staging_dir / "package.json", "w", encoding="utf-8") as out:
        json.dump(package_json, out, indent=2)
        out.write("\n")


def stage_platform_package(staging_dir: Path, version: str, package: str) -> None:
    metadata = PLATFORM_PACKAGE_METADATA[package]
    package_json = {
        "name": package,
        "version": version,
        "description": metadata["description"],
        "license": "Apache-2.0",
        "repository": IKUNCODEX_REPOSITORY,
        "homepage": IKUNCODEX_HOMEPAGE,
        "bugs": IKUNCODEX_BUGS,
        "os": metadata["os"],
        "cpu": metadata["cpu"],
        "files": ["vendor"],
    }

    with open(staging_dir / "package.json", "w", encoding="utf-8") as out:
        json.dump(package_json, out, indent=2)
        out.write("\n")

    target = metadata["target"]
    readme = (
        f"# {package}\n\n"
        f"This package contains the prebuilt native vendor files for `{CODEX_NPM_NAME}` on "
        f"`{target}`.\n\n"
        f"Install the main package instead:\n\n"
        f"```bash\nnpm install -g {CODEX_NPM_NAME}\n```\n"
    )
    (staging_dir / "README.md").write_text(readme, encoding="utf-8")


def run_command(cmd: list[str], cwd: Path | None = None) -> None:
    print("+", " ".join(cmd))
    subprocess.run(cmd, cwd=cwd, check=True)


def resolve_node_command(command: str) -> str:
    """Resolve Windows .cmd shims when Python launches npm/pnpm directly."""

    if sys.platform == "win32":
        return shutil.which(f"{command}.cmd") or shutil.which(command) or command
    return shutil.which(command) or command


def stage_codex_sdk_sources(staging_dir: Path) -> None:
    package_root = CODEX_SDK_ROOT

    run_command(["pnpm", "install", "--frozen-lockfile"], cwd=package_root)
    run_command(["pnpm", "run", "build"], cwd=package_root)

    dist_src = package_root / "dist"
    if not dist_src.exists():
        raise RuntimeError("codex-sdk build did not produce a dist directory.")

    shutil.copytree(dist_src, staging_dir / "dist")

    readme_src = package_root / "README.md"
    if readme_src.exists():
        shutil.copy2(readme_src, staging_dir / "README.md")

    license_src = REPO_ROOT / "LICENSE"
    if license_src.exists():
        shutil.copy2(license_src, staging_dir / "LICENSE")


def copy_native_binaries(
    vendor_src: Path,
    staging_dir: Path,
    package: str,
    components: list[str],
    target_filter: set[str] | None = None,
) -> None:
    vendor_src = vendor_src.resolve()
    if not vendor_src.exists():
        raise RuntimeError(f"Vendor source directory not found: {vendor_src}")

    components_set = {component for component in components if component in COMPONENT_DEST_DIR}
    if not components_set:
        return

    vendor_dest = staging_dir / "vendor"
    if vendor_dest.exists():
        shutil.rmtree(vendor_dest)
    vendor_dest.mkdir(parents=True, exist_ok=True)

    copied_targets: set[str] = set()
    for target_dir in vendor_src.iterdir():
        if not target_dir.is_dir():
            continue
        if target_filter is not None and target_dir.name not in target_filter:
            continue

        target_components = set(components_set)
        if "windows" in target_dir.name:
            target_components.update(WINDOWS_ONLY_COMPONENTS.get(package, []))

        dest_target_dir = vendor_dest / target_dir.name
        dest_target_dir.mkdir(parents=True, exist_ok=True)

        for component in target_components:
            dest_dir_name = COMPONENT_DEST_DIR.get(component)
            if dest_dir_name is None:
                continue

            src_component_dir = target_dir / dest_dir_name
            if not src_component_dir.exists():
                raise RuntimeError(
                    f"Missing native component '{component}' in vendor source: {src_component_dir}"
                )
            expected_filename = expected_component_filename(component, target_dir.name)
            expected_binary = src_component_dir / expected_filename
            if not expected_binary.exists():
                raise RuntimeError(
                    f"Missing native file for '{component}' in vendor source: {expected_binary}"
                )

            dest_component_dir = dest_target_dir / dest_dir_name
            if dest_component_dir.exists():
                shutil.rmtree(dest_component_dir)
            shutil.copytree(src_component_dir, dest_component_dir)

        copied_targets.add(target_dir.name)

    if target_filter is not None:
        missing_targets = sorted(target_filter - copied_targets)
        if missing_targets:
            raise RuntimeError(
                "Missing target directories in vendor source: " + ", ".join(missing_targets)
            )


def run_npm_pack(staging_dir: Path, output_path: Path) -> Path:
    output_path = output_path.resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)

    with tempfile.TemporaryDirectory(prefix="ikuncodex-npm-pack-") as pack_dir_str:
        pack_dir = Path(pack_dir_str)
        npm_cmd = resolve_node_command("npm")
        stdout = subprocess.check_output(
            [npm_cmd, "pack", "--json", "--pack-destination", str(pack_dir)],
            cwd=staging_dir,
            text=True,
        )
        try:
            pack_output = json.loads(stdout)
        except json.JSONDecodeError as exc:
            raise RuntimeError("Failed to parse npm pack output.") from exc

        if not pack_output:
            raise RuntimeError("npm pack did not produce an output tarball.")

        tarball_name = pack_output[0].get("filename") or pack_output[0].get("name")
        if not tarball_name:
            raise RuntimeError("Unable to determine npm pack output filename.")

        tarball_path = pack_dir / tarball_name
        if not tarball_path.exists():
            raise RuntimeError(f"Expected npm pack output not found: {tarball_path}")

        shutil.move(str(tarball_path), output_path)

    return output_path


def expected_component_filename(component: str, target: str) -> str:
    filename = COMPONENT_EXPECTED_FILENAMES[component]
    if component in {"codex", "codex-responses-api-proxy", "rg"} and "windows" in target:
        return f"{filename}.exe"
    return filename


if __name__ == "__main__":
    import sys

    sys.exit(main())

#
# 编号（如：1）：修改
# 主要修改内容：将 npm 打包脚本中的主包名统一为 ikuncodex，并补齐本地安装验证提示与缺失常量。
# 修改目的：让 stage/pack/release 流程围绕 ikuncodex 运转，避免发布链路残留旧包名。
#
# 编号（如：2）：修改
# 主要修改内容：补充 Windows 下 npm 命令解析逻辑，优先定位 npm.cmd 可执行入口。
# 修改目的：避免 Python 子进程在 Windows 环境里找不到 npm，保证本地 staging 能顺利产出 tgz。
#
# 编号（如：3）：修改
# 主要修改内容：新增 ikuncodex 平台子包元数据与 staging 逻辑，并让主包通过 optionalDependencies 依赖这些子包。
# 修改目的：把原来超大的单一 npm 包拆成“轻量主包 + 平台子包”，绕开 npm 对超大上传包的限制。
#
# 编号（如：4）：修改
# 主要修改内容：在复制 vendor 时显式校验每个组件对应的关键可执行文件，尤其是 Windows helper 文件。
# 修改目的：避免 staging 在 helper 缺失时仍然表面成功，从而把不完整的平台包误发到 npm。
#
