# Fork 维护指南

本文档说明如何维护 `@cometix/codex` fork，包括同步上游更新和发布新版本。

## 仓库配置

```bash
# 远程仓库配置
origin    -> git@github.com:Haleclipse/codex.git  # Fork 仓库
upstream  -> https://github.com/openai/codex.git  # 上游仓库

# 如果 upstream 不存在，添加：
git remote add upstream https://github.com/openai/codex.git
```

## 同步上游更新

### 1. 检查上游更新

```bash
# 获取上游更新
git fetch upstream

# 查看上游新提交
git log main..upstream/main --oneline

# 查看上游新版本 tags
git tag -l "rust-v*" --sort=-version:refname | head -10
```

### 2. 合并上游更改

```bash
# 确保在 main 分支
git checkout main

# 合并上游 main
git merge upstream/main
```

### 3. 解决冲突

合并时可能产生冲突，需要保留以下定制化更改：

| 文件 | 保留内容 |
|------|---------|
| `codex-cli/package.json` | `"name": "@cometix/codex"`, `"url": "git+https://github.com/Haleclipse/codex.git"` |
| `codex-cli/README.md` | Cometix 品牌、`@cometix/codex` 包名 |
| `codex-rs/responses-api-proxy/npm/package.json` | `Haleclipse/codex` URL |
| `shell-tool-mcp/package.json` | `Haleclipse/codex` URL |
| `sdk/typescript/package.json` | `Haleclipse/codex` URL |
| `.github/workflows/rust-release.yml` | 见下方详细说明 |
| `.github/dotslash-config.json` | 无 `windows-aarch64` 平台 |
| `scripts/stage_npm_packages.py` | `GITHUB_REPO = "Haleclipse/codex"` |
| `codex-cli/scripts/install_native_deps.py` | `Haleclipse/codex`, 无 `aarch64-pc-windows-msvc` |

#### rust-release.yml 定制化内容

```yaml
# 1. Tag 检查支持 cometix 后缀
[[ "${GITHUB_REF_NAME}" =~ ^rust-v[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta|cometix)(\.[0-9]+)?)?$ ]]

# 2. 使用免费 macOS runners
- runner: macos-latest  # 而非 macos-15-xlarge

# 3. 移除代码签名步骤（无签名密钥）

# 4. cometix 版本发布为 latest
elif [[ "${version}" =~ ^[0-9]+\.[0-9]+\.[0-9]+-cometix(\.[0-9]+)?$ ]]; then
  echo "should_publish=true" >> "$GITHUB_OUTPUT"
  echo "npm_tag=latest" >> "$GITHUB_OUTPUT"  # 显式指定 latest（预发布版本必须指定 tag）

# 5. npm scope 为 @cometix
scope: "@cometix"

# 6. 移除 update-branch job（无 latest-alpha-cli 分支）

# 7. 只发布 codex 包（移除 sdk、responses-api-proxy）
./scripts/stage_npm_packages.py \
  --release-version "${{ steps.release_name.outputs.name }}" \
  --package codex
```

### 4. 推送合并结果

```bash
git push origin main
```

## 发布新版本

### 版本号规则

- 跟随上游版本号，添加 `-cometix` 后缀
- 例如：上游 `0.88.0` → 我们发布 `0.88.0-cometix`
- main 分支始终保持 `version = "0.0.0"`

### 发布流程

```bash
# 1. 保存当前 main SHA
MAIN_SHA=$(git rev-parse HEAD)

# 2. 修改版本号（假设发布 0.88.0-cometix）
sed -i '' 's/version = "0.0.0"/version = "0.88.0-cometix"/' codex-rs/Cargo.toml

# 3. 创建 release commit
git add codex-rs/Cargo.toml
git commit -m "release: 0.88.0-cometix"

# 4. 创建并推送 tag
git tag -a rust-v0.88.0-cometix -m "Release 0.88.0-cometix"
git push origin rust-v0.88.0-cometix

# 5. 重置 main 分支（保持 0.0.0）
git reset --hard $MAIN_SHA
```

### CI 自动化

推送 tag 后，GitHub Actions 会自动：

1. **tag-check** - 验证 tag 格式和版本号匹配
2. **build** - 构建所有平台的二进制文件
   - Linux: x86_64, aarch64 (musl + gnu)
   - macOS: x86_64, aarch64
   - Windows: x86_64
3. **release** - 创建 GitHub Release，上传 artifacts
4. **publish-npm** - 发布 `@cometix/codex` 到 npm

### npm OIDC Trusted Publishing

npm 发布使用 OIDC 认证，需要在 npm 上配置：

- **Package**: `@cometix/codex`
- **Repository**: `Haleclipse/codex`
- **Workflow**: `rust-release.yml`
- **Environment**: 留空

## 常见问题

### Q: 合并后 CI 失败怎么办？

检查以下常见问题：

1. **tag-check 失败** - 确认 Cargo.toml 版本号与 tag 匹配
2. **build 失败** - 检查是否有新的构建目标需要配置
3. **release 失败** - 检查 dotslash-config.json 是否有新平台
4. **publish-npm 失败** - 检查 package.json repository URL

### Q: 上游添加了新的构建目标怎么办？

1. 检查是否需要付费 runners（如 `windows-aarch64`）
2. 如不支持，从以下文件中移除：
   - `BINARY_TARGETS` in `install_native_deps.py`
   - `RG_TARGET_PLATFORM_PAIRS` in `install_native_deps.py`
   - `.github/dotslash-config.json`

### Q: 如何手动设置 npm latest tag？

```bash
npm dist-tag add @cometix/codex@<version> latest
```

## 文件清单

Fork 定制化涉及的所有文件：

```
.github/
├── dotslash-config.json          # 移除 windows-aarch64
└── workflows/
    └── rust-release.yml          # 免费 runners, cometix 配置

codex-cli/
├── package.json                  # @cometix scope, Haleclipse URL
├── README.md                     # Cometix 品牌
└── scripts/
    └── install_native_deps.py    # Haleclipse repo, 移除 aarch64-windows

codex-rs/
└── responses-api-proxy/
    └── npm/
        └── package.json          # Haleclipse URL

scripts/
└── stage_npm_packages.py         # Haleclipse repo

sdk/typescript/
└── package.json                  # Haleclipse URL

shell-tool-mcp/
└── package.json                  # Haleclipse URL
```
