import { For, Show, createSignal, createEffect } from "solid-js";
import type { ChatMessage } from "../lib/types";
import ChatCard from "./ChatCard";

/**
 * 对话模式主区：消息流（上）+ 卡片内嵌 + 底部常驻输入框。
 *
 * 布局逻辑：
 * - 消息流占主体高度，可滚动；用户气泡右对齐，助手左对齐。
 * - 助手消息内可携带多张 ChatCard（步骤/SQL/结论）。
 * - 底部输入区常驻，复用 HomePanel 的输入胶囊风格。
 *
 * 数据流：发送消息 → 追加 user 消息 → await mockAgentReply → 追加 assistant 消息。
 * M2 接真 Agent 时，只替换 mockAgentReply 为真实的流式 LLM 调用。
 */
export default function ChatView(props: {
  messages: ChatMessage[];
  workspace: string;
  onSend: (prompt: string) => void;
  onOpenInSqlPanel: (sql: string) => void;
}) {
  const [input, setInput] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  let scrollEl: HTMLDivElement | undefined;

  // 新消息到达时滚到底部。
  createEffect(() => {
    props.messages;
    if (scrollEl) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
    }
  });

  async function send() {
    const text = input().trim();
    if (!text || busy()) return;
    setInput("");
    setBusy(true);
    try {
      await props.onSend(text);
    } finally {
      setBusy(false);
    }
  }

  function onKeydown(e: KeyboardEvent) {
    // Enter 发送，Shift+Enter 换行。
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void send();
    }
  }

  return (
    <div class="chat-view">
      <div class="chat-stream" ref={scrollEl}>
        <Show
          when={props.messages.length > 0}
          fallback={<div class="chat-empty">向 LakeMind 提问，开始探索你的数据。</div>}
        >
          <For each={props.messages}>
            {(msg) => (
              <div class={`chat-msg chat-msg--${msg.role}`}>
                <div class="chat-msg__avatar">{msg.role === "user" ? "👤" : "🤖"}</div>
                <div class="chat-msg__body">
                  <div class="chat-msg__text">{msg.content}</div>
                  <Show when={msg.cards && msg.cards.length > 0}>
                    <div class="chat-msg__cards">
                      <For each={msg.cards}>{(card) => (
                        <ChatCard card={card} onOpenInSqlPanel={props.onOpenInSqlPanel} />
                      )}</For>
                    </div>
                  </Show>
                </div>
              </div>
            )}
          </For>
          <Show when={busy()}>
            <div class="chat-msg chat-msg--assistant">
              <div class="chat-msg__avatar">🤖</div>
              <div class="chat-msg__body">
                <div class="chat-msg__typing">思考中…</div>
              </div>
            </div>
          </Show>
        </Show>
      </div>

      <div class="chat-composer">
        <textarea
          class="chat-composer__input"
          placeholder={`向 LakeMind 提问（@ 数据源  / 命令  # 关联历史）…`}
          value={input()}
          onInput={(e) => setInput(e.currentTarget.value)}
          onkeydown={onKeydown}
          disabled={busy()}
          rows={2}
        />
        <div class="chat-composer__toolbar">
          <span class="chat-composer__ws" title="当前工作区">📂 {props.workspace}</span>
          <span class="muted">Enter 发送 · Shift+Enter 换行</span>
          <button class="chat-composer__send" disabled={busy() || !input().trim()} onClick={() => void send()}>
            {busy() ? "运行中…" : "发送 ↑"}
          </button>
        </div>
      </div>
    </div>
  );
}
