#!/usr/bin/env bash
# 把 GPUI 版界面打成一个标准 macOS .app。
#
# 直接从终端跑二进制时，Dock / Cmd-Tab 里显示的是可执行文件的默认图标，
# 名称也是 rowbridge-gpui。打成 .app 之后就是正常的 RowBridge，图标与
# Tauri 版一致（同一个 src-tauri/icons/icon.icns）。
#
# 用法：
#   ./scripts/bundle-gpui.sh              # release 构建并打包到 dist-gpui/
#   ./scripts/bundle-gpui.sh --debug      # 用已有的 debug 二进制，打得快
#   ./scripts/bundle-gpui.sh --install    # 额外复制到 /Applications
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

PROFILE="release"
INSTALL=0
for arg in "$@"; do
  case "$arg" in
    --debug)   PROFILE="debug" ;;
    --release) PROFILE="release" ;;
    --install) INSTALL=1 ;;
    -h|--help)
      sed -n '2,14p' "$0"
      exit 0
      ;;
    *) echo "未知参数: $arg" >&2; exit 1 ;;
  esac
done

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
[[ -z "$VERSION" ]] && { echo "无法从 Cargo.toml 取到版本号" >&2; exit 1; }

BIN="$REPO_ROOT/target/$PROFILE/rowbridge-gpui"
if [[ "$PROFILE" == "release" ]]; then
  echo "==> 构建 release 二进制"
  cargo build -p rowbridge-gpui --release
else
  echo "==> 构建 debug 二进制"
  cargo build -p rowbridge-gpui
fi
[[ -x "$BIN" ]] || { echo "找不到二进制: $BIN" >&2; exit 1; }

APP="$REPO_ROOT/dist-gpui/RowBridge.app"
echo "==> 组装 $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
# 可执行文件名即 Dock / Cmd-Tab / 菜单栏显示的名字，改成 RowBridge
cp "$BIN" "$APP/Contents/MacOS/RowBridge"
cp src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>RowBridge</string>
    <key>CFBundleDisplayName</key>
    <string>RowBridge</string>
    <key>CFBundleExecutable</key>
    <string>RowBridge</string>
    <key>CFBundleIdentifier</key>
    <string>com.rowbridge.app.native</string>
    <key>CFBundleIconFile</key>
    <string>icon</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>
    <string>12.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.developer-tools</string>
</dict>
</plist>
PLIST

echo "==> ad-hoc 签名"
codesign --force --deep --sign - "$APP" >/dev/null 2>&1 || echo "   （签名失败，忽略）"

if [[ "$INSTALL" == "1" ]]; then
  echo "==> 复制到 /Applications"
  rm -rf "/Applications/RowBridge.app"
  cp -R "$APP" "/Applications/RowBridge.app"
  echo "已安装 /Applications/RowBridge.app"
fi

echo "完成：$APP"
echo "版本：${VERSION}（${PROFILE}）"
echo "打开：open \"$APP\""
