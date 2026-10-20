#!/usr/bin/env bash
# 生成/更新 homebrew-tap 里的 RowBridge cask。
#
# 用法：
#   ./scripts/update-homebrew-cask.sh                      # 用 Cargo.toml 的版本
#   ./scripts/update-homebrew-cask.sh 0.2.0                # 指定版本
#   ./scripts/update-homebrew-cask.sh --dry-run            # 只打印，不写文件
#   ./scripts/update-homebrew-cask.sh --output path/to.rb
#
# 它做三件事：
#   1. 校验本地/远端 tag 存在且指向同一 commit
#   2. 校验 tag 里的版本号与请求的一致（避免发布错版本）
#   3. 下载 release 里的 dmg 算 sha256，写进 cask
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

OWNER="${OWNER:-life2you}"
REPO="${REPO:-rowbridge}"
TAP_REPO="${TAP_REPO:-homebrew-tap}"
HOMEPAGE="https://github.com/$OWNER/$REPO"

VERSION=""
CASK_PATH=""
DRY_RUN=0

require_command() {
  command -v "$1" >/dev/null 2>&1 || { echo "缺少命令: $1" >&2; exit 1; }
}
require_command git
require_command curl
require_command shasum

show_help() {
  cat <<EOF
Usage: ./scripts/update-homebrew-cask.sh [options] [version]

Options:
  --output PATH   写到指定路径（默认 ../homebrew-tap/Casks/rowbridge.rb）
  --dry-run       只打印内容，不写文件
  --help          显示帮助

默认版本来源：Cargo.toml
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --output)   CASK_PATH="$2"; shift 2 ;;
    --output=*) CASK_PATH="${1#*=}"; shift ;;
    --dry-run)  DRY_RUN=1; shift ;;
    --help)     show_help; exit 0 ;;
    --) shift; [[ $# -gt 0 ]] && { VERSION="$1"; shift; }; break ;;
    -*) echo "未知选项: $1" >&2; exit 1 ;;
    *)  [[ -n "$VERSION" ]] && { echo "多余参数: $1" >&2; exit 1; }; VERSION="$1"; shift ;;
  esac
done

VERSION="${VERSION:-$(sed -n 's/^version = "\(.*\)"/\1/p' "$REPO_ROOT/Cargo.toml" | head -n1)}"
[[ -z "$VERSION" ]] && { echo "无法从 Cargo.toml 取到版本号" >&2; exit 1; }
CASK_PATH="${CASK_PATH:-$REPO_ROOT/../$TAP_REPO/Casks/rowbridge.rb}"

TAG="v$VERSION"

# --- 1. tag 必须存在且本地远端一致 ---
LOCAL_TAG_COMMIT="$(git -C "$REPO_ROOT" rev-parse "$TAG^{}" 2>/dev/null || true)"
if [[ -z "$LOCAL_TAG_COMMIT" ]]; then
  echo "本地没有 tag $TAG。先创建并推送 tag 再更新 cask。" >&2
  exit 1
fi

TAG_VERSION="$(
  git -C "$REPO_ROOT" show "$TAG:Cargo.toml" |
    sed -n 's/^version = "\(.*\)"/\1/p' | head -n1
)"
if [[ "$TAG_VERSION" != "$VERSION" ]]; then
  echo "tag $TAG 里的版本是 ${TAG_VERSION:-<未知>}，与请求的 $VERSION 不一致。" >&2
  echo "拒绝从版本不匹配的 tag 生成 cask。" >&2
  exit 1
fi

REMOTE_TAG_COMMIT="$(git -C "$REPO_ROOT" ls-remote --tags origin "refs/tags/$TAG^{}" | awk 'NR==1 {print $1}')"
[[ -z "$REMOTE_TAG_COMMIT" ]] && REMOTE_TAG_COMMIT="$(git -C "$REPO_ROOT" ls-remote --tags origin "refs/tags/$TAG" | awk 'NR==1 {print $1}')"
if [[ -z "$REMOTE_TAG_COMMIT" ]]; then
  echo "远端没有 tag $TAG。先推送 tag。" >&2
  exit 1
fi
if [[ "$REMOTE_TAG_COMMIT" != "$LOCAL_TAG_COMMIT" ]]; then
  echo "远端 tag $TAG 指向 $REMOTE_TAG_COMMIT，本地指向 $LOCAL_TAG_COMMIT。" >&2
  exit 1
fi

# --- 2. 下载 dmg 算 sha256 ---
DMG_NAME="RowBridge_${VERSION}_aarch64.dmg"
DMG_URL="$HOMEPAGE/releases/download/$TAG/$DMG_NAME"

echo "正在计算 sha256：$DMG_URL"
SHA256="$(
  curl --fail --silent --show-error --location --retry 3 "$DMG_URL" |
    shasum -a 256 | awk '{print $1}'
)"
[[ -z "$SHA256" ]] && { echo "无法获取 dmg 或计算 sha256" >&2; exit 1; }

# --- 3. 渲染 cask ---
CASK_CONTENT="$(cat <<EOF
cask "rowbridge" do
  version "$VERSION"
  sha256 "$SHA256"

  url "https://github.com/$OWNER/$REPO/releases/download/v#{version}/RowBridge_#{version}_aarch64.dmg"
  name "RowBridge"
  desc "MySQL data sync tool that copies tables by primary-key batches"
  homepage "$HOMEPAGE"

  depends_on macos: :catalina
  depends_on arch: :arm64

  app "RowBridge.app"

  postflight do
    system_command "xattr",
                   args: ["-cr", "#{appdir}/RowBridge.app"]
    system_command "codesign",
                   args: ["--force", "--deep", "--sign", "-", "#{appdir}/RowBridge.app"]
  end

  caveats <<~EOS
    RowBridge 未做 Apple 开发者签名，首次启动若提示"已损坏"，可在终端运行：
      xattr -cr /Applications/RowBridge.app

    默认连接 localhost:3306。建议先用测试库验证：
      源 127.0.0.1:3307  目标 127.0.0.1:3308
  EOS

  zap trash: [
    "~/Library/Application Support/com.rowbridge.app",
    "~/Library/Caches/com.rowbridge.app",
    "~/Library/Preferences/com.rowbridge.app.plist",
    "~/Library/Saved Application State/com.rowbridge.app.savedState",
  ]
end
EOF
)"

if [[ "$DRY_RUN" == "1" ]]; then
  printf '%s\n' "$CASK_CONTENT"
  {
    echo "---"
    echo "仅试运行"
    echo "版本:     $VERSION"
    echo "SHA256:   $SHA256"
    echo "输出路径: $CASK_PATH"
  } >&2
  exit 0
fi

mkdir -p "$(dirname "$CASK_PATH")"
TMP="$(mktemp "$(dirname "$CASK_PATH")/rowbridge.rb.XXXXXX")"
trap 'rm -f "$TMP"' EXIT
printf '%s\n' "$CASK_CONTENT" > "$TMP"
mv "$TMP" "$CASK_PATH"

echo "已更新 $CASK_PATH"
echo "版本:   $VERSION"
echo "SHA256: $SHA256"
