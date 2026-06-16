#!/usr/bin/env bash
# 一次性:创建一个固定的自签名代码签名证书「GrowBox Dev Cert」,导入登录钥匙串并信任用于代码签名。
#
# 为什么需要:dev 包默认 ad-hoc 签名(codesign -s -),cdhash 每次重建都变 → macOS TCC
# 把每个新包当成"新 app"→ 上次授予的「完全磁盘访问 / 可移除宗卷」权限作废、反复弹窗。
# 用一个固定证书签名后,app 的 designated requirement 稳定 → TCC 授权一次,跨重建保留。
#
# 用法:scripts/setup-dev-cert.sh  (会要 1-2 次登录密码:导入私钥分区表 + 设置信任)
# 幂等:已存在同名身份则跳过。
set -euo pipefail

CERT_NAME="GrowBox Dev Cert"
LOGIN_KC="$HOME/Library/Keychains/login.keychain-db"

# 幂等:用不带 -v 的检测(连"已导入但未受信任"的也能看到)→ 已存在就绝不重建(重建会因每次
# 重新生成密钥对而产生重复同名证书,codesign --sign 时歧义)。
if security find-identity -p codesigning 2>/dev/null | grep -q "$CERT_NAME"; then
  echo "已存在签名身份「${CERT_NAME}」(无论是否受信任),无需重建。"
  echo "  · 出包会自动用它:scripts/build-test.sh / build-official.sh"
  echo "  · 给已出 .app 补签:codesign --force --deep --sign \"${CERT_NAME}\" --identifier com.nightpoetry.growbox <App路径>"
  echo "  · (可选)让 find-identity -v 也显示/签名验证为受信任:钥匙串 App 双击该证书 → 信任 → 代码签名:始终信任。"
  exit 0
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "[1/4] 生成带 codeSigning 扩展的自签名证书..."
cat > "$TMP/openssl.cnf" <<'CNF'
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = GrowBox Dev Cert
[v3]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$TMP/key.pem" -out "$TMP/cert.pem" \
  -days 3650 -config "$TMP/openssl.cnf" >/dev/null 2>&1
# -legacy 必须:OpenSSL 3 默认 PKCS12 用新 MAC/加密,macOS security 读不了("MAC verification failed")。
openssl pkcs12 -export -legacy -inkey "$TMP/key.pem" -in "$TMP/cert.pem" \
  -name "$CERT_NAME" -out "$TMP/cert.p12" -passout pass:growbox >/dev/null 2>&1

echo "[2/4] 导入登录钥匙串并授权 codesign 使用..."
security import "$TMP/cert.p12" -k "$LOGIN_KC" -P growbox -T /usr/bin/codesign

echo "[3/4] 信任该证书用于代码签名(会弹一次"修改证书信任设置"要登录密码)..."
# 让 find-identity -v 能列出它(出包脚本检测用),并让签名可被验证为受信任。
# 失败也不致命:codesign 用未受信任的自签名证书照样能签;build 脚本的检测已不依赖 -v(见下)。
security add-trusted-cert -r trustRoot -p codeSign -k "$LOGIN_KC" "$TMP/cert.pem" 2>/dev/null \
  || echo "  (信任设置未完成也不要紧:codesign 仍能用它签,TCC 稳定性不受影响。)"

echo "[4/4] 设置 key partition list(免每次签名弹密码;可能再要一次登录密码)..."
security set-key-partition-list -S apple-tool:,apple:,codesign: "$LOGIN_KC" >/dev/null 2>&1 \
  || echo "  (这步失败不致命;首次签名时可能弹一次钥匙串授权,点"始终允许"即可。)"

echo
echo "完成。已创建固定签名身份「${CERT_NAME}」。"
echo "下一步:scripts/build-test.sh / build-official.sh 出包(会自动用它签名);"
echo "或给已出的 .app 补签:codesign --force --deep --sign \"${CERT_NAME}\" --identifier com.nightpoetry.growbox <App>"
echo "首次启动后给该 .app 授一次「完全磁盘访问」,之后重建不再反复弹窗。"
