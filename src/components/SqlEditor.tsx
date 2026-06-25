import { For, Show, createSignal, createEffect, onMount, onCleanup } from "solid-js";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import { EditorState, Compartment } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { sql } from "@codemirror/lang-sql";
import { githubLight, githubDark } from "@uiw/codemirror-theme-github";
import { formatDuckdbSql } from "../lib/sqlFormat";
import { isLightCodeTheme, codeLineNumbers, codeWrap, codeFontSize } from "../lib/codeConfig";
import { ROW_CAP_OPTIONS } from "../lib/types";
import { t } from "../lib/i18n";

/**
 * SQL editor in a Mac console-style card: an environment tag (📊 DuckDB SQL)
 * on the left, run/copy controls on the right, the CodeMirror surface below.
 *
 * Ctrl/Cmd+Enter runs the buffer. The row-cap dropdown caps how many rows the
 * backend may return (0 = uncapped).
 */
export default function SqlEditor(props: {
  initialSql: string;
  rowCap: number;
  busy: boolean;
  onSql: (sql: string) => void;
  onRowCap: (cap: number) => void;
  onRun: () => void;
  onCopy: () => void;
  onSave?: () => void;
  onClose?: () => void;
}) {
  const [copied, setCopied] = createSignal(false);
  const [saved, setSaved] = createSignal(false);
  const [editorHeight, setEditorHeight] = createSignal(180);
  const [formatState, setFormatState] = createSignal<"idle" | "ok" | "err">("idle");
  const [formatErr, setFormatErr] = createSignal<string | null>(null);
  // 行数限制自定义下拉的展开状态。原生 <select> 的下拉面板宽度由系统控制、
  // 无法与选择框等宽，改用自定义下拉以统一宽度。
  const [rowcapOpen, setRowcapOpen] = createSignal(false);
  let rowcapWrapperRef!: HTMLDivElement;
  let host!: HTMLDivElement;

  function startDraggingHeight(e: MouseEvent) {
    e.preventDefault();
    const startY = e.clientY;
    const startHeight = editorHeight();

    const onMouseMove = (moveEvent: MouseEvent) => {
      const deltaY = moveEvent.clientY - startY;
      const newHeight = Math.max(50, Math.min(600, startHeight + deltaY));
      setEditorHeight(newHeight);
    };

    const onMouseUp = () => {
      document.removeEventListener("mousemove", onMouseMove);
      document.removeEventListener("mouseup", onMouseUp);
      document.body.classList.remove("dragging-active-v");
    };

    document.addEventListener("mousemove", onMouseMove);
    document.addEventListener("mouseup", onMouseUp);
    document.body.classList.add("dragging-active-v");
  }
  let view: EditorView | undefined;
  // Compartments for runtime-swappable extensions: theme, line numbers, and
  // line wrapping. Each can be reconfigured via dispatch({effects: ...reconfigure})
  // without rebuilding the editor — the standard CodeMirror 6 pattern.
  const themeCompartment = new Compartment();
  const lineNumberCompartment = new Compartment();
  const lineWrapCompartment = new Compartment();
  /** Editor chrome theme (background, selection, font-size). Uses a compartment
   *  so font-size changes from settings reconfigure without rebuilding. */
  const styleCompartment = new Compartment();
  /** Pick the CodeMirror theme matching the current code-theme light/dark. */
  const cmTheme = () => (isLightCodeTheme() ? githubLight : githubDark);
  /** Editor chrome theme extension driven by codeFontSize(). */
  const cmStyle = () => EditorView.theme({
    "&": { backgroundColor: "var(--bg-surface)" },
    ".cm-content": { fontSize: `${codeFontSize()}px` },
    ".cm-gutters": { backgroundColor: "var(--bg-surface)", border: "none", fontSize: `${codeFontSize()}px` },
    "&.cm-focused": { outline: "none" },
    ".cm-selectionBackground": { backgroundColor: "var(--cm-selection-bg) !important" },
    "&.cm-focused .cm-selectionBackground": { backgroundColor: "var(--cm-selection-bg-focused) !important" },
    "::selection": { backgroundColor: "var(--cm-selection-bg-focused) !important" },
  });

  /**
   * 用 sql-formatter 的 DuckDB 方言格式化当前编辑器全文。成功后替换整个文档
   * （进入 history，可 Ctrl/Cmd+Z 撤销；updateListener 会自动把新内容同步给父组件）。
   * 失败时按钮短暂变红，title 与浏览器控制台输出错误信息。
   */
  function formatSql() {
    const v = view;
    if (!v) return;
    const sqlText = v.state.doc.toString();
    if (!sqlText.trim()) return;
    try {
      const formatted = formatDuckdbSql(sqlText);
      if (formatted !== sqlText) {
        v.dispatch({
          changes: { from: 0, to: v.state.doc.length, insert: formatted },
        });
      }
      setFormatErr(null);
      setFormatState("ok");
      setTimeout(() => setFormatState("idle"), 1500);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setFormatErr(msg);
      setFormatState("err");
      console.error("[SqlEditor] format failed:", msg);
      setTimeout(() => setFormatState("idle"), 3000);
    }
  }

  // 点击行数下拉外部时收起。
  onMount(() => {
    const onDocClick = (e: MouseEvent) => {
      const target = e.target as Node;
      if (rowcapOpen() && rowcapWrapperRef && !rowcapWrapperRef.contains(target)) {
        setRowcapOpen(false);
      }
    };
    document.addEventListener("mousedown", onDocClick);
    onCleanup(() => document.removeEventListener("mousedown", onDocClick));
  });

  onMount(() => {
    const state = EditorState.create({
      doc: props.initialSql,
      extensions: [
        history(),
        keymap.of([
          {
            key: "Mod-Enter",
            run: () => {
              props.onRun();
              return true;
            },
          },
          {
            key: "Mod-s",
            run: () => {
              props.onSave?.();
              setSaved(true);
              setTimeout(() => setSaved(false), 1500);
              return true;
            },
          },
          {
            key: "Mod-Shift-F",
            run: () => {
              formatSql();
              return true;
            },
          },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        sql(),
        themeCompartment.of(cmTheme()),
        // 行号 / 换行由代码预览设置驱动，用 compartment 以便运行时切换。
        lineNumberCompartment.of(codeLineNumbers() ? lineNumbers() : []),
        lineWrapCompartment.of(codeWrap() ? EditorView.lineWrapping : []),
        EditorView.updateListener.of((u) => {
          if (u.docChanged) props.onSql(u.state.doc.toString());
        }),
        styleCompartment.of(cmStyle()),
      ],
    });
    view = new EditorView({ state, parent: host });
    onCleanup(() => view?.destroy());
  });

  // 当外部 initialSql 变化（如切换 SQL task、对话卡片「在 SQL 面板打开」注入）
  // 时，把编辑器文档同步到新内容。修复初始实现只在 mount 读一次的 bug。
  createEffect(() => {
    const next = props.initialSql;
    const v = view;
    if (!v) return;
    if (next !== v.state.doc.toString()) {
      v.dispatch({
        changes: { from: 0, to: v.state.doc.length, insert: next },
      });
    }
  });

  // 代码主题明暗变化时（设置页切换主题，或界面明暗切换）热替换编辑器主题。
  // 读 isLightCodeTheme() 建立依赖；reconfigure 只换主题扩展，不动文档/光标。
  createEffect(() => {
    const v = view;
    if (!v) return;
    void isLightCodeTheme();
    v.dispatch({ effects: themeCompartment.reconfigure(cmTheme()) });
  });

  // 行号 / 换行 / 字号变化时热替换对应扩展。分别读信号建立依赖。
  createEffect(() => {
    const v = view;
    if (!v) return;
    void codeLineNumbers();
    v.dispatch({ effects: lineNumberCompartment.reconfigure(codeLineNumbers() ? lineNumbers() : []) });
  });

  createEffect(() => {
    const v = view;
    if (!v) return;
    void codeWrap();
    v.dispatch({ effects: lineWrapCompartment.reconfigure(codeWrap() ? EditorView.lineWrapping : []) });
  });

  createEffect(() => {
    const v = view;
    if (!v) return;
    void codeFontSize();
    v.dispatch({ effects: styleCompartment.reconfigure(cmStyle()) });
  });

  return (
    <div class="editor-card">
      <div class="editor-toolbar">
        <div class="right-tools">
          <div class="rowcap-dropdown-wrapper" ref={rowcapWrapperRef}>
            <button
              class="rowcap-select"
              title={t("rowCountLimit")}
              disabled={props.busy}
              onClick={() => setRowcapOpen(!rowcapOpen())}
            >
              <span>{ROW_CAP_OPTIONS.find(o => o.value === props.rowCap)?.label ?? ROW_CAP_OPTIONS[0].label}</span>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 10px; height: 10px;">
                <polyline points="6 9 12 15 18 9"></polyline>
              </svg>
            </button>
            <Show when={rowcapOpen()}>
              <div class="rowcap-dropdown-list">
                <For each={[...ROW_CAP_OPTIONS]}>
                  {(o) => (
                    <button
                      class="rowcap-dropdown-item"
                      classList={{ active: o.value === props.rowCap }}
                      onClick={() => { props.onRowCap(o.value); setRowcapOpen(false); }}
                    >
                      {o.label}
                    </button>
                  )}
                </For>
              </div>
            </Show>
          </div>
          <button
            class="icon-btn"
            title={formatErr() ? `${t("formatSqlFailed")}：${formatErr()}` : `${t("formatSql")} (Ctrl/Cmd+Shift+F)`}
            onClick={formatSql}
          >
            {formatState() === "err" ? (
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent-red, #ef4444)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style={{ width: "14px", height: "14px" }}>
                <path d="M12 9v4"></path>
                <path d="M12 17h.01"></path>
              </svg>
            ) : formatState() === "ok" ? (
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent-green, #10b981)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" style={{ width: "14px", height: "14px" }}>
                <polyline points="20 6 9 17 4 12"></polyline>
              </svg>
            ) : (
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style={{ width: "14px", height: "14px" }}>
                <polygon points="14,4 20,10 14,16 8,10"></polygon>
                <path d="M11 13L4 20"></path>
              </svg>
            )}
          </button>
          <button
            class="icon-btn"
            title={copied() ? "已复制" : t("copySql")}
            onClick={() => {
              props.onCopy();
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            }}
          >
            <Show
              when={copied()}
              fallback={
                <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style={{ width: "14px", height: "14px" }}>
                  <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
                  <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
                </svg>
              }
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent-green, #10b981)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" style={{ width: "14px", height: "14px" }}>
                <polyline points="20 6 9 17 4 12"></polyline>
              </svg>
            </Show>
          </button>
          <button
            class="icon-btn"
            title={saved() ? "已保存" : "保存查询 (Ctrl+S)"}
            onClick={() => {
              props.onSave?.();
              setSaved(true);
              setTimeout(() => setSaved(false), 1500);
            }}
          >
            <Show
              when={saved()}
              fallback={
                <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style={{ width: "14px", height: "14px" }}>
                  <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"></path>
                  <path d="M17 21v-8H7v8"></path>
                  <path d="M7 3v5h8"></path>
                </svg>
              }
            >
              <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="var(--accent-green, #10b981)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" style={{ width: "14px", height: "14px" }}>
                <polyline points="20 6 9 17 4 12"></polyline>
              </svg>
            </Show>
          </button>
          <button class="header-close-btn" title="关闭并放弃查询" onClick={() => props.onClose?.()}>
            ✕
          </button>
          <button class="icon-btn" title={`${t("run")} (Ctrl/Cmd+Enter)`} disabled={props.busy} onClick={() => props.onRun()}>
            <Show when={props.busy} fallback={
              <svg viewBox="0 0 24 24" fill="currentColor" style={{ width: "12px", height: "12px" }}>
                <polygon points="5 3 19 12 5 21 5 3"></polygon>
              </svg>
            }>
              <div class="run-btn-spinner" />
            </Show>
          </button>
        </div>
      </div>
      <div class="cm-host" ref={host} style={{ height: `${editorHeight()}px` }} />
      <div class="editor-resizer-h" onMouseDown={startDraggingHeight} />
    </div>
  );
}
