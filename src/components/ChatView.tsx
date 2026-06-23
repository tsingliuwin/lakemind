import { For, Show, Switch, Match, createSignal, createEffect, onMount, onCleanup } from "solid-js";
import type { ChatMessage, Segment } from "../lib/types";
import ToolSegment from "./ToolSegment";
import MarkdownRenderer from "./MarkdownRenderer";

type ReasoningSeg = Extract<Segment, { type: "reasoning" }>;
type TextSeg = Extract<Segment, { type: "text" }>;
const asReasoning = (s: Segment): ReasoningSeg | null => (s.type === "reasoning" ? s : null);
const asText = (s: Segment): TextSeg | null => (s.type === "text" ? s : null);

/**
 * 对话模式主区：消息流（上）+ 段内嵌 + 底部常驻输入框。
 *
 * 消息按 segment 顺序渲染：reasoning（折叠）→ tool（混合折叠）→ text（Markdown）。
 * 进度指示为单行「⏱ 已工作 N 秒 · 正在执行 SQL…」，由当前 running tool 派生。
 */

const TOOL_LABELS: Record<string, string> = {
  list_tables: "探索数据库",
  describe_table: "分析表结构",
  execute_query: "执行 SQL",
  sample_data: "采样数据",
};

export default function ChatView(props: {
  messages: ChatMessage[];
  workspace: string;
  taskName: string;
  onSend: (prompt: string) => void;
  onOpenInSqlPanel: (sql: string) => void;
  onDelete?: () => void;
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

  // Reasoning fold state: latest assistant message open while streaming.
  const [openReasoningIds, setOpenReasoningIds] = createSignal<Set<string>>(new Set());
  // Tool segment fold state: a tool segment is auto-expanded while running,
  // auto-collapsed when its result arrives; user can toggle manually.
  const [expandedToolIds, setExpandedToolIds] = createSignal<Set<string>>(new Set());

  function toggleReasoning(msgId: string) {
    setOpenReasoningIds((prev) => {
      const next = new Set(prev);
      if (next.has(msgId)) next.delete(msgId);
      else next.add(msgId);
      return next;
    });
  }

  function toggleTool(segId: string) {
    setExpandedToolIds((prev) => {
      const next = new Set(prev);
      if (next.has(segId)) next.delete(segId);
      else next.add(segId);
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

  // The latest assistant message id (streaming target).
  function lastAssistantId(): string | undefined {
    const msgs = props.messages;
    if (msgs.length === 0) return undefined;
    const last = msgs[msgs.length - 1];
    return last.role === "assistant" ? last.id : undefined;
  }

  // Auto-expand the latest reasoning segment of the streaming assistant
  // message (there can be several, interleaved between tool calls). Each new
  // reasoning run auto-opens as it starts receiving deltas.
  createEffect(() => {
    const id = lastAssistantId();
    if (!id || !busy()) return;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return;
    for (let i = msg.segments.length - 1; i >= 0; i--) {
      const s = msg.segments[i];
      if (s.type === "reasoning") {
        setOpenReasoningIds((prev) => {
          const next = new Set(prev);
          next.add(s.id);
          return next;
        });
        break;
      }
    }
  });

  // Drive tool-segment auto-expand/collapse: any tool segment whose status is
  // "running" is expanded; once it transitions to ok|error it is removed from
  // the expanded set (collapses to one line). Latest tool thus stays expanded
  // while running, history collapses.
  createEffect(() => {
    const id = lastAssistantId();
    if (!id) return;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return;
    const running = msg.segments
      .filter((s): s is Extract<Segment, { type: "tool" }> => s.type === "tool")
      .filter((s) => s.status === "running")
      .map((s) => s.id);
    setExpandedToolIds((prev) => {
      const next = new Set(prev);
      for (const r of running) next.add(r);
      // Drop completed tools that the user hasn't manually toggled open.
      for (const s of msg.segments) {
        if (s.type === "tool" && s.status !== "running" && !running.includes(s.id)) {
          next.delete(s.id);
        }
      }
      return next;
    });
  });

  // Current single-line action label derived from the running tool (or "思考中…").
  function currentAction(): string | undefined {
    const id = lastAssistantId();
    if (!id) return undefined;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return undefined;
    const tools = msg.segments.filter((s): s is Extract<Segment, { type: "tool" }> => s.type === "tool");
    const running = tools.filter((s) => s.status === "running");
    if (running.length > 0) {
      const last = running[running.length - 1];
      return `正在${TOOL_LABELS[last.tool] ?? "执行工具"}…`;
    }
    return "思考中…";
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
      <div class="chat-header">
        <span class="chat-header__title">{props.taskName || "对话"}</span>
        <button
          class="icon-btn"
          title="关闭并删除对话"
          onClick={() => {
            if (props.messages.length > 0) {
              if (!window.confirm("删除这个对话？历史记录将一并清除且不可恢复。")) return;
            }
            props.onDelete?.();
          }}
          style="color: var(--accent-red);"
        >
          ✕
        </button>
      </div>
      <div class="chat-stream" ref={scrollEl}>
        <Show
          when={props.messages.length > 0}
          fallback={<div class="chat-empty">向 LakeMind 提问，开始探索你的数据。</div>}
        >
          <For each={props.messages}>
            {(msg) => (
              <div class={`chat-msg chat-msg--${msg.role}`}>
                <div class="chat-msg__body">
                  {/* Single ordered loop: preserves the real reasoning → tool →
                      … → text transcript instead of grouping by type. */}
                  <For each={msg.segments}>
                    {(seg) => {
                      const rs = () => asReasoning(seg);
                      const ts = () => asText(seg);
                      return (
                        <Switch>
                          <Match when={seg.type === "reasoning"}>
                            <div class="chat-reasoning">
                              <div class="chat-reasoning__header" onClick={() => toggleReasoning(seg.id)}>
                                <span class="chat-reasoning__icon">💭</span>
                                <span class="chat-reasoning__label">思考过程</span>
                                <span class="chat-reasoning__toggle">
                                  {openReasoningIds().has(seg.id) ? "▾" : "▸"}
                                </span>
                              </div>
                              <Show when={openReasoningIds().has(seg.id) && rs()}>
                                <div class="chat-reasoning__body">{rs()!.text}</div>
                              </Show>
                            </div>
                          </Match>
                          <Match when={seg.type === "tool"}>
                            <ToolSegment
                              seg={seg}
                              expanded={expandedToolIds().has(seg.id)}
                              onToggle={toggleTool}
                              onOpenInSqlPanel={props.onOpenInSqlPanel}
                            />
                          </Match>
                          <Match when={seg.type === "text" && ts()}>
                            <div class="chat-msg__text">
                              <Show
                                when={msg.role === "assistant"}
                                fallback={ts()!.text}
                              >
                                <MarkdownRenderer content={ts()!.text} />
                              </Show>
                            </div>
                          </Match>
                        </Switch>
                      );
                    }}
                  </For>
                </div>
              </div>
            )}
          </For>

          {/* Busy / streaming indicator — single-line status */}
          <Show when={busy()}>
            <div class="chat-msg chat-msg--assistant">
              <div class="chat-msg__body">
                <div class="chat-agent-status">
                  <span class="agent-status__timer">⏱ 已工作 {elapsedSec()} 秒</span>
                  <Show when={currentAction()}>
                    <span class="agent-status__phase">{currentAction()}</span>
                  </Show>
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
