//! Memory 的「内部状态感知」面(决策日志 2026-06-01):AI 感知一切失败/内部事件 + 造物交互瞬态环 + append-only 渲染。

use super::*;

impl Memory {
    // --- 内部状态感知(AI 能感知一切失败/内部状态,见决策日志 2026-06-01)---

    /// AI 感知一条内部状态事件(LLM 失败 / 工具失败 / 持久化写失败等)。两路并行:
    /// ① 推入瞬态环 —— 由 `render_internal_state` 渲染成上下文**最末**的"内部状态"块(经常变动,
    ///    放最后才不破坏前面的稳定 prompt 缓存前缀);
    /// ② 落一条**带时间戳**的时间线节点(role=`internal`)→ 可被嵌入/检索(AI 所感知的一切都能被索引)。
    /// 瞬态自我感知(只入瞬态环,**不落时间线节点**)——用于高频、纯瞬时的自我感知
    /// (如每回合的检索动作 `mind_search`):下回合经 `render_internal_state` 可感知,但不让时间线/
    /// 上下文随之无界增长(否则 `assemble_context` 每回合自我 perceive 会破坏上下文稳定性)。
    pub fn perceive_transient(&mut self, kind: &str, message: impl Into<String>) {
        self.internal_events.push_back(InternalEvent {
            seq: self.internal_seq,
            at: growbox_core::now(),
            kind: kind.to_string(),
            message: message.into(),
        });
        self.internal_seq += 1;
        while self.internal_events.len() > self.transient_caps.internal_events_cap {
            self.internal_events.pop_front();
        }
    }

    /// 当前内部事件发号器值(= 下一个待分配 seq)。agent 循环进入时取作 append-only 游标起点。
    pub fn internal_seq(&self) -> u64 {
        self.internal_seq
    }

    /// 当前造物事件发号器值(= 下一个待分配 seq)。
    pub fn artifact_seq(&self) -> u64 {
        self.artifact_seq
    }

    pub fn perceive(&mut self, kind: &str, message: impl Into<String>) {
        let message = message.into();
        self.perceive_transient(kind, message.clone());
        // 同时进时间线 → 带时间戳、可检索(失败/事件也是 AI 感知到的信息,低频值得留痕)。
        self.ingest_with_role(format!("[内部状态·{kind}] {message}"), "internal");
    }

    /// 渲染"内部状态"块,**置于上下文最末**(特殊句式表明=系统自述、非用户输入)。
    /// 每条带时间戳并明示"按时间戳判先后" —— 检索会打乱位置顺序,AI 据时间戳重建时间序
    /// (决策日志 2026-06-01)。无事件返回 `None`。
    pub fn render_internal_state(&self, prompt_lang: &str) -> Option<String> {
        if self.internal_events.is_empty() {
            return None;
        }
        // 块头/行标按 prompt_lang(zh/en)渲染 —— 给英文模型时不掺中文说明噪音(感知告知双受众:对内随 prompt_lang)。
        let zh = prompt_lang.starts_with("zh");
        let (mut s, line_label, foot) = if zh {
            (
                String::from(
                    "~~~~~~~~~~ 内部状态 · INTERNAL STATE(系统自述,非用户输入)~~~~~~~~~~\n\
                     [本块说明] 以下是你此刻感知到的系统内部状态与最近发生的失败/事件。\n\
                     它随时变动,故永远置于上下文最末;请按每条「时间」字段判先后,不要按位置推断顺序。\n",
                ),
                "时间",
                "~~~~~~~~~~ 内部状态结束 ~~~~~~~~~~",
            )
        } else {
            (
                String::from(
                    "~~~~~~~~~~ INTERNAL STATE (self-reported by the system, not user input) ~~~~~~~~~~\n\
                     [Note] Below is the internal system state you currently perceive and recent failures/events.\n\
                     It changes over time and is always placed at the very end of context; judge order by each entry's timestamp, not by position.\n",
                ),
                "time",
                "~~~~~~~~~~ END INTERNAL STATE ~~~~~~~~~~",
            )
        };
        for e in &self.internal_events {
            // kind 用时按受控表恢复成可读标签(③ 受控 kind 表;未知自由 kind 原样透传)。
            let kind_label = crate::node_kind::label(&e.kind, prompt_lang);
            s.push_str(&format!("[{} {} | {}] {}\n", line_label, e.at.to_rfc3339(), kind_label, e.message));
        }
        s.push_str(foot);
        Some(s)
    }

