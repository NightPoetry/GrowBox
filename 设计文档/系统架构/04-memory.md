# 04 — memory

## 职责
只管**存与取**:对话/经验/结论的存储,分层检索(RAG → 精确层);不管提炼/因果(归 learn)。

## 接口
```rust
pub struct Memory { /* 存储后端可换:内存版(测试)/ 文件版(运行) */ }
impl Memory {
    pub async fn retrieve(&self, q: &Query, llm: &LlmRouter) -> Hits;  // 分层下沉
    pub fn ingest(&mut self, item: Ingestable);                       // 对话/经验/结论
    pub fn maintain(&mut self, llm: &LlmRouter);                      // 做梦/睡眠(idle)
}
```

## 依赖
→ 依赖:core、llm。 ← 被依赖:app、learn(取经验原料)。

## 数据流
```
Query
 → 第一层 RAG(向量近似) ── 够 + 反馈好 → 停
 → 下沉(没找到/反馈差/引用历史)
 → 第二层精确:缓存命中? → 否则时间线 LLM 读原文逐段扫(染色跳深绿区)
 → Hits
idle: maintain() → 扫碎片(做梦) + 推演预索引(睡眠)
```

## 接原理
- `设计/02` 原则1(分层下沉):retrieve 两层。
- `设计/02` 原则2(精确层飞轮):指针/染色/缓存/二级索引/碎片,沉淀扫描副产品。

## 已知坑
- 旧 retrieval ~1万行没验证过、老出错(V3 之因)→ 本次按 `设计/02` 原理从头写 + 测试兜底,**不移植旧实现**。
- 旧文档"不做向量"漏了 RAG 层 → 本次明确两层。
- 种子数据旧版全是系统内部元知识,导致 recall 返回 0 → 本次种子=用户真会用到的知识。
- **当前 `embed` 是词法散列占位**(只抓字面词重叠,无语义)→ RAG 实质是模糊关键词匹配。要做真语义 RAG 必须换真 embedding 模型:本地内嵌小模型(candle)默认 + 可选远程 OpenAI 兼容槽位(base/key/model)。详见 `计划/embedding-service.md`。换模型 = 向量空间变,旧向量全失效要重嵌。
- **指针网/时间线当前全驻内存,不可扩展**:持久 agent 记忆无界,终态应磁盘原生——指针边按 source 落 redb(一次按键读取一个邻域),从当前位置往外导航只 page-in 走过的邻域,三级缓存做 RAM 工作集。详见 `计划/precision-layer.md` 的存储讨论。
