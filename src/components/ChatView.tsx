import { For, Show, createSignal, createEffect, onMount, onCleanup } from "solid-js";
import type { ChatMessage } from "../lib/types";
import ChatCard from "./ChatCard";
import MarkdownRenderer from "./MarkdownRenderer";

/**
 * 对话模式主区：消息流（上）+ 卡片内嵌 + 底部常驻输入框。
 *
 * 优化特性：
 * - Markdown 渲染（标题、代码块、表格、行内代码）
 * - Agent 四步进度条（探索 → 理解 → 查询 → 总结）
 * - 思考过程可展开/折叠
 * - 工作时长计时器
 */

const PHASE_LABELS: Record<string, string> = {
  exploring: "探索数据库",
  analyzing: "分析表结构",
  querying: "执行查询",
  concluding: "生成结论",
};

const PHASE_SHORT_LABELS: Record<string, string> = {
  exploring: "探索",
  analyzing: "分析",
  querying: "查询",
  concluding: "结论",
};

const PHASE_ORDER = ["exploring", "analyzing", "querying", "concluding"];

function phaseIndex(phase: string | undefined): number {
  if (!phase) return -1;
  return PHASE_ORDER.indexOf(phase);
}

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

  // Reasoning fold state: track which messages have their reasoning open.
  // Default: latest assistant message is open, older ones are closed.
  const [openReasoningIds, setOpenReasoningIds] = createSignal<Set<string>>(new Set());

  function toggleReasoning(msgId: string) {
    setOpenReasoningIds((prev) => {
      const next = new Set(prev);
      if (next.has(msgId)) next.delete(msgId);
      else next.add(msgId);
      return next;
    });
  }

  // Elapsed timer for busy state
  const [elapsedSec, setElapsedSec] = createSignal(0);

  createEffect(() => {
    if (busy()) {
      const start = Date.now();
      const handle = setInterval(() => {
        setElapsedSec(Math.floor((Date.now() - start) / 1000));
      }, 1000);
      onCleanup(() => clearInterval(handle));
    } else {
      setElapsedSec(0);
    }
  });

  let scrollEl: HTMLDivElement | undefined;

  // 新消息到达或状态变为 busy 时滚到底部。
  createEffect(() => {
    props.messages;
    busy();
    if (scrollEl) {
      scrollEl.scrollTop = scrollEl.scrollHeight;
    }
  });

  // Auto-open reasoning for the latest streaming message
  createEffect(() => {
    const msgs = props.messages;
    if (msgs.length > 0) {
      const last = msgs[msgs.length - 1];
      if (last.role === "assistant" && last.reasoning && busy()) {
        setOpenReasoningIds((prev) => {
          const next = new Set(prev);
          next.add(last.id);
          return next;
        });
      }
    }
  });

  // Get the current phase from the last streaming assistant message
  function currentPhase(): string | undefined {
    const msgs = props.messages;
    if (msgs.length === 0) return undefined;
    const last = msgs[msgs.length - 1];
    if (last.role === "assistant") return last.phase;
    return undefined;
  }

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
                  {/* Reasoning — collapsible with blue left border */}
                  <Show when={msg.reasoning}>
                    <div class="chat-reasoning">
                      <div class="chat-reasoning__header" onClick={() => toggleReasoning(msg.id)}>
                        <span class="chat-reasoning__icon">💭</span>
                        <span class="chat-reasoning__label">思考过程</span>
                        <span class="chat-reasoning__toggle">
                          {openReasoningIds().has(msg.id) ? "▾" : "▸"}
                        </span>
                      </div>
                      <Show when={openReasoningIds().has(msg.id)}>
                        <div class="chat-reasoning__body">{msg.reasoning}</div>
                      </Show>
                    </div>
                  </Show>

                  {/* Message content — Markdown for assistant, plain for user */}
                  <div class="chat-msg__text">
                    <Show
                      when={msg.role === "assistant"}
                      fallback={msg.content}
                    >
                      <MarkdownRenderer content={msg.content} />
                    </Show>
                  </div>

                  {/* Chat cards */}
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

          {/* Busy / streaming indicator with phase progress bar */}
          <Show when={busy()}>
            <div class="chat-msg chat-msg--assistant">
              <div class="chat-msg__avatar">🤖</div>
              <div class="chat-msg__body">
                {/* Work duration — inspired by screenshot "已工作 46 秒 >" */}
                <div class="chat-agent-status">
                  <span class="agent-status__timer">
                    ⏱ 已工作 {elapsedSec()} 秒
                  </span>
                  <Show when={currentPhase()}>
                    <span class="agent-status__phase">
                      {PHASE_LABELS[currentPhase()!] ?? currentPhase()}
                    </span>
                  </Show>
                </div>

                {/* Four-step progress bar */}
                <div class="chat-phase-bar">
                  <For each={PHASE_ORDER}>
                    {(step, i) => {
                      const idx = () => phaseIndex(currentPhase());
                      const stepIdx = i();
                      const isActive = () => idx() === stepIdx;
                      const isDone = () => idx() > stepIdx;
                      return (
                        <>
                          <Show when={stepIdx > 0}>
                            <div class="phase-connector" classList={{ done: isDone() || isActive() }} />
                          </Show>
                          <div class="phase-step" classList={{ active: isActive(), done: isDone() }}>
                            <span class="phase-dot" />
                            <span>{PHASE_SHORT_LABELS[step] || step}</span>
                          </div>
                        </>
                      );
                    }}
                  </For>
                </div>
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
