import { For, Show, createSignal, createEffect, onMount, onCleanup } from "solid-js";
import { EditorView, keymap, lineNumbers } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { sql } from "@codemirror/lang-sql";
import { oneDark } from "@codemirror/theme-one-dark";
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
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        lineNumbers(),
        sql(),
        oneDark,
        EditorView.lineWrapping,
        EditorView.updateListener.of((u) => {
          if (u.docChanged) props.onSql(u.state.doc.toString());
        }),
        EditorView.theme({
          "&": { backgroundColor: "var(--bg-surface)" },
          ".cm-gutters": { backgroundColor: "var(--bg-surface)", border: "none" },
          "&.cm-focused": { outline: "none" },
        }),
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

  return (
    <div class="editor-card">
      <div class="editor-toolbar">
        <div class="right-tools">
          <select
            class="rowcap-select"
            title={t("rowCountLimit")}
            disabled={props.busy}
            value={props.rowCap}
            onChange={(e) => props.onRowCap(Number(e.currentTarget.value))}
          >
            <For each={[...ROW_CAP_OPTIONS]}>{(o) => <option value={o.value}>{o.label}</option>}</For>
          </select>
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
          <button class="run-btn" title={`${t("run")} (Ctrl/Cmd+Enter)`} disabled={props.busy} onClick={() => props.onRun()}>
            <Show when={props.busy} fallback={
              <>
                <svg viewBox="0 0 24 24" fill="currentColor" style="width: 10px; height: 10px; margin-right: 4px;">
                  <polygon points="5 3 19 12 5 21 5 3"></polygon>
                </svg>
                {t("run")}
              </>
            }>
              {t("running")}
            </Show>
          </button>
        </div>
      </div>
      <div class="cm-host" ref={host} style={{ height: `${editorHeight()}px` }} />
      <div class="editor-resizer-h" onMouseDown={startDraggingHeight} />
    </div>
  );
}
