// =============================================================================
// MOCK —— 对话模式的假 Agent 响应数据。
// M2 接真 Agent（Rig/LLM 工具循环）时，整个文件应被真实的流式响应替换。
// 现在它只为了让 ChatView 的布局与交互可被体验，不调用任何后端。
// =============================================================================

import type { ChatMessage, Segment } from "./types";

let mockSeq = 0;
function segId(prefix: string): string {
  return `mock-${prefix}-${++mockSeq}`;
}

/**
 * 根据用户输入，返回一组模拟的 Agent 产物段（reasoning → tool → tool → text）。
 * 返回一个 Promise 以便 ChatView 用 await 等待，未来直接换成真 fetch。
 */
export function mockAgentReply(userPrompt: string): Promise<ChatMessage> {
  const prompt = userPrompt.trim() || "帮我看看数据";

  const segments: Segment[] = [
    {
      type: "reasoning",
      id: segId("r"),
      text: `用户问「${prompt}」，先看看库里有哪些表，再做聚合。`,
    },
    {
      type: "tool",
      id: segId("tool"),
      tool: "list_tables",
      status: "ok",
      summary: "探测到 3 张表",
      elapsedMs: 120,
    },
    {
      type: "tool",
      id: segId("tool"),
      tool: "execute_query",
      args: {
        sql: "SELECT month_id, SUM(amount) AS total\nFROM s_sales\nWHERE year_id = 2024\nGROUP BY month_id\nORDER BY month_id;",
      },
      status: "ok",
      summary: "返回 12 行（2 列）",
      sql: "SELECT month_id, SUM(amount) AS total\nFROM s_sales\nWHERE year_id = 2024\nGROUP BY month_id\nORDER BY month_id;",
      table: {
        columns: ["month_id", "total"],
        columnTypes: ["INTEGER", "DOUBLE"],
        rows: [
          [1, 102400.5],
          [2, 98700.0],
          [3, 121000.25],
          [4, 110500.0],
          [5, 132000.75],
          [6, 99000.0],
          [7, 145000.0],
          [8, 118000.5],
          [9, 161200.0],
          [10, 124000.0],
          [11, 138500.5],
          [12, 152000.0],
        ],
        rowCount: 12,
        truncated: false,
        elapsedMs: 240,
      },
      elapsedMs: 240,
    },
    {
      type: "text",
      id: segId("txt"),
      text: `已围绕「${prompt}」完成探索。**Q3 销售环比 +18.4%**，主要由「华南」品类拉动。建议进一步下钻 region × category 维度。可点上方 SQL 在 SQL 面板里手动调整后重跑。`,
    },
  ];

  return new Promise((resolve) => {
    // 模拟 Agent 思考 + 执行的延迟（M2 替换为真流式）。
    window.setTimeout(() => {
      resolve({
        id: `mock-msg-${Date.now()}`,
        role: "assistant",
        segments,
        ts: Date.now(),
      });
    }, 900);
  });
}
