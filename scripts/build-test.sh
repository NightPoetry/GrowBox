#!/usr/bin/env bash
# 测试包:前端带调试桥(window.__GROWBOX__),后端开 127.0.0.1:19999 + debug_eval/e2e_report IPC。
# 前端与后端的开关必须同时打开,本脚本保证一致。
set -euo pipefail
cd "$(dirname "$0")/.."

echo "[1/2] 前端(测试,带调试桥 VITE_GROWBOX_DEBUG=1)..."
( cd crates/growbox-gui/frontend && npm run build:debug )

echo "[2/2] Tauri 测试包(--features debug-endpoints)..."
( cd crates/growbox-gui && cargo tauri build --features debug-endpoints "$@" )

# ★固定自签名★:用一个稳定证书重签,让 macOS TCC 授权(完全磁盘访问 / 可移除宗卷)跨重建保留,
# 不再每次出包都反复弹窗(ad-hoc 签名 cdhash 每次变 → TCC 当新 app)。证书由 scripts/setup-dev-cert.sh
# 一次性创建;没有则保持 ad-hoc(行为不变)。
# 自动取刚构建出的 bundle(--debug → target/debug,否则 target/release);取最新者,免硬编码踩空。
APP="$(ls -dt target/release/bundle/macos/GrowBox.app target/debug/bundle/macos/GrowBox.app 2>/dev/null | head -1)"
CERT_NAME="GrowBox Dev Cert"

# ★OS 授权 helper app 体系★:把需要持久 OS 授权的小 app 编译进 Contents/Helpers/(疫苗式,见 helpers.rs)。
# 每个 helper 有自己稳定的 bundle id + 签名 → 自己独立的 TCC 身份。注入在签名之前,随主 app 一起签。
if [ -d "$APP" ]; then
  HELPERS="$APP/Contents/Helpers"
  mkdir -p "$HELPERS"
  rm -rf "$HELPERS/ShutdownHelper.app"
  echo "[helper] 编译 ShutdownHelper.app(System Events 自动化关机,免 root)..."
  osacompile -o "$HELPERS/ShutdownHelper.app" scripts/helpers/ShutdownHelper.applescript
  /usr/libexec/PlistBuddy -c "Set :CFBundleIdentifier com.nightpoetry.growbox.shutdownhelper" \
    "$HELPERS/ShutdownHelper.app/Contents/Info.plist" 2>/dev/null \
    || /usr/libexec/PlistBuddy -c "Add :CFBundleIdentifier string com.nightpoetry.growbox.shutdownhelper" \
       "$HELPERS/ShutdownHelper.app/Contents/Info.plist" 2>/dev/null || true
fi

# ★固定自签名★:用一个稳定证书重签,让 macOS TCC 授权(完全磁盘访问 / 可移除宗卷 / helper 自动化)跨重建保留,
# 不再每次出包都反复弹窗(ad-hoc 签名 cdhash 每次变 → TCC 当新 app)。证书由 scripts/setup-dev-cert.sh
# 一次性创建;没有则保持 ad-hoc(行为不变)。先签嵌套 helper(各自 bundle id),再 --deep 签主 app。
# 去掉 -v:自签名证书即便未被标记"信任"(find-identity -v 看不到),codesign 仍能用它签名
# (签名不需要信任,信任只影响验证);用不带 -v 的检测保证证书一存在就启用固定签名。
if [ -d "$APP" ] && security find-identity -p codesigning 2>/dev/null | grep -q "$CERT_NAME"; then
  echo "[签名] 用固定自签名「${CERT_NAME}」重签(主 app + helper,TCC 授权跨重建保留)..."
  [ -d "$APP/Contents/Helpers/ShutdownHelper.app" ] && \
    codesign --force --sign "$CERT_NAME" --identifier com.nightpoetry.growbox.shutdownhelper \
      "$APP/Contents/Helpers/ShutdownHelper.app"
  codesign --force --deep --sign "$CERT_NAME" --identifier com.nightpoetry.growbox "$APP"
  codesign --verify --verbose=2 "$APP" 2>&1 | tail -2 || true
elif [ -d "$APP" ]; then
  echo "[签名] 未找到固定证书「${CERT_NAME}」,保持 ad-hoc(每次重建会重新弹 TCC 授权;helper 授权也不持久)。"
  echo "       一次性修复:运行 scripts/setup-dev-cert.sh 后再出包。"
fi

echo "完成。调试端口:curl http://127.0.0.1:19999/health"
