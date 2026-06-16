import { createSignal, Show, Switch, Match } from "solid-js";
import DropZone from "./components/DropZone";
import LeftNav from "./components/LeftNav";
import TitleBar from "./components/TitleBar";
import SqlEditor from "./components/SqlEditor";
import ResultTable from "./components/ResultTable";
import RightInspector from "./components/RightInspector";
import BottomConsole, { type ConsoleState } from "./components/BottomConsole";
import SettingsPage from "./components/SettingsPage";
import HomePanel from "./components/HomePanel";
import { executeSql, importFileToWorkspace } from "./lib/duckdb";
import type { LogEntry, SourceTable, SqlResult, QueryTask, Workspace, TaskKind, ChatMessage } from "./lib/types";
import { mockAgentReply } from "./lib/mock";
import ChatView from "./components/ChatView";
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

  // --- workspaces & tasks ---
  const [workspaces, setWorkspaces] = createSignal<Workspace[]>([
    { name: "DefaultProject", path: "DefaultProject" }
  ]);
  const [currentWorkspace, setCurrentWorkspace] = createSignal<Workspace>({
    name: "DefaultProject",
    path: "DefaultProject"
  });
  const [tasks, setTasks] = createSignal<QueryTask[]>([]);
  const [activeTaskId, setActiveTaskId] = createSignal<string | null>(null);

  function addWorkspace(path: string) {
    if (!path) return;
    // Extract folder name from absolute path
    const name = path.split(/[\\/]/).filter(Boolean).pop() || path;

    setWorkspaces((prev) => {
      if (prev.some((w) => w.path === path)) return prev;
      return [...prev, { name, path }];
    });

    const ws = { name, path };
    setCurrentWorkspace(ws);
  }

  function selectWorkspace(path: string) {
    const ws = workspaces().find((w) => w.path === path);
    if (ws) {
      setCurrentWorkspace(ws);
    }
  }

  function removeWorkspace(path: string) {
    const list = workspaces();
    const nextList = list.filter((w) => w.path !== path);
    
    if (nextList.length === 0) {
      const def = { name: "DefaultProject", path: "DefaultProject" };
      setWorkspaces([def]);
      setCurrentWorkspace(def);
      return;
    }
    
    setWorkspaces(nextList);
    if (currentWorkspace().path === path) {
      setCurrentWorkspace(nextList[0]);
    }
  }

  function createTask(prompt: string, kind: TaskKind = "chat") {
    const id = `task-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    let name = prompt.trim();
    if (name.length > 20) {
      name = name.slice(0, 18) + "...";
    }
    name = name.replace(/\n/g, " ");
    if (!name) name = kind === "chat" ? "新对话" : "新查询";

    const newTask: QueryTask = {
      id,
      name,
      sql: kind === "sql" ? prompt.trim() : "",
      createdAt: Date.now(),
      kind,
      messages: kind === "chat" ? [] : undefined,
    };

    setTasks((prev) => [...prev, newTask]);
    setActiveTaskId(id);
    if (kind === "sql") {
      setSql(newTask.sql);
      setResult(null);
      setError(null);
    }
  }

  function selectTask(id: string) {
    const task = tasks().find((t) => t.id === id);
    if (!task) return;
    setActiveTaskId(id);
    if ((task.kind ?? "sql") === "sql") {
      setSql(task.sql);
      setResult(null);
      setError(null);
    }
    // chat task：消息由 task 自带，主区读 task.messages 渲染，无需 injectSql。
  }

  /** 当前激活的 task（派生值）。 */
  function activeTask(): QueryTask | null {
    const id = activeTaskId();
    if (!id) return null;
    return tasks().find((t) => t.id === id) ?? null;
  }

  /** ChatView 发送消息：追加 user 消息 → Mock 回复 → 追加 assistant 消息。 */
  async function sendChatMessage(prompt: string) {
    const id = activeTaskId();
    if (!id) return;
    const userMsg: ChatMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      content: prompt,
      ts: Date.now(),
    };
    setTasks((prev) =>
      prev.map((t) =>
        t.id === id ? { ...t, messages: [...(t.messages ?? []), userMsg] } : t,
      ),
    );
    // MOCK：M2 接真 Agent 时替换 mockAgentReply。
    const reply = await mockAgentReply(prompt);
    setTasks((prev) =>
      prev.map((t) =>
        t.id === id ? { ...t, messages: [...(t.messages ?? []), reply] } : t,
      ),
    );
  }

  /** ChatCard「在 SQL 面板打开」：新建 SQL task 注入 SQL 并自动执行。 */
  function openInSqlPanel(sql: string) {
    createTask(sql, "sql");
    void run();
  }

  function deleteTask(id: string) {
    setTasks((prev) => prev.filter((t) => t.id !== id));
    if (activeTaskId() === id) {
      setActiveTaskId(null);
    }
  }

  function handleSqlChange(newSql: string) {
    setSql(newSql);
    const activeId = activeTaskId();
    if (activeId) {
      setTasks((prev) => prev.map((t) => (t.id === activeId ? { ...t, sql: newSql } : t)));
    }
  }

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
      const scale = consoleState() === "expanded" ? 1.8 : 1.0;
      const newHeight = Math.max(80, Math.min(600, startHeight - deltaY / scale));
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

  async function handleDropFiles(paths: string[]) {
    if (busy()) return;
    setBusy(true);
    setError(null);
    try {
      for (const p of paths) {
        const res = await importFileToWorkspace(currentWorkspace().path, p);
        mergeSources(res);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleImportFile(filePath: string) {
    if (busy()) return;
    setBusy(true);
    setError(null);
    try {
      const res = await importFileToWorkspace(currentWorkspace().path, filePath);
      mergeSources(res);
      if (res.length > 0) {
        selectTable(res[0]);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  async function handleSelectAndRegisterSource() {
    if (busy()) return;
    try {
      const { selectDirectory } = await import("./lib/duckdb");
      const path = await selectDirectory();
      if (path) {
        setBusy(true);
        setError(null);
        const res = await importFileToWorkspace(currentWorkspace().path, path);
        mergeSources(res);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
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

    const active = activeTask();
    if (!active || active.kind !== "sql") {
      createTask(s, "sql");
    } else {
      const activeId = activeTaskId();
      if (activeId) {
        setTasks((prev) => prev.map((t) => (t.id === activeId ? { ...t, sql: s } : t)));
      }
    }
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
      <DropZone workspace={currentWorkspace().path} busy={busy()} onDropFiles={handleDropFiles} />

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
        onNewQuery={() => createTask("SELECT 1 AS n;", "sql")}
        selectedTable={selectedTable()}
        onOpenSettings={onSettings}
      />

      <Show when={settingsOpen()} fallback={
        <>
          <LeftNav
            workspace={currentWorkspace().name}
            workspacePath={currentWorkspace().path}
            workspaces={workspaces()}
            tasks={tasks()}
            activeTaskId={activeTaskId()}
            onSelectTask={selectTask}
            onDeleteTask={deleteTask}
            onSelectWorkspace={selectWorkspace}
            onRemoveWorkspace={removeWorkspace}
            onAddWorkspace={addWorkspace}
            onImportFile={handleImportFile}
            sources={sources()}
            selected={selectedTable()?.name ?? null}
            busy={busy()}
            onSelect={selectTable}
            onOpenSettings={onSettings}
            onNewQuery={() => createTask("SELECT 1 AS n;", "sql")}
            inspectorOpen={inspectorOpen()}
            consoleOpen={consoleState() !== "folded"}
            onToggleInspector={() => setInspectorOpen((v) => !v)}
            onToggleConsole={() => setConsoleState((s) => (s === "folded" ? "default" : "folded"))}
            onDisconnect={() => { setSources([]); setSelectedTable(null); setResult(null); setError(null); setTasks([]); setActiveTaskId(null); }}
          />

          <main class="main">
            <Show
              when={activeTaskId() !== null}
              fallback={
                <HomePanel
                  workspace={currentWorkspace().name}
                  workspaces={workspaces()}
                  onSelectWorkspace={selectWorkspace}
                  onAddWorkspace={addWorkspace}
                  onCreateTask={(prompt) => createTask(prompt, "chat")}
                  onAddSource={handleSelectAndRegisterSource}
                />
              }
            >
              {/* activeTaskId 非空时，按 task.kind 在 ChatView 与 SqlEditor 间切换。
                  SqlEditor 通过自身的 createEffect 同步 initialSql，
                  切换 SQL task 时编辑器内容会正确更新，无需 keyed 重建。 */}
              <Switch>
                <Match when={(activeTask()?.kind ?? "sql") === "chat"}>
                  <ChatView
                    messages={activeTask()?.messages ?? []}
                    workspace={currentWorkspace().name}
                    onSend={sendChatMessage}
                    onOpenInSqlPanel={openInSqlPanel}
                  />
                </Match>
                <Match when={(activeTask()?.kind ?? "sql") === "sql"}>
                  <div class="sql-view">
                    <SqlEditor
                      initialSql={sql()}
                      rowCap={rowCap()}
                      busy={busy()}
                      onSql={handleSqlChange}
                      onRowCap={setRowCap}
                      onRun={run}
                      onCopy={copySql}
                    />
                    <Show when={error()}>
                      <pre class="error-box">{error()}</pre>
                    </Show>
                    <ResultTable result={result()} />
                  </div>
                </Match>
              </Switch>
            </Show>
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
