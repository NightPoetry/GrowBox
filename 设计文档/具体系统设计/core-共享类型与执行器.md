# core(growbox-core)—— 共享类型与执行器契约

## 职责
零依赖底座:被其余所有 crate 依赖,自己不依赖任何业务 crate。放"大家都要用的类型 + 能力的统一契约"。

## 实际设计
- `executor.rs` —— **`Executor` trait(架构公理的契约)**。一切能力(读写文件、shell、后台任务、finish 等)都实现它;`execute` 为 **async**(地基改造,见记忆 `foundation-async-executor`)。Agent 循环通过注册表按统一分发路径调用,安全门只在一处。
- `conclusion.rs` —— **结论模型**:经验/知识/理解同一模型(`Conclusion`),带 `superseded_by`(append-only 进化史,不删旧版)。飞轮压缩与记忆共用。
- `project.rs` —— 项目配置类型(`ProjectConfig` 等)。
- `lib.rs` —— `Timestamp` / `now()` 等时间工具(基于 chrono)。

## 用的库
serde / serde_json(序列化)、chrono(时间)、uuid(id)、sha2(派生 id)、async-trait(trait async)。**无任何第三方业务依赖**。

## 关键文件
`crates/growbox-core/src/{executor,conclusion,project,lib}.rs`

## 现状
稳定。10 单测绿。
