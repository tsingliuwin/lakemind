// =============================================================================
// MOCK —— 对话模式的假 Agent 响应数据。
// M2 接真 Agent（Rig/LLM 工具循环）时，整个文件应被真实的流式响应替换。
// 现在它只为了让 ChatView 的布局与交互可被体验，不调用任何后端。
// =============================================================================

import type { ChatCard, ChatMessage } from "./types";

let mockCardSeq = 0;
function cardId(): string {
  return `mock-card-${++mockCardSeq}`;
}

/**
 * 根据用户输入，返回一组模拟的 Agent 产物（步骤卡 + SQL 卡 + 结论）。
 * 返回一个 Promise 以便 ChatView 用 await 等待，未来直接换成真 fetch。
 */
export function mockAgentReply(userPrompt: string): Promise<ChatMessage> {
  const prompt = userPrompt.trim() || "帮我看看数据";

  return new Promise((resolve) => {
    // 模拟 Agent 思考 + 执行的延迟（M2 替换为真流式）。
    window.setTimeout(() => {
      const cards: ChatCard[] = [
        {
          id: cardId(),
          kind: "step",
          title: "探测了 3 张表",
          detail: "s_sales(1,054 万行) · s_users(8,231 行) · s_regions(31 行)",
        },
        {
          id: cardId(),
          kind: "sql",
          title: "执行聚合查询",
          sql: `SELECT month_id, SUM(amount) AS total\nFROM s_sales\nWHERE year_id = 2024\nGROUP BY month_id\nORDER BY month_id;`,
          rows: 12,
        },
        {
          id: cardId(),
          kind: "conclusion",
          title: "分析结论",
          detail: `针对「${prompt}」：基于已注册的 SOURCE 表，Q3 销售环比 +18.4%，主要由「华南」品类拉动。建议进一步下钻 region × category 维度。`,
        },
      ];

      resolve({
        id: `mock-msg-${Date.now()}`,
        role: "assistant",
        content: `已围绕「${prompt}」完成探索，结论见下方卡片。你可以点 SQL 卡片在 SQL 面板里手动调整后重跑。`,
        cards,
        ts: Date.now(),
      });
    }, 900);
  });
}
