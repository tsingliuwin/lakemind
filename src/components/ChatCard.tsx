import { Show } from "solid-js";
import type { ChatCard as ChatCardData } from "../lib/types";

/**
 * Agent 产物卡片，嵌在助手消息气泡内。四种 kind 各有不同样式：
 * - step：步骤摘要（探测/发现），轻量信息卡。
 * - sql：SQL 代码卡，可「在 SQL 面板打开」→ 新建 SQL task。
 * - table：结果表卡（行数提示）。
 * - conclusion：最终结论卡，最适合「钉到画布」（M3）。
 *
 * 「在 SQL 面板打开」是双模式并存的关键桥梁：把 Agent 的 SQL 注入一个新 SQL
 * task，原对话历史不丢，用户可在两套上下文间自由穿梭。
 */
export default function ChatCard(props: {
  card: ChatCardData;
  onOpenInSqlPanel: (sql: string) => void;
}) {
  return (
    <div class={`chat-card chat-card--${props.card.kind}`}>
      <div class="chat-card__head">
        <span class="chat-card__icon" data-kind={props.card.kind}>
          {iconFor(props.card.kind)}
        </span>
        <span class="chat-card__title">{props.card.title}</span>
        {/* M3 占位：钉到画布。本阶段灰按钮不可点。 */}
        <button
          class="chat-card__pin"
          disabled
          title="钉到画布（M3 上线）"
        >
          📌
        </button>
      </div>

      <Show when={props.card.kind === "sql" && props.card.sql}>
        <div style="position: relative;">
          <pre class="chat-card__code">{props.card.sql}</pre>
          <button
            class="chat-card__copy-btn"
            onClick={async (e) => {
              e.stopPropagation();
              try {
                await navigator.clipboard.writeText(props.card.sql!);
                const btn = e.currentTarget;
                const oldText = btn.innerText;
                btn.innerText = "✓ 已复制";
                setTimeout(() => btn.innerText = oldText, 2000);
              } catch {}
            }}
          >
            复制
          </button>
        </div>
      </Show>

      <Show when={props.card.detail && props.card.kind !== "sql"}>
        <div class="chat-card__detail">{props.card.detail}</div>
      </Show>

      <Show when={props.card.kind === "table" && props.card.rows != null}>
        <div class="chat-card__rows">{props.card.rows!.toLocaleString()} 行</div>
      </Show>

      <Show when={props.card.sql}>
        <div class="chat-card__actions">
          <button
            class="chat-card__open"
            onClick={() => props.onOpenInSqlPanel(props.card.sql!)}
          >
            ▶ 在 SQL 面板打开
          </button>
        </div>
      </Show>
    </div>
  );
}

function iconFor(kind: ChatCardData["kind"]): string {
  switch (kind) {
    case "step":
      return "📋";
    case "sql":
      return "📊";
    case "table":
      return "▦";
    case "conclusion":
      return "✓";
    default:
      return "•";
  }
}
