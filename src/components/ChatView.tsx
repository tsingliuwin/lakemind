import { For, Show, createSignal, createEffect, onMount, onCleanup } from "solid-js";
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
  availableModels: string[];
  selectedModel: string;
  onSelectModel: (model: string) => void;
  selectedPriority: string;
  onSelectPriority: (priority: string) => void;
}) {
  const [modelDropdownOpen, setModelDropdownOpen] = createSignal(false);
  const [priorityDropdownOpen, setPriorityDropdownOpen] = createSignal(false);
  let modelRef: HTMLDivElement | undefined;
  let priorityRef: HTMLDivElement | undefined;

  const handleClickOutside = (e: MouseEvent) => {
    if (modelRef && !modelRef.contains(e.target as Node)) {
      setModelDropdownOpen(false);
    }
    if (priorityRef && !priorityRef.contains(e.target as Node)) {
      setPriorityDropdownOpen(false);
    }
  };

  onMount(() => {
    document.addEventListener("mousedown", handleClickOutside);
    onCleanup(() => {
      document.removeEventListener("mousedown", handleClickOutside);
    });
  });
  const [input, setInput] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  let scrollEl: HTMLDivElement | undefined;

  // 新消息到达或状态变为 busy 时滚到底部。
  createEffect(() => {
    props.messages;
    busy();
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
                  <Show when={msg.reasoning}>
                    <details class="chat-msg__reasoning" style="margin-bottom: 6px; border-left: 2px solid var(--accent-blue, #50a0ff); padding-left: 8px; opacity: 0.75;">
                      <summary style="cursor: pointer; font-size: 12px; color: var(--text-dim); user-select: none;">💭 思考过程</summary>
                      <div style="white-space: pre-wrap; margin-top: 4px; font-size: 12px; color: var(--text-dim);">{msg.reasoning}</div>
                    </details>
                  </Show>
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
          placeholder={`向 LakeMind 提问（Enter 发送 · Shift+Enter 换行）…`}
          value={input()}
          onInput={(e) => setInput(e.currentTarget.value)}
          onkeydown={onKeydown}
          disabled={busy()}
          rows={2}
        />
        <div class="chat-composer__toolbar">
          <div style="display: flex; align-items: center; gap: 16px;">
            <span class="chat-composer__ws" title="当前工作区">📂 {props.workspace}</span>
            
            {/* Model Selector Dropdown */}
            <div class="dropdown-wrapper" ref={modelRef} style="position: relative;">
              <button
                class="pill-btn select-btn"
                style="background: transparent; border: none; padding: 2px 6px; font-size: 12px; display: flex; align-items: center; gap: 4px; color: var(--text-normal); cursor: pointer; border-radius: 4px;"
                onClick={() => setModelDropdownOpen(!modelDropdownOpen())}
              >
                <span class="model-status-dot" classList={{ active: props.availableModels.length > 0 }} />
                <span>{props.selectedModel || "选择模型"}</span>
                <span class="btn-caret">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 8px; height: 8px;">
                    <polyline points="6 9 12 15 18 9"></polyline>
                  </svg>
                </span>
              </button>
              <Show when={modelDropdownOpen()}>
                <div class="custom-dropdown-list" style="bottom: calc(100% + 6px); left: 0; right: auto;">
                  <Show
                    when={props.availableModels.length > 0}
                    fallback={
                      <div class="dropdown-item muted" style="font-size: 11px; pointer-events: none; padding: 6px 12px;">
                        无可用模型
                      </div>
                    }
                  >
                    <For each={props.availableModels}>
                      {(model) => (
                        <button class="dropdown-item" onClick={() => { props.onSelectModel(model); setModelDropdownOpen(false); }}>
                          {model}
                        </button>
                      )}
                    </For>
                  </Show>
                </div>
              </Show>
            </div>

            {/* Priority Selector Dropdown */}
            <div class="dropdown-wrapper" ref={priorityRef} style="position: relative;">
              <button
                class="pill-btn select-btn"
                style="background: transparent; border: none; padding: 2px 6px; font-size: 12px; display: flex; align-items: center; gap: 4px; color: var(--text-normal); cursor: pointer; border-radius: 4px;"
                onClick={() => setPriorityDropdownOpen(!priorityDropdownOpen())}
              >
                <span>⚙️</span>
                <span>{props.selectedPriority}</span>
                <span class="btn-caret">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 8px; height: 8px;">
                    <polyline points="6 9 12 15 18 9"></polyline>
                  </svg>
                </span>
              </button>
              <Show when={priorityDropdownOpen()}>
                <div class="custom-dropdown-list" style="bottom: calc(100% + 6px); left: 0; right: auto;">
                  <button class="dropdown-item" onClick={() => { props.onSelectPriority("最高"); setPriorityDropdownOpen(false); }}>最高</button>
                  <button class="dropdown-item" onClick={() => { props.onSelectPriority("均衡"); setPriorityDropdownOpen(false); }}>均衡</button>
                  <button class="dropdown-item" onClick={() => { props.onSelectPriority("最快"); setPriorityDropdownOpen(false); }}>最快</button>
                </div>
              </Show>
            </div>
          </div>
          
          <button class="chat-composer__send" disabled={busy() || !input().trim()} onClick={() => void send()}>
            {busy() ? "运行中…" : "发送 ↑"}
          </button>
        </div>
      </div>
    </div>
  );
}
