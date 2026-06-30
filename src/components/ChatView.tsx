import { For, Index, Show, Switch, Match, createSignal, createEffect, createMemo, onMount, onCleanup, untrack } from "solid-js";
import type { ChatMessage, Segment, TokenUsage } from "../lib/types";
import { derivePanelMetrics, fmtCap, fmtPct } from "../lib/metrics";
import ToolSegment from "./ToolSegment";
import ChartSegment from "./ChartSegment";
import MarkdownRenderer from "./MarkdownRenderer";

type ReasoningSeg = Extract<Segment, { type: "reasoning" }>;
type TextSeg = Extract<Segment, { type: "text" }>;
type ErrorSeg = Extract<Segment, { type: "error" }>;
const asReasoning = (s: Segment): ReasoningSeg | null => (s.type === "reasoning" ? s : null);
const asText = (s: Segment): TextSeg | null => (s.type === "text" ? s : null);
const asError = (s: Segment): ErrorSeg | null => (s.type === "error" ? s : null);

/**
 * 对话模式主区：消息流（上）+ 段内嵌 + 底部常驻输入框。
 *
 * 消息按 segment 顺序渲染：reasoning（折叠）→ tool（混合折叠）→ text（Markdown）。
 * 底部进度指示为单行「⏱ 已工作 N 秒」。
 */

