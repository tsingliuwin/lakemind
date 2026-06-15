import { createSignal, Show } from "solid-js";
import DropZone from "./components/DropZone";
import LeftNav from "./components/LeftNav";
import TitleBar from "./components/TitleBar";
import SqlEditor from "./components/SqlEditor";
import ResultTable from "./components/ResultTable";
import RightInspector from "./components/RightInspector";
import BottomConsole, { type ConsoleState } from "./components/BottomConsole";
import SettingsPage from "./components/SettingsPage";
import { executeSql } from "./lib/duckdb";
import type { LogEntry, SourceTable, SqlResult } from "./lib/types";
import "./App.css";

/**
 * LakeMind M1 — pure-compute DuckDB client.
 *
 * Four-way grid layout (ZCode 3.0 dark):
 *   ┌ TopBar ─────────────────────────────────────────────┐
 *   │ LeftNav │ Main(SqlEditor + ResultTable) │ RightInsp. │
 *   │         │ BottomConsole                 │           │
 *   └─────────────────────────────────────────────────────┘
 */
export default function App() {
  // --- data ---
  const [sources, setSources] = createSignal<SourceTable[]>([]);
  const [selectedTable, setSelectedTable] = createSignal<SourceTable | null>(null);

  // --- 编辑器 / 查询 ---
  const [sql, setSql] = createSignal<string>("SELECT 1 AS n;");
  const [rowCap, setRowCap] = createSignal<number>(10_000);
  const [result, setResult] = createSignal<SqlResult | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal<boolean>(false);

  // --- drawers & layout sizes ---
  const [inspectorOpen, setInspectorOpen] = createSignal<boolean>(true);
  const [consoleState, setConsoleState] = createSignal<ConsoleState>("folded");
  const [settingsOpen, setSettingsOpen] = createSignal<boolean>(false);

  const [leftWidth, setLeftWidth] = createSignal<number>(240);
  const [rightWidth, setRightWidth] = createSignal<number>(280);
  const [bottomHeight, setBottomHeight] = createSignal<number>(180);

  // --- execution log ---
  const [logs, setLogs] = createSignal<LogEntry[]>([]);
  let logSeq = 0;

  const [isDraggingLeft, setIsDraggingLeft] = createSignal<boolean>(false);
  const [isDraggingRight, setIsDraggingRight] = createSignal<boolean>(false);
  const [isDraggingBottom, setIsDraggingBottom] = createSignal<boolean>(false);

  function startDraggingLeft(e: MouseEvent) {
    e.preventDefault();
    setIsDraggingLeft(true);
    document.body.classList.add("dragging-active");
    const startX = e.clientX;
    const startWidth = leftWidth();

    const onMouseMove = (moveEvent: MouseEvent) => {
      const deltaX = moveEvent.clientX - startX;
      const newWidth = Math.max(160, Math.min(450, startWidth + deltaX));
      setLeftWidth(newWidth);
    };

    const onMouseUp = () => {
      setIsDraggingLeft(false);
      document.body.classList.remove("dragging-active");
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    };

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
  }

  function startDraggingRight(e: MouseEvent) {
    e.preventDefault();
    setIsDraggingRight(true);
    document.body.classList.add("dragging-active");
    const startX = e.clientX;
    const startWidth = rightWidth();

    const onMouseMove = (moveEvent: MouseEvent) => {
      const deltaX = moveEvent.clientX - startX;
      const newWidth = Math.max(200, Math.min(500, startWidth - deltaX));
      setRightWidth(newWidth);
    };

    const onMouseUp = () => {
      setIsDraggingRight(false);
      document.body.classList.remove("dragging-active");
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    };

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
  }

  function startDraggingBottom(e: MouseEvent) {
    e.preventDefault();
    setIsDraggingBottom(true);
    document.body.classList.add("dragging-active");
    const startY = e.clientY;
    const startHeight = bottomHeight();

    const onMouseMove = (moveEvent: MouseEvent) => {
      const deltaY = moveEvent.clientY - startY;
      const newHeight = Math.max(80, Math.min(600, startHeight - deltaY));
      setBottomHeight(newHeight);
      if (consoleState() === "folded") {
        setConsoleState("default");
      }
    };

    const onMouseUp = () => {
      setIsDraggingBottom(false);
      document.body.classList.remove("dragging-active");
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
    };

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
  }

  /** 控制台三档循环：折叠 → 默认 → 展开 → 折叠。 */
  function cycleConsole() {
    setConsoleState((s) => (s === "folded" ? "default" : s === "default" ? "expanded" : "folded"));
  }

  function mergeSources(incoming: SourceTable[]) {
    if (incoming.length === 0) return;
    setSources((prev) => {
      const map = new Map(prev.map((t) => [t.name, t]));
      for (const t of incoming) map.set(t.name, t);
      return [...map.values()];
    });
  }

  /** 执行当前 SQL，无论成功或失败都写入日志。 */
  async function run() {
    const q = sql().trim();
    if (!q || busy()) return;
    setBusy(true);
    setError(null);
    const started = Date.now();
    let entry: LogEntry;
    try {
      const res = await executeSql(q, rowCap());
      setResult(res);
      entry = {
        id: ++logSeq,
        ts: started,
        sql: q,
        status: "ok",
        rowCount: res.rowCount,
        truncated: res.truncated,
        elapsedMs: res.elapsedMs,
      };
    } catch (e) {
      const msg = String(e);
      setResult(null);
      setError(msg);
      entry = { id: ++logSeq, ts: started, sql: q, status: "error", error: msg, elapsedMs: Date.now() - started };
      // 失败时自动展开控制台，便于看到原始报错。
      setConsoleState((s) => (s === "folded" ? "default" : s));
    } finally {
      setBusy(false);
    }
    setLogs((prev) => [entry, ...prev].slice(0, 100));
  }

  /** 点击侧栏表：选中并在右侧检查器展示其 schema。 */
  function selectTable(t: SourceTable) {
    setSelectedTable(t);
    setInspectorOpen(true);
  }

  /** 检查器 → 编辑器：注入一段 SQL（不自动执行）。 */
  function injectSql(s: string) {
    setSql(s);
    setResult(null);
    setError(null);
  }

  /** 检查器 → 直接预览某表（SELECT * LIMIT 50）。 */
  function previewTable(t: SourceTable) {
    injectSql(`SELECT * FROM "${t.name}" LIMIT 50;`);
    void run();
  }

  async function copySql() {
    try {
      await navigator.clipboard.writeText(sql());
    } catch {
      /* clipboard may be unavailable; ignore */
    }
  }

  function onSettings() {
    setSettingsOpen(true);
  }

  const rightWidthActual = () => (settingsOpen() || !inspectorOpen()) ? "0px" : `${rightWidth()}px`;
  const bottomHeightActual = () => {
    if (settingsOpen()) return "0px";
    if (consoleState() === "folded") return "32px";
    if (consoleState() === "expanded") return `${bottomHeight() * 1.8}px`;
    return `${bottomHeight()}px`;
  };

  return (
    <div 
      classList={{ "app-shell": true, "no-inspector": !inspectorOpen() }}
      style={{
        "--left-width": `${leftWidth()}px`,
        "--right-width": `${rightWidth()}px`,
        "--right-width-actual": rightWidthActual(),
        "--bottom-height": `${bottomHeight()}px`,
        "--bottom-height-actual": bottomHeightActual()
      }}
    >
      <DropZone busy={busy()} onSources={mergeSources} onError={(m) => setError(m)} />

      {/* Vertical Left Resizer */}
      <Show when={!settingsOpen()}>
        <div 
          class="resizer-v" 
          classList={{ dragging: isDraggingLeft() }}
          style={{ left: `${leftWidth() - 3}px` }} 
          onMouseDown={startDraggingLeft}
        />
      </Show>
      
      {/* Vertical Right Resizer */}
      <Show when={inspectorOpen() && !settingsOpen()}>
        <div 
          class="resizer-v" 
          classList={{ dragging: isDraggingRight() }}
          style={{ right: `${rightWidth() - 3}px` }} 
          onMouseDown={startDraggingRight}
        />
      </Show>

      {/* Horizontal Console Resizer */}
      <Show when={consoleState() !== "folded" && !settingsOpen()}>
        <div 
          class="resizer-h" 
          classList={{ dragging: isDraggingBottom() }}
          style={{ 
            bottom: `${parseFloat(bottomHeightActual()) - 3}px`,
            left: `${leftWidth()}px`,
            right: rightWidthActual()
          }} 
          onMouseDown={startDraggingBottom}
        />
      </Show>

      <TitleBar
        inspectorOpen={inspectorOpen()}
        consoleOpen={consoleState() !== "folded"}
        onToggleInspector={() => setInspectorOpen((v) => !v)}
        onToggleConsole={() => setConsoleState((s) => (s === "folded" ? "default" : "folded"))}
        onNewQuery={() => injectSql("SELECT 1 AS n;")}
        selectedTable={selectedTable()}
        onOpenSettings={onSettings}
      />

      <Show when={settingsOpen()} fallback={
        <>
          <LeftNav
            workspace="lakemind"
            sources={sources()}
            selected={selectedTable()?.name ?? null}
            busy={busy()}
            onSelect={selectTable}
            onOpenSettings={onSettings}
            onNewQuery={() => injectSql("SELECT 1 AS n;")}
            inspectorOpen={inspectorOpen()}
            consoleOpen={consoleState() !== "folded"}
            onToggleInspector={() => setInspectorOpen((v) => !v)}
            onToggleConsole={() => setConsoleState((s) => (s === "folded" ? "default" : "folded"))}
            onDisconnect={() => { setSources([]); setSelectedTable(null); setResult(null); setError(null); }}
          />

          <main class="main">
            <SqlEditor
              initialSql={sql()}
              rowCap={rowCap()}
              busy={busy()}
              onSql={setSql}
              onRowCap={setRowCap}
              onRun={run}
              onCopy={copySql}
            />
            <Show when={error()}>
              <pre class="error-box">{error()}</pre>
            </Show>
            <ResultTable result={result()} />
          </main>

          <Show when={inspectorOpen()}>
            <RightInspector
              table={selectedTable()}
              busy={busy()}
              onInjectSql={injectSql}
              onPreview={previewTable}
            />
          </Show>

          <BottomConsole
            logs={logs()}
            state={consoleState()}
            onCycleState={cycleConsole}
            onClear={() => setLogs([])}
          />
        </>
      }>
        <SettingsPage onClose={() => setSettingsOpen(false)} />
      </Show>
    </div>
  );
}
