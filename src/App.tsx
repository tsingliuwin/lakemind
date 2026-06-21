import { createSignal, Show, Switch, Match, onMount, onCleanup, createEffect } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
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
  const [leftOpen, setLeftOpen] = createSignal<boolean>(true);

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
  const [fileTrigger, setFileTrigger] = createSignal<number>(0);

  // --- model settings sync ---
  const [availableModels, setAvailableModels] = createSignal<string[]>([]);
  const [selectedModel, setSelectedModel] = createSignal<string>("");
  const [selectedPriority, setSelectedPriority] = createSignal<string>("最高");

  async function loadModelsFromSettings() {
    try {
      const json = await invoke<string>("load_settings_json");
      if (json && json !== "{}") {
        const loaded = JSON.parse(json);
        const models: string[] = [];
        if (loaded.providers) {
          for (const prov of loaded.providers) {
            if (prov.enabled && prov.models) {
              for (const m of prov.models) {
                models.push(m.id);
              }
            }
          }
        }
        setAvailableModels(models);
        if (models.length > 0) {
          if (!selectedModel() || !models.includes(selectedModel())) {
            setSelectedModel(models[0]);
          }
        } else {
          setSelectedModel("");
        }
      }
    } catch (err) {
      console.error("Failed to load settings models:", err);
    }
  }

  createEffect(() => {
    if (!settingsOpen()) {
      void loadModelsFromSettings();
    }
  });

  onMount(async () => {
    void loadModelsFromSettings();

    const unlistenAgent = await listen<any>("agent-event", (event) => {
      const payload = event.payload;
      const targetId = payload.taskId;
      
      setTasks((prev) =>
        prev.map((t) => {
          if (t.id !== targetId) return t;
          
          let messages = [...(t.messages ?? [])];
          if (messages.length === 0) return t;
          
          let lastMsg = messages[messages.length - 1];
          if (lastMsg.role !== "assistant") {
            lastMsg = {
              id: `msg-assistant-${Date.now()}`,
              role: "assistant",
              content: "",
              cards: [],
              ts: Date.now(),
            };
            messages.push(lastMsg);
          } else {
            lastMsg = { ...lastMsg, cards: lastMsg.cards ? [...lastMsg.cards] : [] };
            messages[messages.length - 1] = lastMsg;
          }
          
          if (payload.kind === "text") {
            lastMsg.content += payload.text ?? "";
          } else if (payload.kind === "reasoning") {
            lastMsg.reasoning = (lastMsg.reasoning ?? "") + (payload.text ?? "");
          } else if (payload.kind === "phase" && payload.phase) {
            lastMsg.phase = payload.phase;
          } else if (payload.kind === "error") {
            lastMsg.content += `\n\n⚠️ **错误**: ${payload.text ?? "未知错误"}`;
          } else if (payload.kind === "card" && payload.card) {
            const card = payload.card;
            if (!lastMsg.cards) lastMsg.cards = [];
            const existingIdx = lastMsg.cards.findIndex(c => c.id === card.id);
            if (existingIdx >= 0) {
              lastMsg.cards[existingIdx] = card;
            } else {
              lastMsg.cards.push(card);
            }
          }
          
          if (payload.kind === "done" || payload.kind === "error") {
            void saveChatTaskBackend(targetId, t.name, messages);
          }
          
          return { ...t, messages };
        })
      );
    });

    try {
      const list = await invoke<Workspace[]>("load_workspaces");
      if (list && list.length > 0) {
        setWorkspaces(list);
        const defaultWS = list.find((w) => w.path === "DefaultProject") || list[0];
        setCurrentWorkspace(defaultWS);
      }
    } catch (err) {
      console.error("Failed to load workspaces:", err);
    }

    const handleGlobalKeyDown = (e: KeyboardEvent) => {
      const isN = e.key.toLowerCase() === "n";
      const isS = e.key.toLowerCase() === "s";
      const isCtrlOrCmd = e.ctrlKey || e.metaKey;
      if (isCtrlOrCmd && isN) {
        e.preventDefault();
        if (e.shiftKey) {
          createTask("", "chat");
        } else {
          createTask("SELECT 1 AS n;", "sql");
        }
      } else if (isCtrlOrCmd && isS) {
        e.preventDefault();
        saveActiveTask();
      }
    };
    window.addEventListener("keydown", handleGlobalKeyDown);
    onCleanup(() => {
      window.removeEventListener("keydown", handleGlobalKeyDown);
      unlistenAgent();
    });
  });

  // Track workspace change to load its tasks and scan source files
  createEffect(async () => {
    const ws = currentWorkspace();
    if (!ws) return;
    setBusy(true);
    setError(null);
    try {
      // 1. Load tasks
      const loadedTasks = await invoke<QueryTask[]>("load_workspace_tasks", { workspacePath: ws.path });
      setTasks(loadedTasks);
      
      if (loadedTasks.length > 0) {
        const activeId = activeTaskId();
        if (activeId && loadedTasks.some(t => t.id === activeId)) {
          selectTask(activeId);
        } else {
          selectTask(loadedTasks[0].id);
        }
      } else {
        setActiveTaskId(null);
        setSql("SELECT 1 AS n;");
        setResult(null);
        setError(null);
      }

      // 2. Scan and register files in workspace, then load all DuckDB tables
      await invoke<SourceTable[]>("register_workspace_sources", { workspacePath: ws.path });
      const dbTables = await invoke<SourceTable[]>("list_duckdb_tables");
      setSources(dbTables);
      setSelectedTable(null);
    } catch (err) {
      console.error("Failed to load workspace tasks & sources:", err);
      setError(String(err));
    } finally {
      setBusy(false);
    }
  });

  async function addWorkspace(path: string) {
    if (!path) return;
    const name = path.split(/[\\/]/).filter(Boolean).pop() || path;

    try {
      await invoke("add_workspace", { name, path });
      setWorkspaces((prev) => {
        if (prev.some((w) => w.path === path)) return prev;
        return [...prev, { name, path }];
      });
      const ws = { name, path };
      setCurrentWorkspace(ws);
    } catch (err) {
      console.error("Failed to add workspace:", err);
    }
  }

  function selectWorkspace(path: string) {
    const ws = workspaces().find((w) => w.path === path);
    if (ws) {
      setCurrentWorkspace(ws);
    }
  }

  async function removeWorkspace(path: string) {
    try {
      await invoke("remove_workspace", { path });
      const list = workspaces();
      const nextList = list.filter((w) => w.path !== path);
      
      if (nextList.length === 0) {
        const def = { name: "DefaultProject", path: "DefaultProject" };
        await invoke("add_workspace", def);
        setWorkspaces([def]);
        setCurrentWorkspace(def);
        return;
      }
      
      setWorkspaces(nextList);
      if (currentWorkspace().path === path) {
        setCurrentWorkspace(nextList[0]);
      }
    } catch (err) {
      console.error("Failed to remove workspace:", err);
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
      modelId: kind === "chat" ? selectedModel() : undefined,
      saved: false,
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

  async function saveChatTaskBackend(taskId: string, name: string, messages: ChatMessage[]) {
    try {
      await invoke("save_chat_task", {
        workspacePath: currentWorkspace().path,
        taskId,
        name,
        messages,
      });
    } catch (err) {
      console.error("Failed to save chat task to backend:", err);
    }
  }

  /** ChatView 发送消息：追加 user 消息 → 触发 Rust Agent 循环。 */
  async function sendChatMessage(prompt: string) {
    const id = activeTaskId();
    if (!id) return;
    const task = tasks().find((t) => t.id === id);
    if (!task) return;

    if (availableModels().length === 0) {
      alert("请先前往设置中心（右上角菜单 -> 模型设置中心）配置并启用大模型供应商及模型。");
      return;
    }

    const userMsg: ChatMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      content: prompt,
      ts: Date.now(),
    };
    
    const updatedMessages = [...(task.messages ?? []), userMsg];
    setTasks((prev) =>
      prev.map((t) =>
        t.id === id ? { ...t, messages: updatedMessages } : t,
      ),
    );
    await saveChatTaskBackend(id, task.name, updatedMessages);

    try {
      const activeModel = task.modelId || selectedModel();
      const historyToSend = task.messages ?? [];
      const historyJson = JSON.stringify(historyToSend);
      
      await invoke("start_agent_chat", {
        taskId: id,
        modelId: activeModel,
        prompt,
        historyJson,
        priority: selectedPriority(),
      });
    } catch (err) {
      console.error("Failed to start agent chat:", err);
      const errorMsg: ChatMessage = {
        id: `msg-err-${Date.now()}`,
        role: "assistant",
        content: `⚠️ **无法启动对话**: ${err}`,
        ts: Date.now(),
      };
      setTasks((prev) =>
        prev.map((t) =>
          t.id === id ? { ...t, messages: [...updatedMessages, errorMsg] } : t,
        ),
      );
    }
  }

  async function createChatTaskAndSend(prompt: string, modelId?: string) {
    const id = `task-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    let name = prompt.trim();
    if (name.length > 20) {
      name = name.slice(0, 18) + "...";
    }
    name = name.replace(/\n/g, " ");
    if (!name) name = "新对话";

    if (availableModels().length === 0) {
      alert("请先前往设置中心（右上角菜单 -> 模型设置中心）配置并启用大模型供应商及模型。");
      return;
    }

    const userMsg: ChatMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      content: prompt,
      ts: Date.now(),
    };

    const targetModel = modelId || selectedModel();

    const newTask: QueryTask = {
      id,
      name,
      sql: "",
      createdAt: Date.now(),
      kind: "chat",
      messages: [userMsg],
      modelId: targetModel,
      saved: false,
    };

    setTasks((prev) => [...prev, newTask]);
    setActiveTaskId(id);
    await saveChatTaskBackend(id, name, [userMsg]);

    try {
      const historyJson = JSON.stringify([]);
      await invoke("start_agent_chat", {
        taskId: id,
        modelId: targetModel,
        prompt,
        historyJson,
        priority: selectedPriority(),
      });
    } catch (err) {
      console.error("Failed to start agent chat:", err);
      const errorMsg: ChatMessage = {
        id: `msg-err-${Date.now()}`,
        role: "assistant",
        content: `⚠️ **无法启动对话**: ${err}`,
        ts: Date.now(),
      };
      setTasks((prev) =>
        prev.map((t) =>
          t.id === id ? { ...t, messages: [userMsg, errorMsg] } : t,
        ),
      );
    }
  }

  async function sendChatMessageFromHome(id: string, prompt: string, modelId?: string) {
    const userMsg: ChatMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      content: prompt,
      ts: Date.now(),
    };

    let name = prompt.trim();
    if (name.length > 20) {
      name = name.slice(0, 18) + "...";
    }
    name = name.replace(/\n/g, " ");

    if (availableModels().length === 0) {
      alert("请先前往设置中心（右上角菜单 -> 模型设置中心）配置并启用大模型供应商及模型。");
      return;
    }

    const targetModel = modelId || selectedModel();

    setTasks((prev) =>
      prev.map((t) =>
        t.id === id
          ? {
              ...t,
              name,
              messages: [userMsg],
              modelId: targetModel,
            }
          : t
      )
    );
    await saveChatTaskBackend(id, name, [userMsg]);

    try {
      const historyJson = JSON.stringify([]);
      await invoke("start_agent_chat", {
        taskId: id,
        modelId: targetModel,
        prompt,
        historyJson,
        priority: selectedPriority(),
      });
    } catch (err) {
      console.error("Failed to start agent chat:", err);
      const errorMsg: ChatMessage = {
        id: `msg-err-${Date.now()}`,
        role: "assistant",
        content: `⚠️ **无法启动对话**: ${err}`,
        ts: Date.now(),
      };
      setTasks((prev) =>
        prev.map((t) =>
          t.id === id ? { ...t, messages: [userMsg, errorMsg] } : t,
        ),
      );
    }
  }

  /** ChatCard「在 SQL 面板打开」：新建 SQL task 注入 SQL 并自动执行。 */
  function openInSqlPanel(sql: string) {
    createTask(sql, "sql");
    void run();
  }

  async function deleteTask(id: string) {
    setTasks((prev) => prev.filter((t) => t.id !== id));
    if (activeTaskId() === id) {
      setActiveTaskId(null);
    }
    try {
      await invoke("delete_task", { taskId: id });
    } catch (err) {
      console.error("Failed to delete task:", err);
    }
  }

  async function saveActiveTask() {
    const activeId = activeTaskId();
    if (!activeId) return;
    const task = tasks().find((t) => t.id === activeId);
    if (!task || task.kind !== "sql") return;

    const isDefaultOrEmpty = !task.sql.trim() || task.sql.trim() === "SELECT 1 AS n;";
    if (isDefaultOrEmpty) {
      alert("默认查询或空文件无需保存！");
      return;
    }

    let name = task.sql.trim();
    const lines = name.split("\n");
    name = lines[0].trim();
    if (name.length > 20) {
      name = name.slice(0, 18) + "...";
    }
    if (!name) name = "已保存查询";

    setTasks((prev) =>
      prev.map((t) =>
        t.id === activeId ? { ...t, name, saved: true } : t
      )
    );

    try {
      await invoke("save_sql_task", {
        workspacePath: currentWorkspace().path,
        taskId: activeId,
        name,
        sql: task.sql,
      });
    } catch (err) {
      console.error("Failed to save SQL task:", err);
    }
  }

  const visibleTasks = () => {
    return tasks().filter((task) => {
      if (task.kind === "chat") {
        return (task.messages?.length ?? 0) > 0;
      } else {
        const isDefaultOrEmpty = !task.sql.trim() || task.sql.trim() === "SELECT 1 AS n;";
        return !!task.saved && !isDefaultOrEmpty;
      }
    });
  };

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

  async function handleDropFiles(paths: string[]) {
    if (busy()) return;
    setBusy(true);
    setError(null);
    try {
      let imported = false;
      for (const p of paths) {
        const res = await importFileToWorkspace(currentWorkspace().path, p);
        if (res.length > 0) imported = true;
      }
      if (imported) {
        const dbTables = await invoke<SourceTable[]>("list_duckdb_tables");
        setSources(dbTables);
        setFileTrigger((t) => t + 1);
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
      if (res.length > 0) {
        const dbTables = await invoke<SourceTable[]>("list_duckdb_tables");
        setSources(dbTables);
        const newTable = dbTables.find(t => res.some(r => r.name === t.name));
        if (newTable) {
          selectTable(newTable);
        }
        setFileTrigger((t) => t + 1);
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
        if (res.length > 0) {
          const dbTables = await invoke<SourceTable[]>("list_duckdb_tables");
          setSources(dbTables);
          setFileTrigger((t) => t + 1);
        }
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

      // Refresh the table list in case the query changed schemas. Non-fatal:
      // a failure here must NOT clobber the query result we just set.
      try {
        const dbTables = await invoke<SourceTable[]>("list_duckdb_tables");
        setSources(dbTables);
      } catch (refreshErr) {
        console.error("refresh table list failed:", refreshErr);
      }
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

  /** 点击侧栏表：选中并在右侧检查器展示其 schema，同时自动执行 LIMIT 50 预览查询。 */
  function selectTable(t: SourceTable) {
    setSelectedTable(t);
    setInspectorOpen(true);
    previewTable(t);
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
      classList={{ 
        "app-shell": true, 
        "no-inspector": !inspectorOpen(),
        "sidebar-collapsed": !leftOpen()
      }}
      style={{
        "--left-width": `${leftWidth()}px`,
        "--right-width": `${rightWidth()}px`,
        "--right-width-actual": rightWidthActual(),
        "--bottom-height": `${bottomHeight()}px`,
        "--bottom-height-actual": bottomHeightActual()
      }}
    >
      <DropZone workspace={currentWorkspace().path} busy={busy()} onDropFiles={handleDropFiles} />
      <Show 
        when={!settingsOpen()}
        fallback={
          <SettingsPage 
            onClose={() => setSettingsOpen(false)} 
            titleBar={
              <TitleBar
                inspectorOpen={inspectorOpen()}
                consoleOpen={consoleState() !== "folded"}
                onToggleInspector={() => setInspectorOpen((v) => !v)}
                onToggleConsole={() => setConsoleState((s) => (s === "folded" ? "default" : "folded"))}
                onNewQuery={() => createTask("SELECT 1 AS n;", "sql")}
                selectedTable={selectedTable()}
                onOpenSettings={onSettings}
                busy={busy()}
                leftOpen={leftOpen()}
                onToggleLeft={() => setLeftOpen(!leftOpen())}
                hideLayoutToggles={true}
              />
            }
          />
        }
      >
        <Show when={leftOpen()}>
          <div 
            class="resizer-v" 
            classList={{ dragging: isDraggingLeft() }}
            style={{ left: `${leftWidth() - 3}px`, top: 0, height: "100vh" }} 
            onMouseDown={startDraggingLeft}
          />
        </Show>

        <LeftNav
          workspace={currentWorkspace().name}
          workspacePath={currentWorkspace().path}
          workspaces={workspaces()}
          tasks={visibleTasks()}
          activeTaskId={activeTaskId()}
          onSelectTask={selectTask}
          onDeleteTask={deleteTask}
          onSelectWorkspace={selectWorkspace}
          onRemoveWorkspace={removeWorkspace}
          onAddWorkspace={addWorkspace}
          onImportFile={handleImportFile}
          sources={sources()}
          fileTrigger={fileTrigger()}
          selected={selectedTable()?.name ?? null}
          busy={busy()}
          onSelect={selectTable}
          onOpenSettings={onSettings}
          onNewQuery={() => createTask("SELECT 1 AS n;", "sql")}
          onNewChat={() => createTask("", "chat")}
          onDisconnect={() => { setSources([]); setSelectedTable(null); setResult(null); setError(null); setTasks([]); setActiveTaskId(null); }}
          leftOpen={leftOpen()}
          onToggleLeft={() => setLeftOpen(!leftOpen())}
        />

        <div class="right-container">
          <TitleBar
            inspectorOpen={inspectorOpen()}
            consoleOpen={consoleState() !== "folded"}
            onToggleInspector={() => setInspectorOpen((v) => !v)}
            onToggleConsole={() => setConsoleState((s) => (s === "folded" ? "default" : "folded"))}
            onNewQuery={() => createTask("SELECT 1 AS n;", "sql")}
            selectedTable={selectedTable()}
            onOpenSettings={onSettings}
            busy={busy()}
            leftOpen={leftOpen()}
            onToggleLeft={() => setLeftOpen(!leftOpen())}
          />

          <div class="right-content-layout">
            <Show when={inspectorOpen()}>
              <div 
                class="resizer-v" 
                classList={{ dragging: isDraggingRight() }}
                style={{ right: `${rightWidth() - 3}px` }} 
                onMouseDown={startDraggingRight}
              />
            </Show>

            <Show when={consoleState() !== "folded"}>
              <div 
                class="resizer-h" 
                classList={{ dragging: isDraggingBottom() }}
                style={{ 
                  bottom: `${parseFloat(bottomHeightActual()) - 3}px`,
                  left: 0,
                  right: rightWidthActual()
                }} 
                onMouseDown={startDraggingBottom}
              />
            </Show>

            <main class="main">
              <Show
                when={activeTaskId() !== null && (activeTask()?.kind !== "chat" || (activeTask()?.messages?.length ?? 0) > 0)}
                fallback={
                  <HomePanel
                    workspace={currentWorkspace().name}
                    workspaces={workspaces()}
                    onSelectWorkspace={selectWorkspace}
                    onAddWorkspace={addWorkspace}
                    onCreateTask={(prompt, modelId) => {
                      const active = activeTask();
                      if (active && active.kind === "chat" && (active.messages?.length ?? 0) === 0) {
                        void sendChatMessageFromHome(active.id, prompt, modelId);
                      } else {
                        void createChatTaskAndSend(prompt, modelId);
                      }
                    }}
                    onAddSource={handleSelectAndRegisterSource}
                    availableModels={availableModels()}
                    selectedModel={selectedModel()}
                    onSelectModel={setSelectedModel}
                    selectedPriority={selectedPriority()}
                    onSelectPriority={setSelectedPriority}
                  />
                }
              >
                <Switch>
                  <Match when={(activeTask()?.kind ?? "sql") === "chat"}>
                    <ChatView
                      messages={activeTask()?.messages ?? []}
                      workspace={currentWorkspace().name}
                      onSend={sendChatMessage}
                      onOpenInSqlPanel={openInSqlPanel}
                      availableModels={availableModels()}
                      selectedModel={activeTask()?.modelId || selectedModel()}
                      onSelectModel={(model) => {
                        const activeId = activeTaskId();
                        if (activeId) {
                          setTasks((prev) =>
                            prev.map((t) =>
                              t.id === activeId ? { ...t, modelId: model } : t
                            )
                          );
                        }
                        setSelectedModel(model);
                      }}
                      selectedPriority={selectedPriority()}
                      onSelectPriority={setSelectedPriority}
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
                        onSave={saveActiveTask}
                        onClose={() => deleteTask(activeTaskId()!)}
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
          </div>
        </div>
      </Show>
    </div>
  );
}