export default function ChatView(props: {
  taskId: string;
  messages: ChatMessage[];
  workspace: string;
  taskName: string;
  onSend: (prompt: string) => void;
  /** Abort the running stream (stop button). */
  onStop?: () => void;
  /** Token usage from the last LLM response (for context window display). */
  tokenUsage?: TokenUsage | null;
  /** Current model's context window size (from settings.json). */
  contextWindow?: number;
  onOpenInSqlPanel: (sql: string) => void;
  onDelete?: () => void;
  availableModels: string[];
  selectedModel: string;
  onSelectModel: (model: string) => void;
  selectedPriority: string;
  onSelectPriority: (priority: string) => void;
  selectedConfirm: string;
  onSelectConfirm: (mode: string) => void;
  /** 用户对 awaiting 状态的 DDL 工具做出确认/取消决定。 */
  onConfirmTool: (toolCallId: string, approved: boolean) => void;
  /** 该对话是否正在流式输出（由父级 streamingTaskId 派生）。 */
  streaming: boolean;
}) {
  const [modelDropdownOpen, setModelDropdownOpen] = createSignal(false);
  const [priorityDropdownOpen, setPriorityDropdownOpen] = createSignal(false);
  const [confirmDropdownOpen, setConfirmDropdownOpen] = createSignal(false);
  // Panel metrics memo is defined after `now`/`streamStart`/`isStreaming` below.
  let modelRef: HTMLDivElement | undefined;
  let priorityRef: HTMLDivElement | undefined;
  let confirmRef: HTMLDivElement | undefined;

  const handleClickOutside = (e: MouseEvent) => {
    if (modelRef && !modelRef.contains(e.target as Node)) {
      setModelDropdownOpen(false);
    }
    if (priorityRef && !priorityRef.contains(e.target as Node)) {
      setPriorityDropdownOpen(false);
    }
    if (confirmRef && !confirmRef.contains(e.target as Node)) {
      setConfirmDropdownOpen(false);
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
  const [showConfirm, setShowConfirm] = createSignal(false);
  const [copiedMessageId, setCopiedMessageId] = createSignal<string | null>(null);

  const handleCopyMessage = async (msg: ChatMessage) => {
    const textToCopy = getMessageCopyText(msg);
    try {
      await navigator.clipboard.writeText(textToCopy);
      setCopiedMessageId(msg.id);
      setTimeout(() => {
        if (copiedMessageId() === msg.id) {
          setCopiedMessageId(null);
        }
      }, 1500);
    } catch {}
  };

  const displayTitle = createMemo(() => {
    if (props.taskName && !props.taskName.endsWith("...")) {
      return props.taskName;
    }
    const firstMsg = props.messages.find((m) => m.role === "user");
    if (firstMsg) {
      for (const seg of firstMsg.segments) {
        const ts = asText(seg);
        if (ts) {
          return ts.text.trim().replace(/\n/g, " ");
        }
      }
    }
    return props.taskName || "对话";
  });

  // 流式输出状态：发送瞬间的本地 busy 与父级 streaming 合成，覆盖
  // start_agent_chat 立即返回但流式仍在进行的窗口期。
  const isStreaming = createMemo(() => busy() || props.streaming);

  createEffect(() => {
    // Reset confirmation state when messages or active conversation changes
    props.messages;
    props.taskName;
    setShowConfirm(false);
  });

  // Reasoning fold state: the latest reasoning run auto-opens while streaming
  // (so the in-progress 思考过程 is visible by default). A segment the user has
  // manually folded is left alone thereafter.
  const [openReasoningIds, setOpenReasoningIds] = createSignal<Set<string>>(new Set());
  const [manualReasoningIds, setManualReasoningIds] = createSignal<Set<string>>(new Set());
  // Tool segment fold state: a tool segment is auto-expanded while running,
  // auto-collapsed when its result arrives. Segments the user has manually
  // toggled are never auto-collapsed, so expanded results stay open mid-stream.
  const [expandedToolIds, setExpandedToolIds] = createSignal<Set<string>>(new Set());
  const [manualToolIds, setManualToolIds] = createSignal<Set<string>>(new Set());
  // (Removed delayed tool expand — tools now expand immediately when running.)

  function toggleReasoning(segId: string) {
    setManualReasoningIds((prev) => {
      const next = new Set(prev);
      next.add(segId);
      return next;
    });
    setOpenReasoningIds((prev) => {
      const next = new Set(prev);
      if (next.has(segId)) next.delete(segId);
      else next.add(segId);
      return next;
    });
  }

  function toggleTool(segId: string) {
    setManualToolIds((prev) => {
      const next = new Set(prev);
      next.add(segId);
      return next;
    });
    setExpandedToolIds((prev) => {
      const next = new Set(prev);
      if (next.has(segId)) next.delete(segId);
      else next.add(segId);
      return next;
    });
  }

  // ── 计时：按墙钟连续走表 ──
  // 「已工作」「思考过程」计时共用一个每 100ms 推进的时钟，按真实墙钟连续走动，
  // 不受流式 delta 到达节奏影响——模型卡顿（停顿一会儿再生成）时计时器依旧连续
  // 推进，不会冻结。旧实现里「思考过程」计时读的是段内存储的 elapsedMs，它只在
  // 收到 delta 时才更新，卡顿期间就表现为「停住」。
  const [now, setNow] = createSignal(Date.now());
  let streamStart = 0; // 当前流式开始的墙钟时间戳；0 表示未开始

  createEffect(() => {
    if (isStreaming()) {
      if (streamStart === 0) streamStart = Date.now();
    } else {
      streamStart = 0;
    }
  });

  // Panel metrics — single source of truth for every number/bars the data
  // panel renders. Re-derives reactively when tokenUsage, contextWindow, the
  // 100ms clock (live tok/s while streaming), or streaming state changes.
  const metrics = createMemo(() =>
    derivePanelMetrics(props.tokenUsage ?? null, {
      contextWindow: props.contextWindow ?? 128000,
      nowMs: now(),
      runStartMs: streamStart > 0 ? streamStart : undefined,
      streaming: isStreaming(),
    }),
  );
  // Capacity percentage for the always-visible pill (0 when no usage yet).
  const capPct = createMemo(() => metrics()?.capacity.pct ?? 0);

  onMount(() => {
    const handle = setInterval(() => {
      const t = Date.now();
      setNow(t);
      // Solid 在 setInterval 驱动下偶发不把 now() 刷新到深层 <Index>/<Show>
      // 嵌套里的 DOM 文本（表现为「只有来 delta 时计时才动」）。这里直接写
      // DOM 兜底，保证卡顿（无 delta 到达）期间计时也连续走动。
      if (isStreaming() && streamStart > 0) {
        const bt = document.getElementById("chat-bottom-timer");
        if (bt) bt.textContent = String(Math.floor((t - streamStart) / 1000));
      }
      const lm = props.messages[props.messages.length - 1];
      if (lm && lm.role === "assistant" && isStreaming()) {
        const s = lm.segments[lm.segments.length - 1];
        if (s && s.type === "reasoning" && s.startTime != null) {
          const el = document.getElementById(`rs-timer-${s.id}`);
          if (el) el.textContent = fmtMs(t - s.startTime);
        }
      }
    }, 100);
    onCleanup(() => clearInterval(handle));
  });

  let scrollEl: HTMLDivElement | undefined;

  // ── 贴底滚动 ──
  // 核心策略：用「程序性滚动」标志精确区分程序滚动与用户滚动，取代旧的
  //   scrollHeight 变化启发式——旧启发式在流式输出中会把用户的滚动事件一并
  //   吃掉，导致滚回底部也无法恢复贴底。
  //   - 自动贴底 / 回到底部按钮的滚动前置位标志，scroll 事件中标志为真则忽略。
  //   - 流式中 scroll 事件只负责「取消贴底」，不主动恢复（避免小幅上滚被误判
  //     为到底而反复贴底回弹）；改由 wheel 向下且接近底部时恢复贴底。
  //   - 非流式中 scroll 事件既可取消也可恢复贴底。
  const [stickToBottom, setStickToBottom] = createSignal(true);
  const [showScrollDown, setShowScrollDown] = createSignal(false);
  let isProgrammaticScroll = false;

  const handleScroll = (e: Event) => {
    if (isProgrammaticScroll) return; // 忽略程序性贴底滚动
    const el = e.currentTarget as HTMLDivElement;
    const diff = el.scrollHeight - el.scrollTop - el.clientHeight;
    const nearBottom = diff <= 30;
    if (!nearBottom) {
      // 离开底部 → 取消贴底（无论是否流式）
      setStickToBottom(false);
    } else if (!isStreaming()) {
      // 非流式状态下滚到底部 → 恢复贴底
      // 流式中不在此恢复（见 wheel），避免贴底回弹
      setStickToBottom(true);
    }
    setShowScrollDown(!nearBottom);
  };

  // wheel 事件在 scroll 事件之前同步触发，用于捕获滚动意图：
  //   向上 → 立即取消贴底；向下且已接近底部 → 恢复贴底（流式中亦然）。
  const handleWheel = (e: WheelEvent) => {
    if (e.deltaY < 0) {
      setStickToBottom(false);
    } else if (e.deltaY > 0 && scrollEl) {
      const diff = scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight;
      if (diff <= 30) setStickToBottom(true);
    }
  };

  // 程序性贴底：赋值前置位标志。由 scrollTop 赋值触发的 scroll 事件在当前帧
  // 的渲染步骤中派发（早于 rAF 回调），故下一帧清除标志即可安全忽略。
  function stickScrollToBottom() {
    if (!scrollEl) return;
    isProgrammaticScroll = true;
    scrollEl.scrollTop = scrollEl.scrollHeight;
    requestAnimationFrame(() => {
      isProgrammaticScroll = false;
    });
  }

  // 「回到底部」按钮：平滑滚动跨多帧，标志在 scrollend 后清除；
  // scrollend 不可用时由兜底定时器清除，避免标志滞留导致后续用户滚动被忽略。
  function smoothStickToBottom() {
    if (!scrollEl) return;
    const el = scrollEl;
    isProgrammaticScroll = true;
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
    let t: ReturnType<typeof setTimeout>;
    const clear = () => {
      isProgrammaticScroll = false;
      el.removeEventListener("scrollend", clear);
      clearTimeout(t);
    };
    el.addEventListener("scrollend", clear);
    t = setTimeout(clear, 600);
  }

  // 新消息 / 流式 delta 到达时，若贴底则跟随滚动。
  // 用 untrack 读取 stickToBottom，避免 stickToBottom 变化本身触发此 effect。
  createEffect(() => {
    props.messages;
    isStreaming();
    if (scrollEl && untrack(stickToBottom)) {
      stickScrollToBottom();
    }
  });

  // 切换对话时重置折叠状态并跳到最新消息。
  // ⚠️ 关键：根据 props.taskId 的变更来检测对话切换，避免依赖 tasks() 导致流式输出中反复重置，
  // 并且保证即使对话同名也能正确触发重置。
  let prevTaskId: string | undefined;
  createEffect(() => {
    const currentId = props.taskId;
    if (currentId === prevTaskId) return; // 同一个对话，跳过
    prevTaskId = currentId;
    setOpenReasoningIds(new Set<string>());
    setExpandedToolIds(new Set<string>());
    setManualReasoningIds(new Set<string>());
    setManualToolIds(new Set<string>());
    setStickToBottom(true);
    stickScrollToBottom();
  });

  // The latest assistant message id (streaming target).
  function lastAssistantId(): string | undefined {
    const msgs = props.messages;
    if (msgs.length === 0) return undefined;
    const last = msgs[msgs.length - 1];
    return last.role === "assistant" ? last.id : undefined;
  }

  // Active reasoning segment id during streaming
  const activeReasoningId = createMemo(() => {
    const id = lastAssistantId();
    if (!id) return undefined;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return undefined;
    const segs = msg.segments;
    const last = segs[segs.length - 1];
    return isStreaming() && last && last.type === "reasoning" ? last.id : undefined;
  });

  // 折叠策略：只有「正在进行中的思考过程」默认展开——即流式输出中且
  // 消息的最后一段仍是 reasoning（还在往里追加）。一旦后面来了 tool/text
  // 段，这段思考即视为已结束 → 默认折叠。用户手动展开/收起的段保持原状。
  createEffect(() => {
    const id = lastAssistantId();
    if (!id) return;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return;
    const segs = msg.segments;
    const last = segs[segs.length - 1];
    const activeId =
      isStreaming() && last && last.type === "reasoning" ? last.id : undefined;
    const manual = manualReasoningIds();
    setOpenReasoningIds((prev) => {
      const next = new Set(prev);
      for (const s of segs) {
        if (s.type !== "reasoning") continue;
        if (manual.has(s.id)) continue; // 用户操作过的段保持原状
        if (s.id === activeId) next.add(s.id);
        else next.delete(s.id);
      }
      return next;
    });
  });

  // Drive tool-segment auto-expand/collapse. A running tool expands immediately.
  // Completed tools collapse to one line (unless the user has manually toggled
  // them). An `awaiting` tool (pending user confirm) also expands immediately
  // and never auto-collapses.
  createEffect(() => {
    const id = lastAssistantId();
    if (!id) return;
    const msg = props.messages.find((m) => m.id === id);
    if (!msg) return;
    const running = new Set(
      msg.segments
        .filter((s): s is Extract<Segment, { type: "tool" }> => s.type === "tool")
        .filter((s) => s.status === "running")
        .map((s) => s.id),
    );
    const awaiting = new Set(
      msg.segments
        .filter((s): s is Extract<Segment, { type: "tool" }> => s.type === "tool")
        .filter((s) => s.status === "awaiting")
        .map((s) => s.id),
    );

    // running + awaiting 工具立即展开（读 expandedToolIds 用 untrack 避免循环）。
    const expanded0 = untrack(expandedToolIds);
    let changed = false;
    const next = new Set(expanded0);
    for (const r of running) {
      if (!next.has(r)) { next.add(r); changed = true; }
    }
    for (const a of awaiting) {
      if (!next.has(a)) { next.add(a); changed = true; }
    }
    if (changed) setExpandedToolIds(next);

    // 已完成（不再 running/awaiting）的工具：从展开集中移除（用户手动操作过的除外）。
    const manual = manualToolIds();
    const expanded = untrack(expandedToolIds);
    const toCollapse: string[] = [];
    for (const s of msg.segments) {
      if (s.type !== "tool" || running.has(s.id) || awaiting.has(s.id)) continue;
      if (!manual.has(s.id) && expanded.has(s.id)) toCollapse.push(s.id);
    }
    if (toCollapse.length > 0) {
      setExpandedToolIds((prev) => {
        const next = new Set(prev);
        for (const c of toCollapse) next.delete(c);
        return next;
      });
    }
  });

  async function send() {
    const text = input().trim();
    if (!text || isStreaming()) return;
    setInput("");
    setBusy(true);
    setStickToBottom(true); // 发送新消息时重置为贴底
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
        <div style="display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0;">
          <span class="chat-header__title">{displayTitle()}</span>
          <span class="chat-header__ws" title={`当前工作区: ${props.workspace}`}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; flex-shrink: 0; color: var(--text-dim);">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
            </svg>
            <span class="ws-text">{props.workspace}</span>
          </span>
        </div>
        <Show
          when={showConfirm()}
          fallback={
            <button
              class="header-close-btn"
              title="关闭并删除对话"
              onClick={() => {
                if (props.messages.length > 0) {
                  setShowConfirm(true);
                } else {
                  props.onDelete?.();
                }
              }}
            >
              ✕
            </button>
          }
        >
          <div style="display: flex; align-items: center; gap: 8px; font-size: 12px; background: var(--bg-hover); padding: 4px 10px; border-radius: 6px; border: 1px solid var(--border-faint);">
            <span style="color: var(--accent-red); font-weight: 500;">确定删除？</span>
            <button
              onClick={() => {
                setShowConfirm(false);
                props.onDelete?.();
              }}
              style="background: var(--accent-red); color: white; border: none; padding: 2px 8px; border-radius: 4px; cursor: pointer; font-size: 11px; font-weight: 500;"
            >
              确定
            </button>
            <button
              onClick={() => setShowConfirm(false)}
              style="background: transparent; color: var(--text-secondary); border: 1px solid var(--border-strong); padding: 2px 8px; border-radius: 4px; cursor: pointer; font-size: 11px; font-weight: 500;"
            >
              取消
            </button>
          </div>
        </Show>
      </div>
      <div class="chat-stream" ref={scrollEl} onScroll={handleScroll} onWheel={handleWheel}>
        <Show
          when={props.messages.length > 0}
          fallback={<div class="chat-empty">向 LakeMind 提问，开始探索你的数据。</div>}
        >
          <Index each={props.messages}>
            {(msg) => (
              <div class={`chat-msg chat-msg--${msg().role}`}>
                <div class="chat-msg__body">
                  {/* Single ordered loop: preserves the real reasoning → tool →
                      … → text transcript instead of grouping by type. */}
                  <Index each={msg().segments}>
                    {(seg) => {
                      const rs = () => asReasoning(seg());
                      const ts = () => asText(seg());
                      const es = () => asError(seg());
                      // 本段思考耗时：进行中的段按墙钟实时走表（now() 每 100ms 推进，
                      // 卡顿期间也连续走动）；已结束的段读后端记录的 elapsedMs。
                      // 用 memo 显式声明对 now() 的依赖，避免内联表达式在深层
                      // <Index>/<Switch>/<Show> 嵌套下漏更新。
                      const reasoningMs = createMemo<number | undefined>(() => {
                        if (seg().id === activeReasoningId() && rs()?.startTime != null) {
                          return now() - rs()!.startTime!;
                        }
                        return rs()?.elapsedMs;
                      });
                      return (
                        <Switch>
                          <Match when={seg().type === "error" && es()}>
                            <div class="chat-terminal-error">
                              <span class="chat-terminal-error__icon">
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                                  <path d="m21.73 18-8-14a2 2 0 0 0-3.48 0l-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.73-3Z" />
                                  <line x1="12" y1="9" x2="12" y2="13" />
                                  <line x1="12" y1="17" x2="12.01" y2="17" />
                                </svg>
                              </span>
                              <span class="chat-terminal-error__text">{es()!.text}</span>
                            </div>
                          </Match>
                          <Match when={seg().type === "reasoning"}>
                            <div class="chat-reasoning">
                              <div class="chat-reasoning__header" onClick={() => toggleReasoning(seg().id)}>
                                <span class="chat-reasoning__icon">
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                                    <path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96-.44 2.5 2.5 0 0 1 0-3.12 3 3 0 0 1 0-4.88 2.5 2.5 0 0 1 0-3.12A2.5 2.5 0 0 1 9.5 2Z" />
                                    <path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96-.44 2.5 2.5 0 0 0 0-3.12 3 3 0 0 0 0-4.88 2.5 2.5 0 0 0 0-3.12A2.5 2.5 0 0 0 14.5 2Z" />
                                  </svg>
                                </span>
                                <span class="chat-reasoning__label">思考过程</span>
                                 <Show when={reasoningMs() != null}>
                                   <span style="color: var(--text-dim); margin-left: 2px;">· <span id={`rs-timer-${seg().id}`}>{fmtMs(reasoningMs()!)}</span></span>
                                 </Show>
                                 <span class="chat-reasoning__toggle" classList={{ "chat-reasoning__toggle--open": openReasoningIds().has(seg().id) }}>
                                   <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 10px; height: 10px; transition: transform 0.15s ease;">
                                     <polyline points="9 18 15 12 9 6"></polyline>
                                   </svg>
                                 </span>
                              </div>
                              <Show when={openReasoningIds().has(seg().id) && rs()}>
                                <ReasoningBody text={rs()!.text} />
                              </Show>
                            </div>
                          </Match>
                          <Match when={seg().type === "tool"}>
                            <Show when={seg()}>
                              {(s) => (
                                <ToolSegment
                                  seg={s()}
                                  expanded={expandedToolIds().has(s().id)}
                                  onToggle={toggleTool}
                                  onOpenInSqlPanel={props.onOpenInSqlPanel}
                                  onConfirm={(approved) => props.onConfirmTool(s().id, approved)}
                                />
                              )}
                            </Show>
                          </Match>
                          <Match when={seg().type === "chart"}>
                            <Show when={seg()}>
                              {(s) => (
                                <ChartSegment seg={s() as Extract<Segment, { type: "chart" }>} />
                              )}
                            </Show>
                          </Match>
                          <Match when={seg().type === "text" && ts()}>
                            <div class="chat-msg__text">
                              <Show
                                when={msg().role === "assistant"}
                                fallback={ts()!.text}
                              >
                                <MarkdownRenderer content={ts()!.text} />
                              </Show>
                            </div>
                          </Match>
                        </Switch>
                      );
                    }}
                  </Index>
                  <Show when={!(msg().role === "assistant" && isStreaming() && msg().id === props.messages[props.messages.length - 1]?.id)}>
                    <div class="chat-msg__actions">
                      <span class="chat-msg__time">{formatTime(msg().ts)}</span>
                      <button
                        class="chat-msg__copy-btn"
                        title={copiedMessageId() === msg().id ? "已复制" : "复制"}
                        onClick={() => handleCopyMessage(msg())}
                      >
                        <Show
                          when={copiedMessageId() === msg().id}
                          fallback={
                            <svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                              <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                            </svg>
                          }
                        >
                          <svg xmlns="http://www.w3.org/2000/svg" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="var(--accent-green, #10b981)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round">
                            <polyline points="20 6 9 17 4 12"></polyline>
                          </svg>
                        </Show>
                      </button>
                    </div>
                  </Show>
                </div>
              </div>
            )}
          </Index>


          {/* Busy / streaming indicator — single-line status */}
          <Show when={isStreaming()}>
            <div class="chat-msg chat-msg--assistant">
              <div class="chat-msg__body">
                <div class="chat-agent-status">
                  <span class="agent-status__timer">⏱ 已工作 <span id="chat-bottom-timer">{Math.floor((now() - streamStart) / 1000)}</span> 秒</span>
                </div>
              </div>
            </div>
          </Show>
        </Show>
      </div>

      <Show when={showScrollDown()}>
        <button
          class="chat-view__scroll-down"
          onClick={() => {
            setStickToBottom(true);
            smoothStickToBottom();
          }}
          title="回到底部"
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <polyline points="19 12 12 19 5 12"></polyline>
          </svg>
        </button>
      </Show>

      <div class="chat-composer">
        <div class="chat-composer__box">
          <textarea
            class="chat-composer__input"
            placeholder="向 LakeMind 提问（Enter 发送 · Shift+Enter 换行）…"
            value={input()}
            onInput={(e) => setInput(e.currentTarget.value)}
            onkeydown={onKeydown}
            disabled={isStreaming()}
            rows={2}
          />
          <div class="chat-composer__toolbar">
            <div style="display: flex; align-items: center; gap: 8px; flex-shrink: 0;">
              <button
                class="chat-composer__plus-btn"
                style="background: transparent; border: none; padding: 4px; display: flex; align-items: center; justify-content: center; color: var(--text-dim); cursor: pointer;"
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 16px; height: 16px;">
                  <line x1="12" y1="5" x2="12" y2="19"></line>
                  <line x1="5" y1="12" x2="19" y2="12"></line>
                </svg>
              </button>
            </div>

            <div style="display: flex; align-items: center; gap: 10px; min-width: 0; flex-shrink: 1;">
              {/* Token usage indicator — hover for details */}
              <div class="token-usage-wrap">
                <div
                  class="token-usage-pill"
                  classList={{
                    "token-usage-pill--warn": capPct() >= 70 && capPct() < 90,
                    "token-usage-pill--danger": capPct() >= 90,
                  }}
                  title="上下文容量"
                >
                  <span class="battery-icon-wrapper">
                    <svg class="battery-icon" viewBox="0 0 24 12" fill="none" stroke="currentColor" stroke-width="1.5">
                      <rect x="1" y="1" width="18" height="10" rx="2" />
                      <path d="M20 4v4" stroke-linecap="round" />
                      <Show when={capPct() > 0}>
                        <rect
                          x="2.5"
                          y="2.5"
                          width={15 * (capPct() / 100)}
                          height="7"
                          rx="1"
                          fill="currentColor"
                          stroke="none"
                        />
                      </Show>
                    </svg>
                  </span>
                  <span class="token-usage-pct">{capPct().toFixed(0)}%</span>
                </div>
                <div class="token-usage-panel">
                  <Show
                    when={metrics()}
                    fallback={<div class="token-usage-panel__empty">暂无用量数据</div>}
                  >
                    {(m) => (
                      <>
                        {/* Header: 上下文容量 and value */}
                        <div class="token-usage-panel__header">
                          <span class="token-usage-panel__title">上下文容量</span>
                          <span class="token-usage-panel__capacity">
                            {fmtCap(m().capacity.peak)}/{fmtCap(m().capacity.ctx)} ({fmtPct(m().capacity.pct)})
                          </span>
                        </div>

                        {/* Progress Bar */}
                        <div class="token-usage-panel__bar">
                          <div
                            class="token-usage-panel__bar-fill"
                            style={{
                              width: `${Math.max(0, Math.min(100, m().capacity.pct))}%`,
                              background: m().capacity.pct >= 90 ? "#ef4444" : m().capacity.pct >= 70 ? "#f6bd16" : "#5b8ff9",
                            }}
                          />
                        </div>

                        {/* Composition List */}
                        <div class="token-usage-panel__list">
                          <div class="token-usage-panel__item">
                            <span class="token-usage-panel__dot token-usage-panel__dot--msg" />
                            <span class="token-usage-panel__label">消息</span>
                            <span class="token-usage-panel__value">{fmtPct(m().composition.messages.pct)}</span>
                          </div>
                          <div class="token-usage-panel__item">
                            <span class="token-usage-panel__dot token-usage-panel__dot--tools" />
                            <span class="token-usage-panel__label">系统工具</span>
                            <span class="token-usage-panel__value">{fmtPct(m().composition.tools.pct)}</span>
                          </div>
                          <div class="token-usage-panel__item">
                            <span class="token-usage-panel__dot token-usage-panel__dot--preamble" />
                            <span class="token-usage-panel__label">系统提示词</span>
                            <span class="token-usage-panel__value">{fmtPct(m().composition.preamble.pct)}</span>
                          </div>
                        </div>

                        {/* Gap/Spacer */}
                        <div class="token-usage-panel__spacer" />

                        {/* Average Cache Hit Rate */}
                        <div class="token-usage-panel__item token-usage-panel__item--highlight">
                          <span class="token-usage-panel__dot token-usage-panel__dot--hitrate" />
                          <span class="token-usage-panel__label">平均缓存命中率</span>
                          <span class="token-usage-panel__value token-usage-panel__value--green">
                            {fmtPct(m().cumulative.hitRate)}
                          </span>
                        </div>
                      </>
                    )}
                  </Show>
                </div>
              </div>

              {/* Model Selector Dropdown */}
              <div class="dropdown-wrapper" ref={modelRef} style="position: relative;">
                <button
                  class="chat-composer__pill-btn select-btn"
                  onClick={() => setModelDropdownOpen(!modelDropdownOpen())}
                >
                  <span>{props.selectedModel || "选择模型"}</span>
                  <span class="btn-caret">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 8px; height: 8px;">
                      <polyline points="6 9 12 15 18 9"></polyline>
                    </svg>
                  </span>
                </button>
                <Show when={modelDropdownOpen()}>
                  <div class="custom-dropdown-list" style="bottom: calc(100% + 6px); right: 0; left: auto;">
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
                  class="chat-composer__pill-btn select-btn"
                  onClick={() => setPriorityDropdownOpen(!priorityDropdownOpen())}
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                    <path d="M9.5 2A2.5 2.5 0 0 1 12 4.5v15a2.5 2.5 0 0 1-4.96-.44 2.5 2.5 0 0 1 0-3.12 3 3 0 0 1 0-4.88 2.5 2.5 0 0 1 0-3.12A2.5 2.5 0 0 1 9.5 2Z" />
                    <path d="M14.5 2A2.5 2.5 0 0 0 12 4.5v15a2.5 2.5 0 0 0 4.96-.44 2.5 2.5 0 0 0 0-3.12 3 3 0 0 0 0-4.88 2.5 2.5 0 0 0 0-3.12A2.5 2.5 0 0 0 14.5 2Z" />
                  </svg>
                  <span>{props.selectedPriority}</span>
                  <span class="btn-caret">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 8px; height: 8px;">
                      <polyline points="6 9 12 15 18 9"></polyline>
                    </svg>
                  </span>
                </button>
                <Show when={priorityDropdownOpen()}>
                  <div class="custom-dropdown-list" style="bottom: calc(100% + 6px); right: 0; left: auto;">
                    <button class="dropdown-item" onClick={() => { props.onSelectPriority("最高"); setPriorityDropdownOpen(false); }}>最高</button>
                    <button class="dropdown-item" onClick={() => { props.onSelectPriority("均衡"); setPriorityDropdownOpen(false); }}>均衡</button>
                    <button class="dropdown-item" onClick={() => { props.onSelectPriority("最快"); setPriorityDropdownOpen(false); }}>最快</button>
                  </div>
                </Show>
              </div>

              {/* Confirmation Mode Selector Dropdown */}
              <div class="dropdown-wrapper" ref={confirmRef} style="position: relative;">
                <button
                  class="chat-composer__pill-btn select-btn"
                  onClick={() => setConfirmDropdownOpen(!confirmDropdownOpen())}
                >
                  <span class="btn-prefix">
                    {props.selectedConfirm === "自动执行" ? (
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                        <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon>
                      </svg>
                    ) : (
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                        <path d="M9 11V6a2 2 0 0 1 4 0v5"></path>
                        <path d="M13 6a2 2 0 0 1 4 0v5"></path>
                        <path d="M17 6a2 2 0 0 1 4 0v8a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"></path>
                      </svg>
                    )}
                  </span>
                  <span>{props.selectedConfirm}</span>
                  <span class="btn-caret">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" style="width: 8px; height: 8px;">
                      <polyline points="6 9 12 15 18 9"></polyline>
                    </svg>
                  </span>
                </button>
                <Show when={confirmDropdownOpen()}>
                  <div class="custom-dropdown-list" style="bottom: calc(100% + 6px); right: 0; left: auto;">
                    <button class="dropdown-item" onClick={() => { props.onSelectConfirm("变更前确认"); setConfirmDropdownOpen(false); }}>变更前确认</button>
                    <button class="dropdown-item" onClick={() => { props.onSelectConfirm("自动执行"); setConfirmDropdownOpen(false); }}>自动执行</button>
                  </div>
                </Show>
              </div>

              <Show
                when={isStreaming()}
                fallback={
                  <button
                    class="chat-composer__send-square"
                    disabled={!input().trim()}
                    onClick={() => void send()}
                    title="发送"
                  >
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                      <line x1="12" y1="19" x2="12" y2="5"></line>
                      <polyline points="5 12 12 5 19 12"></polyline>
                    </svg>
                  </button>
                }
              >
                <button
                  class="chat-composer__send-square chat-composer__stop"
                  onClick={() => props.onStop?.()}
                  title="停止生成"
                >
                  <svg viewBox="0 0 24 24" fill="currentColor" style="width: 12px; height: 12px;">
                    <rect x="6" y="6" width="12" height="12" rx="2" />
                  </svg>
                </button>
              </Show>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * 思考过程内容区：自带独立的贴底滚动管理。
 * - 默认贴底，随新文本自动滚动到最新内容。
 * - 用户向上滚动时停止贴底，可自由翻阅。
 * - 用户滚回底部时恢复贴底。
 * - wheel 事件在内层可滚动时阻止冒泡，使内外层滚动互不干扰。
 */
function ReasoningBody(props: { text: string }) {
  let bodyRef: HTMLDivElement | undefined;
  const [stick, setStick] = createSignal(true);
  let lastScrollHeight = 0;

  // 文本变化时，若贴底则自动滚到最新内容
  createEffect(() => {
    props.text;
    if (bodyRef && untrack(stick)) {
      bodyRef.scrollTop = bodyRef.scrollHeight;
    }
  });

  const handleScroll = () => {
    if (!bodyRef) return;
    const currentScrollHeight = bodyRef.scrollHeight;

    // 若高度改变，说明是内容加载/排版变化，忽略此事件，防止误触取消贴底
    if (currentScrollHeight !== lastScrollHeight) {
      lastScrollHeight = currentScrollHeight;
      return;
    }

    // 根据滚动后位置自动判断贴底状态：
    // 只要位于底部附近（包含程序滚动产生的到底），就保持贴底状态；
    // 向上滑动后偏离底部（diff > 15），则设为不贴底，允许自由翻阅。
    const diff = bodyRef.scrollHeight - bodyRef.scrollTop - bodyRef.clientHeight;
    setStick(diff <= 15);
  };

  const handleWheel = (e: WheelEvent) => {
    if (!bodyRef) return;
    const el = bodyRef;
    if (e.deltaY < 0) {
      // 向上滚动时，立即取消贴底，无需等待 scroll 事件
      setStick(false);
      // 内层还能向上滚 → 阻止冒泡，不影响外层
      if (el.scrollTop > 0) e.stopPropagation();
    } else if (e.deltaY > 0) {
      // 向下滚动，内层还能向下滚 → 阻止冒泡
      if (el.scrollHeight - el.scrollTop - el.clientHeight > 1) {
        e.stopPropagation();
      }
    }
  };

  return (
    <div
      class="chat-reasoning__body"
      ref={bodyRef}
      onScroll={handleScroll}
      onWheel={handleWheel}
    >
      {props.text}
    </div>
  );
}

function fmtMs(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}


function formatTime(ts: number): string {
  const d = new Date(ts);
  const h = d.getHours().toString().padStart(2, '0');
  const m = d.getMinutes().toString().padStart(2, '0');
  return `${h}:${m}`;
}

function getMessageCopyText(msg: ChatMessage): string {
  const texts = msg.segments
    .filter((s) => s.type === "text")
    .map((s) => (s as any).text);
  if (texts.length > 0) {
    return texts.join("\n");
  }
  return msg.segments
    .map((s) => {
      if (s.type === "text" || s.type === "reasoning" || s.type === "error") {
        return (s as any).text;
      }
      return "";
    })
    .filter(Boolean)
    .join("\n");
}
