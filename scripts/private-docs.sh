#!/usr/bin/env bash
# 私密文档保险箱:把"纯 AI 会话内部产物"(记忆镜像/历史交接/决策日志原话/含 key 的交接报告)
# 打包 + AES-256 加密成单个不透明 blob 进 git;明文目录 + 口令文件被 .gitignore 排除。
# 这样这些文档能随仓库安全上传 GitHub(密文公开无害),但只有持本地口令的人能解开。
#
#   pack         明文有变才重新打包加密(据内容哈希判断;--force 强制)
#   unpack       解密还原明文(新机器克隆后、或本地误删后用;需 .private-docs.key)
#   check        明文与上次打包是否一致(一致 exit 0;需重打 exit 1)——给 pre-commit 钩子用
#   init         首次生成随机口令文件(若不存在)+ 建 private/ 目录
#   install-hook 装 pre-commit 钩子(提交前自动 pack + git add,fresh-init 后需重装)
#
# 加密:openssl AES-256-CBC + PBKDF2(600k 迭代)+ 随机盐。口令足够长(随机 48 字节 base64)。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# ── 私密文档清单(要加密、不进明文 git 的)。改这里即改保险箱范围。──
PRIVATE_PATHS=(
  "设计文档/AI记忆快照"
  "设计文档/History"
  "设计文档/用户决策/决策日志.md"
  "交接报告.md"
)

KEYFILE=".private-docs.key"
VAULT="private"
ARCHIVE="$VAULT/docs.tar.gz.enc"
HASHFILE="$VAULT/docs.sha256"
ITER=600000
PUB_MANIFEST="$VAULT/vault-manifest.md5"
LOCAL_MANIFEST="$VAULT/vault-manifest.full.local.md5"
MANIFEST_ZIP="$VAULT/vault-manifest.zip"

# 单个文件的 md5(macOS md5 -q / Linux md5sum)
md5_of() { if command -v md5 >/dev/null 2>&1; then md5 -q "$1"; else md5sum "$1" | awk '{print $1}'; fi; }

die() { echo "[private-docs] 错误: $*" >&2; exit 1; }

ensure_key() {
  [ -f "$KEYFILE" ] || die "缺口令文件 $KEYFILE。先跑 'scripts/private-docs.sh init' 生成,或从其它机器拷过来。"
}