    /// 当前瞬态内部事件数(观测/测试用)。
    pub fn internal_event_count(&self) -> usize {
        self.internal_events.len()
    }

    /// ★append-only 注入★:渲染 `seq >= since` 的**新**内部事件成一个轻量块,供 agent 循环
    /// **一次性追加进对话历史、永不重渲**(byte-stable prefix,命中 deepseek KV 缓存)。
    /// 返回 `(块文本, 新游标=下一个待分配 seq)`;无新事件返回 `None`。轻量头(不含 render_internal_state
    /// 的大段说明)—— 因为它只追加一次,不像旧"每轮夹末尾"会重复且破坏缓存(2026-06-04 实测 hit 640→128)。
    pub fn render_internal_since(&self, prompt_lang: &str, since: u64) -> Option<(String, u64)> {
        let zh = prompt_lang.starts_with("zh");
        let label = if zh { "时间" } else { "time" };
        let lines: Vec<String> = self
            .internal_events
            .iter()
            .filter(|e| e.seq >= since)
            .map(|e| {
                let kind_label = crate::node_kind::label(&e.kind, prompt_lang);
                format!("[{} {} | {}] {}", label, e.at.to_rfc3339(), kind_label, e.message)
            })
            .collect();
        if lines.is_empty() {
            return None;
        }
        let head = if zh {
            "~~~~~~~~~~ 内部状态更新 · INTERNAL STATE(系统自述,非用户输入;按时间戳判先后)~~~~~~~~~~"
        } else {
            "~~~~~~~~~~ INTERNAL STATE update (self-reported by the system, not user input; order by timestamp) ~~~~~~~~~~"
        };
        Some((format!("{head}\n{}", lines.join("\n")), self.internal_seq))
    }

    /// AI 感知一条**造物交互**(被造物 UI 的点击/输入回传)。仅入独立的造物瞬态环
    /// (不落时间线、不进聊天)——下回合经 `render_artifact_state` 让 AI 感知最近交互。
    /// 默认丢(满则淘汰最旧);值得永久记的结论由 AI 主动 `perceive`(Phase 4)。
    pub fn perceive_artifact(&mut self, canvas_id: &str, callback_id: &str, value: &str) {
        // ★上报量由 LLM 自己决定(用户原则 2026-06-04)★:不写死小上限;只设一个**宽松安全兜底**
        // (防 runaway 巨串撑爆内存/上下文),正常游戏态远低于此,LLM 想报多少全盘状态都放行。
        // ★自我感知原则★:真超兜底被截断时,**如实把"被截断 + 原因 + 怎么办"写进这条让 AI 感知**
        // (它据此精简/分块上报,而非默默收到残缺 JSON 看不清——真机 200 截断致落子全乱的教训)。
        const ARTIFACT_VALUE_CAP: usize = 16384;
        let original_len = value.chars().count();
        let value: String = if original_len > ARTIFACT_VALUE_CAP {
            let kept: String = value.chars().take(ARTIFACT_VALUE_CAP).collect();
            format!(
                "{kept}…[内部限制:本条造物回传 {original_len} 字超过上限 {ARTIFACT_VALUE_CAP} 已被截断;\
                 我只看到了前 {ARTIFACT_VALUE_CAP} 字。如需我看到完整状态,请让造物更紧凑地上报\
                 (如只报本步变化/紧凑编码,而非每次回传整盘原始数组)]"
            )
        } else {
            value.to_string()
        };
        self.artifact_interactions.push_back(InternalEvent {
            seq: self.artifact_seq,
            at: growbox_core::now(),
            kind: crate::node_kind::ARTIFACT.to_string(),
            message: format!("「{canvas_id}」{callback_id} = {value}"),
        });
        self.artifact_seq += 1;
        while self.artifact_interactions.len() > self.transient_caps.artifact_interactions_cap {
            self.artifact_interactions.pop_front();
        }
    }

