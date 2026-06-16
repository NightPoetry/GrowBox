# 03 — safety

## 职责
只管**判定一个操作能不能做、要不要问用户**:沙箱路径分级、黑名单、风险评级、三种授权;不管执行操作本身(那是执行器)。

## 接口
```rust
pub struct Sandbox { /* writable_roots / readonly_roots / 黑名单 */ }
impl Sandbox {
    pub fn judge(&self, op: &Operation) -> Verdict;
    pub fn grant(&mut self, scope: GrantScope);   // 用户授权后记入项目级配置
}
pub enum Verdict { Allow, NeedAuth{reason: String}, Deny{reason: String} }
pub enum GrantScope { Once, ThisProjectPath(PathBuf), ThisProject }
```

## 依赖
→ 依赖:core。 ← 被依赖:app(Agent 循环 ③ 安全门)。

## 数据流
`Operation → judge() → {Allow→直接执行 | NeedAuth→弹授权(reason) | Deny→拒绝}`。
用户点授权范围 → `grant()` 写入项目级配置 → 后续同类放行。

## 接原理
- `设计/03` 原则1(默认安全,越界即交还):judge 默认拒,越界返回 NeedAuth 带 reason。
- `设计/03` 推论3:`GrantScope` 三档(Once / ThisProjectPath / ThisProject)。
- `设计/00` 推论4(可逆性定自主度):`Risk::Reversible` 直接 Allow,`Irreversible` 走 NeedAuth。

## 已知坑
- 旧 auth 的沙箱判定散在 cmds.rs 调用点 → 本次统一为 `judge()` 单入口。
- 路径必须 canonicalize 后判断(`../` 逃逸);放行 `2>/dev/null` 等标准 idiom(旧代码已有此修复,保留思路)。
