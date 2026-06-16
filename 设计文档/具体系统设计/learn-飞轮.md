# learn(growbox-learn)—— 飞轮 + 永久目标调度

## 职责
让 Agent "越用越强":把交互/操作的副产品收集起来,压缩成更精确的结论;按永久目标在 idle 时调度学习。只做提炼/调度,存取归 memory。

## 实际设计
- `flywheel.rs` —— **飞轮**:收集 → 聚类压缩(经验压成知识,旧版 `superseded_by` 标记,见 core 的结论模型)。idle 时跑(见 gui 的 supervisor + 后台任务 idle 学习闭环)。
- `scheduler.rs` —— **永久目标调度**:按目标安排学习/演练。
- 与 `设计/04-飞轮` 对应:结论=猜想要验证;不重复压缩(supersede 后下一轮跳过)。

## 用的库
依赖 core + memory;serde、async-trait。无第三方重型库。

## 关键文件
`crates/growbox-learn/src/{flywheel,scheduler,lib}.rs`

## 现状
11 单测绿。精确层飞轮的"做梦/睡眠/疲劳 + 潜意识仲裁器"(P5)尚未做——属精确层后续(见 `计划/precision-layer.md` 阶段5、记忆 `precision-layer-progress`)。
