# 01 — Agent 循环端到端(真机)

> 实验日期:2026-05-30。模型:`deepseek-v4-flash`,API `https://api.deepseek.com`(OpenAI 兼容)。
> 装配完 gui 后第一次用真模型跑通整条脊柱。测试:`crates/growbox-gui/tests/live_agent.rs`。
> **核心结论:整条 Agent 循环(组上下文→流式 LLM→工具→安全门→执行→回填自纠→学习)与真 flash 完全跑通,3 轮 5 秒达成多步任务。**

---

## 一句话给后续开发

**脊柱已被真机证明可用。** 给 flash 一个多步文件任务("建 note.txt 再读回确认"),它自动:输出 reasoning → 调 `file_write`(标准流式 tool_calls,参数完整)→ 收到结果继续 reasoning → 调 `file_read` → 给中文总结收尾。`实验记录/00` 的所有结论在装配后依然成立(reasoning 先到、工具参数不空、给足 token 即正常)。

---

## 记录 1:整条循环跑通,多步任务自洽

- **待验**:mock 单测过的 `agent_loop`,接真 flash + 真执行器 + 真安全门后是否端到端可用?
- **方法**:沙箱=临时目录(可写);消息"用 file_write 建 note.txt 写'GrowBox 工作正常。',再 file_read 读回确认,最后一句话告诉我结果"。`max_tokens=8192`。
- **观测**(实时事件流):
  1. 流式 `reasoning`:"用户要求我创建 note.txt…让我先看看当前项目目录的结构。"
  2. 工具调用 `file_write {"path":"note.txt","content":"GrowBox 工作正常。"}` → 安全门放行(在可写根内)→ 真落盘 23 字节。
  3. 结果回填,模型续 `reasoning`:"文件已创建成功,现在用 file_read 读回确认。"
  4. 工具调用 `file_read {"path":"note.txt"}` → 返回"GrowBox 工作正常。"
  5. 续 `reasoning` 后给 `content`:"已完成:成功创建 note.txt…读回确认内容一致。"
  6. `finish_reason=stop`,循环 `Completed`,共 **3 轮 / 5.05s**。
- **结论**:① reasoning 与 content/tool_calls 分离处理正确;② 流式 tool_calls 按 index 拼参,参数完整不空;③ 唯一安全门 + 唯一分发路径工作正常;④ 工具结果回填后模型能自驱进入下一步(多步任务无需人插手);⑤ 每步 `flywheel.collect` 采集到经验。

## 记录 2:本地词法嵌入足够支撑第一层检索

- **背景**:DeepSeek 无嵌入端点,`LlmBridge::embed` 用本地散列词法向量(256 维,L2 归一)。
- **观测**:空记忆下检索不报错,不误命中;循环正常注入"无相关记忆"。
- **结论**:第一层 RAG 用本地向量可行(便宜、离线、确定)。命中质量待记忆攒起来后再评估;不够再下沉精确层(judge 走真 LLM)。

## 残留(不挡用,见 `交接报告.md` §5)

- 真机只验了 happy path(可写区内多步文件任务)。**待补真机**:越界写触发授权弹窗的前端闭环、shell 工具、create_project 的 ui-intent 闭环、截断重试在真模型上的触发(本次 8192 够用未触发)。
- 记忆持久化、飞轮 idle `turn`、v1 面板接厚仍是 TODO。
