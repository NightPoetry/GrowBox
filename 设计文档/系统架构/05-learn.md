# 05 — learn

## 职责
只管**飞轮**:从经验提炼→压缩→验证→泛化结论,元优化,永久目标调度;不管存原文(归 memory)。

## 接口
```rust
pub struct Flywheel { /* distiller / compressor / verifier / generalizer */ }
impl Flywheel {
    pub fn collect(&self, snapshot: Snapshot) -> Conclusion;        // 收集:经验
    pub async fn turn(&self, mem: &mut Memory, llm: &LlmRouter);    // 提炼→压缩→验证→泛化一轮
}
pub struct Scheduler { /* idle 时按 L0 > P1 > P2 > P3 调度 */ }
```

## 依赖
→ 依赖:core、memory、llm。 ← 被依赖:app(循环 ⑤ + idle 调度)。

## 数据流
```
每次操作 → collect() → 经验入 memory
idle → Scheduler:
  P1 元优化(检索/索引自调) → P2 探索冲动(验证未验证猜想) → P3 最大化自动化
turn(): 提炼(聚类) → 压缩(最少前提/最大推论→猜想) → 验证(可回滚/不可回滚/自验证) → 泛化(放宽定义域)
```

## 接原理
- `设计/04` 全篇:五阶段、自指元优化、三永久目标(P1/P2/P3)。
- `设计/01` 原则2(自动化闭环):P3 = 消除人的中间人角色。

## 已知坑
- 旧 learner/conclusion 没进工作区、只活在注释里 → 本次 learn 是脊柱一环,会话结束**必调** collect,idle **必调** turn。
- 旧种子结论很多是错的(如 flash 用法)→ 初始飞轮种子 = 已实测验证的结论 + `设计/` 原则。