# 现存明文私密文件的组合哈希(内容+路径敏感,mtime 无关);无任何明文则输出空。
hash_current() {
  local existing=()
  local p
  for p in "${PRIVATE_PATHS[@]}"; do [ -e "$p" ] && existing+=("$p"); done
  [ ${#existing[@]} -eq 0 ] && { echo ""; return; }
  # 逐文件 shasum(行含相对路径)→ 排序 → 再哈希 = 内容+路径敏感、顺序无关的组合指纹。
  # 用 -exec(非 -print0/sort -z):macOS BSD sort 不支持 -z;本仓库文件名无换行,行排序即稳定。
  find "${existing[@]}" -type f -exec shasum -a 256 {} \; | LC_ALL=C sort | shasum -a 256 | awk '{print $1}'
}

# 据当前明文 + 密文重生 MD5 证据:公开只哈希 manifest + 无密码 zip + 本地完整(带文件名)清单。
gen_manifest() {
  local existing=() p
  for p in "${PRIVATE_PATHS[@]}"; do [ -e "$p" ] && existing+=("$p"); done
  [ ${#existing[@]} -eq 0 ] && return 0
  find "${existing[@]}" -type f | LC_ALL=C sort | while IFS= read -r f; do echo "$(md5_of "$f")  $f"; done > "$LOCAL_MANIFEST"
  {
    echo "# GrowBox(OPUS 接管项目)私密保险箱内容指纹 — 无密码 · 可公开作证据"
    echo "# 加密归档 $ARCHIVE:"
    echo "#   md5    = $(md5_of "$ARCHIVE")"
    echo "#   sha256 = $(shasum -a 256 "$ARCHIVE" | awk '{print $1}')"
    echo "#   bytes  = $(wc -c < "$ARCHIVE" | tr -d ' ')"
    echo "#   文件数 = $(grep -c . "$LOCAL_MANIFEST")"
    echo "# 下列 = 归档内每个文件的 md5,已排序、不含文件名(隐私)。"
    echo "# 核验: 用口令解密归档 → 对解出的每个文件算 md5 → 确认都在下表中。"
    echo "#"
    awk '{print $1}' "$LOCAL_MANIFEST" | LC_ALL=C sort
  } > "$PUB_MANIFEST"
  if command -v zip >/dev/null 2>&1; then rm -f "$MANIFEST_ZIP"; zip -qj "$MANIFEST_ZIP" "$PUB_MANIFEST"; fi
  echo "[private-docs] 已重生 MD5 证据(公开只哈希 $PUB_MANIFEST + $MANIFEST_ZIP;本地完整 $LOCAL_MANIFEST)"
}

cmd_init() {
  mkdir -p "$VAULT"
  if [ ! -f "$KEYFILE" ]; then
    openssl rand -base64 48 | tr -d '\n' > "$KEYFILE"
    chmod 600 "$KEYFILE"
    echo "[private-docs] 已生成随机口令 → $KEYFILE (chmod 600,已被 .gitignore 排除,切勿入库)"
  else
    echo "[private-docs] 口令文件已存在,跳过。"
  fi
}

cmd_pack() {
  ensure_key
  mkdir -p "$VAULT"
  local cur stored
  cur="$(hash_current)"
  [ -z "$cur" ] && die "找不到任何明文私密文件(${PRIVATE_PATHS[*]});无可打包。"
  stored="$( [ -f "$HASHFILE" ] && cat "$HASHFILE" || echo "" )"
  if [ "${1:-}" != "--force" ] && [ "$cur" = "$stored" ]; then
    echo "[private-docs] 明文无变化,保险箱已是最新(免重打)。"
    return 0
  fi
  local existing=()
  local p
  for p in "${PRIVATE_PATHS[@]}"; do [ -e "$p" ] && existing+=("$p"); done
  tar -czf - "${existing[@]}" \
    | openssl enc -aes-256-cbc -salt -pbkdf2 -iter "$ITER" -pass "file:$KEYFILE" -out "$ARCHIVE"
  echo "$cur" > "$HASHFILE"
  echo "[private-docs] 已重新打包加密 → $ARCHIVE ($(du -h "$ARCHIVE" | awk '{print $1}'))  指纹 → $HASHFILE"
  gen_manifest
}

cmd_unpack() {
  ensure_key
  [ -f "$ARCHIVE" ] || die "缺密文 $ARCHIVE。"
  openssl enc -d -aes-256-cbc -pbkdf2 -iter "$ITER" -pass "file:$KEYFILE" -in "$ARCHIVE" \
    | tar -xzf - -C "$ROOT"
  echo "[private-docs] 已解密还原明文(${PRIVATE_PATHS[*]})。"
}

cmd_check() {
  local cur stored
  cur="$(hash_current)"
  [ -z "$cur" ] && { echo "[private-docs] 无明文(可能未 unpack),check 跳过。"; return 0; }
  stored="$( [ -f "$HASHFILE" ] && cat "$HASHFILE" || echo "" )"
  if [ "$cur" = "$stored" ]; then echo "[private-docs] 最新。"; return 0; else echo "[private-docs] 明文已变,需 pack。"; return 1; fi
}

cmd_install_hook() {
  local hookdir=".git/hooks"
  [ -d "$hookdir" ] || die "找不到 $hookdir(不在 git 仓库?)"
  cat > "$hookdir/pre-commit" <<'HOOK'
#!/usr/bin/env bash
# 自动重打私密保险箱:明文有变就重新加密并 git add,保证提交里密文始终最新。
set -e
ROOT="$(git rev-parse --show-toplevel)"
if [ -f "$ROOT/.private-docs.key" ]; then
  bash "$ROOT/scripts/private-docs.sh" pack
  git add "$ROOT/private/docs.tar.gz.enc" "$ROOT/private/docs.sha256" "$ROOT/private/vault-manifest.md5" "$ROOT/private/vault-manifest.zip" 2>/dev/null || true
else
  if ! bash "$ROOT/scripts/private-docs.sh" check >/dev/null 2>&1; then
    echo "[pre-commit] 警告:无 .private-docs.key 且私密明文与保险箱不一致,跳过自动重打。" >&2
  fi
fi
HOOK
  chmod +x "$hookdir/pre-commit"
  echo "[private-docs] 已装 pre-commit 钩子 → $hookdir/pre-commit"
}

case "${1:-}" in
  init)         cmd_init ;;
  pack)         shift; cmd_pack "${1:-}" ;;
  unpack)       cmd_unpack ;;
  check)        cmd_check ;;
  install-hook) cmd_install_hook ;;
  *) echo "用法: scripts/private-docs.sh {init|pack [--force]|unpack|check|install-hook}"; exit 2 ;;
esac
