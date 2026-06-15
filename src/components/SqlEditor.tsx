import { For, Show, onMount, onCleanup } from "solid-js";
import { EditorView, keymap } from "@codemirror/view";
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
}) {
  let host!: HTMLDivElement;
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
          ...defaultKeymap,
          ...historyKeymap,
        ]),
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

  return (
    <div class="editor-card">
      <div class="editor-toolbar">
        <span class="env-tag">{t("sqlEditorTag")}</span>
        <span class="shortcut-hint">Ctrl/Cmd+Enter</span>
        <div class="right-tools">
          <label class="rowcap">
            {t("rowCountLimit")}
            <select
              disabled={props.busy}
              value={props.rowCap}
              onChange={(e) => props.onRowCap(Number(e.currentTarget.value))}
            >
              <For each={[...ROW_CAP_OPTIONS]}>{(o) => <option value={o.value}>{o.label}</option>}</For>
            </select>
          </label>
          <button class="icon-btn" title={t("copySql")} onClick={() => props.onCopy()}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
              <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
            </svg>
          </button>
          <button class="run-btn" disabled={props.busy} onClick={() => props.onRun()}>
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
      <div class="cm-host" ref={host} />
    </div>
  );
}