    /// 渲染"造物交互"块(置于上下文尾,内部状态块之后)。无交互返回 None。
    /// 与内部状态块分开:这是被造物 UI 的近期交互流(AI 据此响应造物),不与系统内部状态混。
    pub fn render_artifact_state(&self, prompt_lang: &str) -> Option<String> {
        if self.artifact_interactions.is_empty() {
            return None;
        }
        let zh = prompt_lang.starts_with("zh");
        let (mut s, line_label, foot) = if zh {
            (
                String::from(
                    "~~~~~~~~~~ 造物交互 · ARTIFACT(你现造 UI 上的近期交互)~~~~~~~~~~\n\
                     [本块说明] 以下是用户在你现造的造物(沙箱 UI)上的最近交互(点击/输入)。\n\
                     按「时间」判先后;如需回应请更新造物(render_artifact)。这是瞬态交互流,不进聊天记录。\n",
                ),
                "时间",
                "~~~~~~~~~~ 造物交互结束 ~~~~~~~~~~",
            )
        } else {
            (
                String::from(
                    "~~~~~~~~~~ ARTIFACT INTERACTIONS (recent interactions on the UI you authored) ~~~~~~~~~~\n\
                     [Note] Below are the user's recent interactions (clicks/inputs) on the artifact (sandboxed UI) you authored.\n\
                     Judge order by each entry's timestamp; to respond, update the artifact (render_artifact). This is a transient stream, not in chat history.\n",
                ),
                "time",
                "~~~~~~~~~~ END ARTIFACT INTERACTIONS ~~~~~~~~~~",
            )
        };
        for e in &self.artifact_interactions {
            let kind_label = crate::node_kind::label(&e.kind, prompt_lang);
            s.push_str(&format!("[{} {} | {}] {}\n", line_label, e.at.to_rfc3339(), kind_label, e.message));
        }
        s.push_str(foot);
        Some(s)
    }

    /// 当前造物交互瞬态条数(观测/测试用)。
    pub fn artifact_interaction_count(&self) -> usize {
        self.artifact_interactions.len()
    }

    /// ★append-only 注入★:渲染 `seq >= since` 的**新**造物交互成一个轻量块(供 agent 循环一次性
    /// 追加进历史、永不重渲;byte-stable prefix)。返回 `(块文本, 新游标)`;无新交互返回 `None`。
    pub fn render_artifact_since(&self, prompt_lang: &str, since: u64) -> Option<(String, u64)> {
        let zh = prompt_lang.starts_with("zh");
        let label = if zh { "时间" } else { "time" };
        let lines: Vec<String> = self
            .artifact_interactions
            .iter()
            .filter(|e| e.seq >= since)
            .map(|e| {
                let kind_label = crate::node_kind::label(&e.kind, prompt_lang);
                format!("[{} {} | {}] {}", label, e.at.to_rfc3339(), kind_label, e.message)
            })
            .collect();
        if lines.is_empty() {
            return None;
        }
        let head = if zh {
            "~~~~~~~~~~ 造物交互 · ARTIFACT(你现造 UI 上的近期交互;如需回应请 render_artifact)~~~~~~~~~~"
        } else {
            "~~~~~~~~~~ ARTIFACT INTERACTIONS (on the UI you authored; to respond, render_artifact) ~~~~~~~~~~"
        };
        Some((format!("{head}\n{}", lines.join("\n")), self.artifact_seq))
    }

    /// AI 主动把一条**造物结论**永久记入(值得长留的里程碑:定下约定 / 解锁结局 / 替用户做的决定)。
    /// 与 `perceive_artifact`(默认丢的瞬态交互流)对立:这条同时入内部状态瞬态环 + **落时间线节点**
    /// (kind=artifact,可检索、可按 kind 筛"造物日志")——"默认丢 + 主动留"的"主动留"半。
    pub fn perceive_artifact_conclusion(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.perceive_transient(crate::node_kind::ARTIFACT, message.clone());
        self.ingest_with_role(format!("[造物结论] {message}"), crate::node_kind::ARTIFACT);
    }
}
