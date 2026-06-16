#!/bin/bash
# GrowBox GUI 启动脚本
#
# 用法:
#   ./scripts/launch.sh              # build + 启动 growbox
#   ./scripts/launch.sh --no-build   # 跳过 build,直接启动已有 release binary
#
# 注意:必须用 tauri build(cargo build 不嵌入前端 dist)。

set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BUILD=true
for arg in "$@"; do
  case "$arg" in
    --no-build) BUILD=false ;;
    -h|--help) sed -n '1,10p' "$0"; exit 0 ;;
    *) echo "未知参数: $arg" >&2; exit 1 ;;
  esac
done

CRATE="crates/growbox-gui"
BIN="target/release/growbox"
LOG="/tmp/growbox.log"

echo "GrowBox GUI 启动"

# build
if [ "$BUILD" = "true" ]; then
  echo "-- build via tauri (必须用 tauri build,cargo build 不嵌 dist)"
  # 先重建前端(tauri.conf.json 的 beforeBuildCommand 是空,不会自动重建)
  echo "-- 重建前端"
  (cd "$CRATE/frontend" && npm run build)
  echo "-- tauri build"
  (cd "$CRATE" && cargo tauri build --no-bundle)
fi

# kill old
echo "-- kill 旧进程"
pkill -f "target/release/growbox" 2>/dev/null || true
sleep 1

# launch(必须从项目根目录启动)
echo "-- 启动 $BIN (日志: $LOG)"
RUST_LOG=info "$BIN" > "$LOG" 2>&1 &
PID=$!
echo "  PID: $PID"

echo ""
echo "GrowBox 已启动"
echo "  日志: tail -f $LOG"
echo "  停止: kill $PID"
