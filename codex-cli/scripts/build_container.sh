#!/bin/bash

set -euo pipefail

SCRIPT_DIR=$(realpath "$(dirname "$0")")
trap "popd >> /dev/null" EXIT
pushd "$SCRIPT_DIR/.." >> /dev/null || {
  echo "Error: Failed to change directory to $SCRIPT_DIR/.."
  exit 1
}
pnpm install
pnpm run build
rm -rf ./dist/ikuncodex-*.tgz
pnpm pack --pack-destination ./dist
mv ./dist/ikuncodex-*.tgz ./dist/ikuncodex.tgz
docker build -t ikuncodex -f "./Dockerfile" .

# 编号（如：1）：修改
# 主要修改内容：将容器打包脚本中的 tgz 文件名与镜像标签切换为 ikuncodex。
# 修改目的：让容器构建产物与 npm 包的新名称保持一致，减少发布时的名字混淆。
