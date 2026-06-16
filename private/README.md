# private/ — 加密的私密文档保险箱

`docs.tar.gz.enc` 是 **AES-256(PBKDF2/600k 迭代/随机盐)** 加密的不透明归档,内含纯 AI
会话内部产物 —— 记忆镜像、历史交接、决策日志原话、含 key 的交接报告。**密文公开无害**,
可随仓库安全上传 GitHub。

## 用法

- 还原明文(新机器克隆后,或本地误删):`scripts/private-docs.sh unpack`
- 明文改完重新加密:`scripts/private-docs.sh pack`(已装 pre-commit 钩子则提交时自动做)
- 首次/新机器装钩子:`scripts/private-docs.sh install-hook`

## 解密口令

口令在仓库根 `.private-docs.key`(64 字符随机串,`*.key` 已被 .gitignore 排除,**绝不入库**)。
**新机器需把这个文件另行带来**(U 盘/密码管理器),克隆下来的仓库只有密文、没有口令。

## docs.sha256

明文集合的内容指纹(只是个哈希,不泄露内容),用于判断明文是否变化、需不需要重打。
