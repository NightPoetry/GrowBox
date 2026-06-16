// 用户裁决回投(决定脊柱的前端半):一次重试 + 失败必告知(双受众)。
//
// 后端 request_decision 阻塞等 ack、超时按拒绝(安全侧)。所以丢失 ack 的后果是
// "用户点了允许,却被当成拒绝,且双方都不知道"——这违背交互层原则(裁决是用户
// 不可代劳的那一下,绝不能静默丢弃)。这里统一:失败短暂退避重试一次;仍失败则
// notify("decision.ack_failed"):对外 toast 告知用户、对内 perceive 让 AI 知道
// 这是通信失败而非用户拒绝。弹窗不等待投递结果(UI 即点即关,重试在后台进行)。

import { api } from "./tauri-api";
import { notify } from "./notices";

export function ackDecision(id: string, decision: string): void {
  void (async () => {
    try {
      await api.decisionAck(id, decision);
    } catch {
      await new Promise((r) => setTimeout(r, 300));
      try {
        await api.decisionAck(id, decision);
      } catch (err) {
        console.error("[decision] ack 两次投递失败:", err);
        notify("decision.ack_failed", { decision });
      }
    }
  })();
}
