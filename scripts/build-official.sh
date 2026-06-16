#!/usr/bin/env bash
# 正式包:前端无调试桥,后端无 19999 端口 / 无 debug IPC。
#   --with-model  把本地 e5 模型(约 470MB)随包(full 变体);省略=lite(首启联网下载)。
#   其余参数透传给 cargo tauri build(如 --debug)。
set -euo pipefail
cd "$(dirname "$0")/.."

WITH_MODEL=0
PASS=()
for a in "$@"; do
  if [ "$a" = "--with-model" ]; then WITH_MODEL=1; else PASS+=("$a"); fi
done

MODEL_NAME="multilingual-e5-small"
MODEL_DST="crates/growbox-gui/models/$MODEL_NAME"
FILES=(config.json tokenizer.json model.safetensors)

stage_model() {
  # 三件齐了就跳过。
  local missing=0
  for f in "${FILES[@]}"; do [ -f "$MODEL_DST/$f" ] || missing=1; done
  if [ "$missing" = 0 ]; then echo "  模型已就位:$MODEL_DST"; return; fi
  mkdir -p "$MODEL_DST"
  # 来源优先级:assets/models(仓库内 staging,gitignore)→ HuggingFace 本地缓存快照。
  local src=""
  if [ -f "assets/models/$MODEL_NAME/model.safetensors" ]; then
    src="assets/models/$MODEL_NAME"
  else
    src=$(find "$HOME/.cache/huggingface/hub/models--intfloat--multilingual-e5-small/snapshots" \
      -maxdepth 1 -mindepth 1 -type d 2>/dev/null | head -1)
  fi
  if [ -z "$src" ] || [ ! -f "$src/model.safetensors" ]; then
    echo "错误:找不到 e5 模型源。先跑一次真机下载(live_e5 测试)或把权重放到 assets/models/$MODEL_NAME/" >&2
    exit 1
  fi
  echo "  从 $src 暂存模型到 $MODEL_DST ..."
  for f in "${FILES[@]}"; do cp -L "$src/$f" "$MODEL_DST/$f"; done
}

echo "[1/2] 前端(正式,无调试桥)..."
( cd crates/growbox-gui/frontend && npm run build )

CONF_ARG=()
if [ "$WITH_MODEL" = 1 ]; then
  echo "[*] 随包 e5 模型(full 变体)..."
  stage_model
  CONF_ARG=(--config tauri.bundle-model.conf.json)
else
  echo "[*] lite 变体(不随模型,首启联网下载)。"
fi

echo "[2/2] Tauri 正式包(无 debug-endpoints feature)..."
# bash 3.2(macOS)+ set -u:空数组展开会报 unbound,用 ${arr[@]+...} 守护。
( cd crates/growbox-gui && cargo tauri build ${CONF_ARG[@]+"${CONF_ARG[@]}"} ${PASS[@]+"${PASS[@]}"} )

# ★固定自签名(与 build-test.sh 同)★:正式包(你的日常用包)也用固定证书签,TCC 授权跨重建保留;
# 顺带注入 ShutdownHelper(自关机功能的免 root helper,各自稳定 bundle id → 自己的 TCC 身份)。
APP="$(ls -dt target/release/bundle/macos/GrowBox.app target/debug/bundle/macos/GrowBox.app 2>/dev/null | head -1)"
CERT_NAME="GrowBox Dev Cert"
if [ -n "$APP" ] && [ -d "$APP" ]; then
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
# 去 -v(同 build-test.sh):证书一存在就用,未受信任也能签。先签嵌套 helper,再 --deep 签主 app。
if [ -n "$APP" ] && [ -d "$APP" ] && security find-identity -p codesigning 2>/dev/null | grep -q "$CERT_NAME"; then
  echo "[签名] 用固定自签名「${CERT_NAME}」重签(主 app + helper,TCC 授权跨重建保留)..."
  [ -d "$APP/Contents/Helpers/ShutdownHelper.app" ] && \
    codesign --force --sign "$CERT_NAME" --identifier com.nightpoetry.growbox.shutdownhelper \
      "$APP/Contents/Helpers/ShutdownHelper.app"
  codesign --force --deep --sign "$CERT_NAME" --identifier com.nightpoetry.growbox "$APP"
  codesign --verify --verbose=2 "$APP" 2>&1 | tail -2 || true
elif [ -n "$APP" ] && [ -d "$APP" ]; then
  echo "[签名] 未找到固定证书「${CERT_NAME}」,保持 ad-hoc(运行 scripts/setup-dev-cert.sh 后再出包可固定签名)。"
fi

echo "完成。产物在 target/release/bundle/(或加 --debug 时 target/debug/bundle/)。"
