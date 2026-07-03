import { createSignal, Show, Switch, Match, onMount, onCleanup, createEffect, createMemo, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import DropZone from "./components/DropZone";
import LeftNav from "./components/LeftNav";
import TitleBar from "./components/TitleBar";
import SqlEditor from "./components/SqlEditor";
import ResultTable from "./components/ResultTable";
import RightInspector from "./components/RightInspector";
import BottomConsole, { type ConsoleState } from "./components/BottomConsole";
import SettingsPage, { type SettingsTab } from "./components/SettingsPage";
import HomePanel from "./components/HomePanel";
import { executeSql, importFileToWorkspace, selectDirectory, selectFile } from "./lib/duckdb";
import { tryFormatDuckdbSql } from "./lib/sqlFormat";
import type { LogEntry, SourceTable, SqlResult, QueryTask, Workspace, TaskKind, ChatMessage, RegisterStatus, ImportProgress, DepInfo } from "./lib/types";
import ChatView from "./components/ChatView";
import { appendDelta, pushToolCall, pushChart, mergeToolResult, normalizeMessage } from "./lib/chat";
import { mergeUsage } from "./lib/metrics";
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
  // File-registration coverage of the active workspace → colored dot by the
  // project name (all/partial/none). Refreshed after workspace switch + import.
  const [registerStatus, setRegisterStatus] = createSignal<RegisterStatus>("all");
  // Current file-import progress (null = idle). Driven by "import-progress" events.
  const [importStatus, setImportStatus] = createSignal<ImportProgress | null>(null);
  // Dependency info for the selected table (upstreams + downstreams).
  const [deps, setDeps] = createSignal<DepInfo | null>(null);
  /** Fetch registration coverage for a workspace and update the status dot. */
  async function refreshRegisterStatus(wsPath: string) {
    try {
      const r = await invoke<{ status: string }>("workspace_register_status", { workspacePath: wsPath });
      setRegisterStatus(r.status as RegisterStatus);
    } catch (e) {
      console.error("refresh register status failed:", e);
    }
  }

  // --- 编辑器 / 查询 ---
  const [sql, setSql] = createSignal<string>("SELECT 1 AS n;");
  const [rowCap, setRowCap] = createSignal<number>(10_000);
  interface QueryResultTab {
    id: string;
    name: string;
    sql: string;
    result: SqlResult | null;
    error: string | null;
    timestamp: number;
  }
  const [taskTabs, setTaskTabs] = createSignal<Record<string, QueryResultTab[]>>({});
  const [taskActiveTabId, setTaskActiveTabId] = createSignal<Record<string, string | null>>({});
  const [editingTabId, setEditingTabId] = createSignal<string | null>(null);
  const [editingText, setEditingText] = createSignal<string>("");
  const [busy, setBusy] = createSignal<boolean>(false);

  // --- drawers & layout sizes ---
  const [inspectorOpen, setInspectorOpen] = createSignal<boolean>(false);
  const [consoleState, setConsoleState] = createSignal<ConsoleState>("folded");
  const [settingsOpen, setSettingsOpen] = createSignal<boolean>(false);
  // Sub-section to land on when settings opens (e.g. "databases" from the
  // workspace sidebar empty-state link). Reset on close so the next generic
  // open falls back to the default "general" tab.
  const [settingsTab, setSettingsTab] = createSignal<SettingsTab>("general");
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
  const [modelCtxWindows, setModelCtxWindows] = createSignal<Record<string, number>>({});
  const [selectedModel, setSelectedModel] = createSignal<string>("");
  const [selectedPriority, setSelectedPriority] = createSignal<string>("最高");
  const [selectedConfirm, setSelectedConfirm] = createSignal<string>("变更前确认");
  // 当前正在流式输出的对话任务 id。start_agent_chat 是 fire-and-forget
  // （tokio::spawn 后立即返回），真正的流式通过 agent-event 异步回来，
  // 所以用一个独立信号准确跟踪执行状态，供 ChatView 派生 streaming。
  const [streamingTaskId, setStreamingTaskId] = createSignal<string | null>(null);

  function changeWorkspace(ws: Workspace) {
    // Clear workspace-specific states synchronously to avoid showing stale data from
    // the previous workspace during the async load phase.
    setSources([]);
    setTasks([]);
    setSelectedTable(null);
    setActiveTaskId(null);
    setDeps(null);
    setImportStatus(null);
    setStreamingTaskId(null);
    setCurrentWorkspace(ws);
    // Remember the last workspace so the next launch reopens it instead of
    // always falling back to DefaultProject. Fire-and-forget; the config store
    // is the app's existing key/value persistence layer.
    invoke("set_app_config", { key: "workspace.last", value: ws.path }).catch((e) =>
      console.error("Failed to persist last workspace:", e)
    );
  }


  const currentTabs = createMemo<QueryResultTab[]>(() => {
    const taskId = activeTaskId();
    if (!taskId) return [];
    return taskTabs()[taskId] || [];
  });

  const currentActiveTabId = createMemo<string | null>(() => {
    const taskId = activeTaskId();
    if (!taskId) return null;
    return taskActiveTabId()[taskId] || null;
  });

  const activeTab = createMemo<QueryResultTab | null>(() => {
    const tabId = currentActiveTabId();
    if (!tabId) return null;
    return currentTabs().find((t: QueryResultTab) => t.id === tabId) ?? null;
  });

  // Ref to the scrollable result-tab bar; used to keep the active/new tab visible.
  let resultTabsBarRef: HTMLDivElement | undefined;
  // Track refs of each tab item so we can scrollIntoView the active one.
  const tabItemRefs = new Map<string, HTMLDivElement>();

  // Whenever the active tab changes, scroll it into view (covers click / new-result / close cases).
  createEffect(() => {
    const id = currentActiveTabId();
    if (!id) return;
    const el = tabItemRefs.get(id);
    if (el) {
      // 'nearest' avoids jumping when the tab is already partially visible.
      el.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  });

  async function loadModelsFromSettings() {
    try {
      const json = await invoke<string>("load_settings_json");
      if (json && json !== "{}") {
        const loaded = JSON.parse(json);
        const models: string[] = [];
        const ctxMap: Record<string, number> = {};
        if (loaded.providers) {
          for (const prov of loaded.providers) {
            if (prov.enabled && prov.models) {
              for (const m of prov.models) {
                models.push(m.id);
                if (m.contextWindow) ctxMap[m.id] = m.contextWindow;
              }
            }
          }
        }
        setAvailableModels(models);
        setModelCtxWindows(ctxMap);
        
        const savedDefault = localStorage.getItem("default_model");
        if (models.length > 0) {
          if (savedDefault && models.includes(savedDefault)) {
            setSelectedModel(savedDefault);
          } else if (!selectedModel() || !models.includes(selectedModel())) {
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

    // File-import progress: update the status signal in real time AND log every
    // stage to the bottom console so the whole process is inspectable later.
    const unlistenImport = await listen<ImportProgress>("import-progress", (event) => {
      const p = event.payload;
      setImportStatus(p);

      // Log every stage to the console.
      const stageText: Record<string, string> = {
        copying: "复制文件",
        scanning: "扫描",
        registering: p.table ? `映射为表 ${p.table}` : "映射",
        done: `完成 → ${p.table ?? ""}（${p.columns ?? 0} 列${p.rows != null ? `, ${p.rows} 行` : ""}）`,
        error: `失败：${p.error ?? "未知错误"}`,
      };
      const desc = `导入 ${p.file} · ${stageText[p.stage] ?? p.stage}`;
      setLogs((prev) => [{
        id: ++logSeq,
        ts: Date.now(),
        sql: desc,
        status: p.stage === "error" ? "error" as const : "ok" as const,
        rowCount: p.stage === "done" ? (p.rows ?? undefined) : undefined,
        error: p.stage === "error" ? (p.error ?? undefined) : undefined,
      }, ...prev].slice(0, 100));

      if (p.stage === "done" || p.stage === "error") {
        const clearMs = p.stage === "done" ? 3000 : 8000;
        setTimeout(() => {
          setImportStatus((cur) => (cur === p ? null : cur));
        }, clearMs);
      }
    });

    const unlistenAgent = await listen<any>("agent-event", (event) => {
      const payload = event.payload;
      const targetId = payload.taskId;

      setTasks((prev) =>
        prev.map((t) => {
          if (t.id !== targetId) return t;

          let messages = [...(t.messages ?? [])];

          // Ensure an assistant message is the last one (lazily created).
          let lastMsg = messages[messages.length - 1];
          if (!lastMsg || lastMsg.role !== "assistant") {
            lastMsg = {
              id: `msg-assistant-${Date.now()}`,
              role: "assistant",
              segments: [],
              ts: Date.now(),
            };
            messages = [...messages, lastMsg];
          }

          let segments = lastMsg.segments ? [...lastMsg.segments] : [];
          const kind = payload.kind as string;

          if (kind === "text") {
            segments = appendDelta(segments, "text", payload.text ?? "");
          } else if (kind === "reasoning") {
            segments = appendDelta(segments, "reasoning", payload.text ?? "");
          } else if (kind === "tool_call" && payload.segment) {
            const s = payload.segment;
            segments = pushToolCall(segments, {
              id: s.id,
              tool: s.tool,
              args: s.args,
            });
          } else if (kind === "tool_result" && payload.segment) {
            const s = payload.segment;
            segments = mergeToolResult(segments, {
              id: s.id,
              status: s.status,
              summary: s.summary,
              sql: s.sql,
              table: s.table,
              elapsedMs: s.elapsedMs,
            });
          } else if (kind === "chart" && payload.segment) {
            const s = payload.segment;
            segments = pushChart(segments, {
              id: s.id,
              chartType: s.chartType,
              title: s.title,
              xField: s.xField,
              yFields: s.yFields,
              table: s.table,
            });
            // A DDL tool just finished (created/dropped a table or view) —
            // refresh the data tree immediately so the change is visible. Only
            // on terminal states (ok/error), not the intermediate "awaiting".
            if (s.status === "ok" || s.status === "error") {
              const seg = segments.find((x) => x.type === "tool" && x.id === s.id);
              const toolName = seg && seg.type === "tool" ? seg.tool : "";
              if (
                toolName === "create_table" ||
                toolName === "create_view" ||
                toolName === "drop_object"
              ) {
                invoke<SourceTable[]>("list_duckdb_tables")
                  .then(setSources)
                  .catch((err) => console.error("Failed to refresh sources after DDL:", err));
              }
            }
          } else if (kind === "usage" && payload.text) {
            // Fold the usage event into the task's persisted TokenUsage via the
            // pure `mergeUsage` (handles estimate / real / run-summary events,
            // calibration, cumulative accumulation, peak, turn counting).
            try {
              const evt = JSON.parse(payload.text);
              t = { ...t, tokenUsage: mergeUsage(t.tokenUsage ?? null, evt) };
            } catch { /* ignore parse error */ }
          } else if (kind === "error") {
            segments = [
              ...segments,
              {
                type: "error",
                id: `seg-err-${Date.now()}`,
                text: payload.text ?? "未知错误",
              },
            ];
          }

          messages[messages.length - 1] = { ...lastMsg, segments };

          if (kind === "done" || kind === "error") {
            const lastIdx = segments.length - 1;
            if (lastIdx >= 0) {
              const lastSeg = segments[lastIdx];
              if (lastSeg.type === "reasoning" && lastSeg.startTime && !lastSeg.elapsedMs) {
                segments[lastIdx] = { ...lastSeg, elapsedMs: Date.now() - lastSeg.startTime };
                messages[messages.length - 1] = { ...lastMsg, segments };
              }
            }
            void saveChatTaskBackend(targetId, t.name, messages);
          }

          return { ...t, messages };
        }),
      );

      // 流式结束（成功或出错）：清除执行状态，解除输入锁定。
      if (payload.kind === "done" || payload.kind === "error") {
        setStreamingTaskId(null);
      }
    });

    try {
      const list = await invoke<Workspace[]>("load_workspaces");
      if (list && list.length > 0) {
        setWorkspaces(list);
        // Prefer the last-used workspace, falling back to DefaultProject, then
        // the first entry. The persisted key may point at a workspace that was
        // since removed, so we validate it against the list before using it.
        let last: string | null = null;
        try {
          last = await invoke<string | null>("get_app_config", { key: "workspace.last" });
        } catch {
          /* config read is best-effort; fall through to defaults */
        }
        const defaultWS =
          (last && list.find((w) => w.path === last)) ||
          list.find((w) => w.path === "DefaultProject") ||
          list[0];
        // Only switch if the resolved default differs from the current workspace
        // — otherwise the unchanged-value set would re-trigger the workspace-load
        // effect (duplicate scan/sync + list_tables_fast on startup).
        if (currentWorkspace().path !== defaultWS.path) {
          changeWorkspace(defaultWS);
        }
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
      unlistenImport();
    });
  });

  // Track workspace change to load its tasks and scan source files
  createEffect(async () => {
    const ws = currentWorkspace();
    if (!ws) return;
    setBusy(true);
    try {
      // 1. Load tasks. Normalize legacy chat messages (flat content/reasoning/
      //    cards) into the segment model so old persisted chats stay readable.
      const loadedTasks = await invoke<QueryTask[]>("load_workspace_tasks", { workspacePath: ws.path });
      if (currentWorkspace().path !== ws.path) return;

      const migrated = loadedTasks
        .map((t) =>
          t.kind === "chat" && Array.isArray(t.messages)
            ? { ...t, messages: t.messages.map((m) => normalizeMessage(m)) }
            : t,
        )
        .sort((a, b) => b.createdAt - a.createdAt);
      setTasks(migrated);

      if (migrated.length > 0) {
        const activeId = activeTaskId();
        if (activeId && migrated.some(t => t.id === activeId)) {
          selectTask(activeId);
        } else {
          selectTask(migrated[0].id);
        }
      } else {
        setActiveTaskId(null);
        setSql("SELECT 1 AS n;");
      }

      // 2. Show tables instantly from the SQLite cache (millisecond-fast: names +
      //    columns + row counts all persisted). Then run the file scan + sync in
      //    the background; it rebuilds only sources whose fingerprint changed and
      //    merges custom tables/views back in when done.
      const fastTables = await invoke<SourceTable[]>("list_tables_fast", { workspacePath: ws.path });
      if (currentWorkspace().path !== ws.path) return;

      setSources(fastTables);
      setSelectedTable(null);
      refreshRegisterStatus(ws.path);

      // Background: scan files + sync sources (rebuilds only changed tables).
      void invoke<SourceTable[]>("register_workspace_sources", { workspacePath: ws.path })
        .then(() => {
          if (currentWorkspace()?.path !== ws.path) return;
          // After sync, refresh via list_duckdb_tables which merges custom
          // tables/views (t_/v_) from the lake catalog — sync + fast list only
          // know about s_ sources in SQLite. This makes views appear right after
          // sync instead of waiting for the slower warmup pass.
          void invoke<SourceTable[]>("list_duckdb_tables")
            .then((merged) => {
              if (currentWorkspace()?.path === ws.path) setSources(merged);
            })
            .catch((err) => console.error("Failed to refresh table list:", err));
          // Warm up in the background: verifies each lake object is usable and
          // rebuilds any that went missing. Refreshes the tree if rebuilds ran.
          void invoke<SourceTable[]>("warmup_sources")
            .then((warmed) => {
              if (currentWorkspace()?.path === ws.path) setSources(warmed);
            })
            .catch((err) => console.error("Failed to warmup sources:", err));
        })
        .catch((err) => {
          if (currentWorkspace()?.path === ws.path) {
            console.error("Failed to sync workspace sources:", err);
          }
        });
    } catch (err) {
      if (currentWorkspace().path === ws.path) {
        console.error("Failed to load workspace tasks & sources:", err);
      }
    } finally {
      if (currentWorkspace().path === ws.path) {
        setBusy(false);
      }
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
      changeWorkspace(ws);
    } catch (err) {
      console.error("Failed to add workspace:", err);
    }
  }

  function selectWorkspace(path: string) {
    const ws = workspaces().find((w) => w.path === path);
    if (ws) {
      changeWorkspace(ws);
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
        changeWorkspace(def);
        return;
      }
      
      setWorkspaces(nextList);
      if (currentWorkspace().path === path) {
        changeWorkspace(nextList[0]);
      }
    } catch (err) {
      console.error("Failed to remove workspace:", err);
    }
  }

  function createTask(prompt: string, kind: TaskKind = "chat") {
    const id = `task-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`;
    let name = prompt.trim().replace(/\n/g, " ");
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
    }
  }

  function selectTask(id: string) {
    const task = tasks().find((t) => t.id === id);
    if (!task) return;
    setActiveTaskId(id);
    setDeps(null);
    if ((task.kind ?? "sql") === "sql") {
      setSql(task.sql);
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
      const task = tasks().find((t) => t.id === taskId);
      const modelId = task?.modelId || null;
      const tokenUsage = task?.tokenUsage ?? null;
      await invoke("save_chat_task", {
        workspacePath: currentWorkspace().path,
        taskId,
        name,
        messages,
        modelId,
        tokenUsage,
      });
    } catch (err) {
      console.error("Failed to save chat task to backend:", err);
    }
  }

  /** ChatView 发送消息：追加 user 消息 → 触发 Rust Agent 循环。 */
  /** Abort the running agent stream for a task. */
  async function stopChat(taskId: string) {
    try {
      await invoke("abort_chat", { taskId });
      // The backend emits "done" after the abort is processed, which unlocks
      // the input via the agent-event listener. But also set it here in case
      // the stream is stuck in a tool call.
      setStreamingTaskId(null);
    } catch (err) {
      console.error("Failed to abort chat:", err);
    }
  }

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
      segments: [{ type: "text", id: `seg-txt-${Date.now()}`, text: prompt }],
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
      setStreamingTaskId(id);
      const activeModel = task.modelId || selectedModel();
      const historyToSend = task.messages ?? [];
      const historyJson = JSON.stringify(historyToSend);
      
      await invoke("start_agent_chat", {
        taskId: id,
        modelId: activeModel,
        prompt,
        historyJson,
        priority: selectedPriority(),
        confirmMode: selectedConfirm(),
      });
    } catch (err) {
      console.error("Failed to start agent chat:", err);
      setStreamingTaskId(null);
      const errorMsg: ChatMessage = {
        id: `msg-err-${Date.now()}`,
        role: "assistant",
        segments: [{
          type: "text",
          id: `seg-txt-${Date.now()}`,
          text: `⚠️ **无法启动对话**: ${err}`,
        }],
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
    let name = prompt.trim().replace(/\n/g, " ");
    if (!name) name = "新对话";

    if (availableModels().length === 0) {
      alert("请先前往设置中心（右上角菜单 -> 模型设置中心）配置并启用大模型供应商及模型。");
      return;
    }

    const userMsg: ChatMessage = {
      id: `msg-${Date.now()}`,
      role: "user",
      segments: [{ type: "text", id: `seg-txt-${Date.now()}`, text: prompt }],
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
      setStreamingTaskId(id);
      await invoke("start_agent_chat", {
        taskId: id,
        modelId: targetModel,
        prompt,
        historyJson,
        priority: selectedPriority(),
        confirmMode: selectedConfirm(),
      });
    } catch (err) {
      console.error("Failed to start agent chat:", err);
      setStreamingTaskId(null);
      const errorMsg: ChatMessage = {
        id: `msg-err-${Date.now()}`,
        role: "assistant",
        segments: [{
          type: "text",
          id: `seg-txt-${Date.now()}`,
          text: `⚠️ **无法启动对话**: ${err}`,
        }],
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
      segments: [{ type: "text", id: `seg-txt-${Date.now()}`, text: prompt }],
      ts: Date.now(),
    };

    let name = prompt.trim().replace(/\n/g, " ");

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
      setStreamingTaskId(id);
      await invoke("start_agent_chat", {
        taskId: id,
        modelId: targetModel,
        prompt,
        historyJson,
        priority: selectedPriority(),
        confirmMode: selectedConfirm(),
      });
    } catch (err) {
      console.error("Failed to start agent chat:", err);
      setStreamingTaskId(null);
      const errorMsg: ChatMessage = {
        id: `msg-err-${Date.now()}`,
        role: "assistant",
        segments: [{
          type: "text",
          id: `seg-txt-${Date.now()}`,
          text: `⚠️ **无法启动对话**: ${err}`,
        }],
        ts: Date.now(),
      };
      setTasks((prev) =>
        prev.map((t) =>
          t.id === id ? { ...t, messages: [userMsg, errorMsg] } : t,
        ),
      );
    }
  }

  /** ToolSegment「在 SQL 面板打开」：新建 SQL task 注入（已格式化的）SQL 并自动执行。 */
  function openInSqlPanel(sql: string) {
    createTask(tryFormatDuckdbSql(sql), "sql");
    void run();
  }

  /** ToolSegment「确认执行/取消」：把用户的决定回传给正在阻塞等待的 DDL 工具。 */
  async function resolveToolConfirmation(taskId: string, toolCallId: string, approved: boolean) {
    try {
      await invoke("resolve_tool_confirmation", {
        taskId,
        toolCallId,
        approved,
      });
    } catch (err) {
      console.error("Failed to resolve tool confirmation:", err);
    }
  }

  async function deleteTask(id: string) {
    const remaining = tasks().filter((t) => t.id !== id);
    setTasks(remaining);
    if (activeTaskId() === id) {
      const visible = remaining
        .filter((task) => {
          if (task.kind === "chat") {
            return (task.messages?.length ?? 0) > 0;
          } else {
            const isDefaultOrEmpty = !task.sql.trim() || task.sql.trim() === "SELECT 1 AS n;";
            return !!task.saved && !isDefaultOrEmpty;
          }
        })
        .sort((a, b) => b.createdAt - a.createdAt);
      if (visible.length > 0) {
        selectTask(visible[0].id);
      } else {
        setActiveTaskId(null);
      }
    }
    try {
      await invoke("delete_task", { taskId: id });
    } catch (err) {
      console.error("Failed to delete task:", err);
    }
  }

  function closeTab(tabId: string) {
    const taskId = activeTaskId();
    if (!taskId) return;

    // Drop the DOM ref so the active-tab effect / Map doesn't hold a stale node.
    tabItemRefs.delete(tabId);

    setTaskTabs((prev) => {
      const list = prev[taskId] || [];
      const filtered = list.filter((t) => t.id !== tabId);
      return { ...prev, [taskId]: filtered };
    });

    if (taskActiveTabId()[taskId] === tabId) {
      const remaining = (taskTabs()[taskId] || []).filter((t) => t.id !== tabId);
      if (remaining.length > 0) {
        setTaskActiveTabId((prev) => ({ ...prev, [taskId]: remaining[remaining.length - 1].id }));
      } else {
        setTaskActiveTabId((prev) => ({ ...prev, [taskId]: null }));
      }
    }
  }

  function saveRename(tabId: string) {
    const nextName = editingText().trim();
    if (nextName && activeTaskId()) {
      const taskId = activeTaskId()!;
      setTaskTabs((prev) => {
        const list = prev[taskId] || [];
        return {
          ...prev,
          [taskId]: list.map((t) => (t.id === tabId ? { ...t, name: nextName } : t)),
        };
      });
    }
    setEditingTabId(null);
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
    return tasks()
      .filter((task) => {
        if (task.kind === "chat") {
          return (task.messages?.length ?? 0) > 0;
        } else {
          const isDefaultOrEmpty = !task.sql.trim() || task.sql.trim() === "SELECT 1 AS n;";
          return !!task.saved && !isDefaultOrEmpty;
        }
      })
      .sort((a, b) => b.createdAt - a.createdAt);
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

  /** Shared import path for drag-drop, the file picker, and the folder picker.
   *  Loops the given paths through `importFileToWorkspace` and refreshes the
   *  source list when anything landed. Per-path progress + failures (including
   *  "no recognizable data files") are surfaced by the backend via the
   *  `import-progress` event banner — no silent no-op. */
  async function importPaths(paths: string[]) {
    if (busy() || paths.length === 0) return;
    setBusy(true);
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
      refreshRegisterStatus(currentWorkspace().path);
    } catch (e) {
      console.error("Failed to import data source:", e);
    } finally {
      setBusy(false);
    }
  }

  async function handleDropFiles(paths: string[]) {
    await importPaths(paths);
  }

  async function handleImportFile(filePath: string) {
    if (busy()) return;
    setBusy(true);
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
      refreshRegisterStatus(currentWorkspace().path);
    } catch (e) {
      console.error("Failed to import file:", e);
    } finally {
      setBusy(false);
    }
  }

  async function handleSelectFile() {
    if (busy()) return;
    try {
      const path = await selectFile();
      if (path) await importPaths([path]);
    } catch (e) {
      console.error("Failed to select file:", e);
    }
  }

  async function handleSelectFolder() {
    if (busy()) return;
    try {
      const path = await selectDirectory("请选择要扫描的数据文件夹");
      if (path) await importPaths([path]);
    } catch (e) {
      console.error("Failed to select directory:", e);
    }
  }

  /** 执行当前 SQL，无论成功或失败都写入日志，并记录至结果 tab 列表中。 */
  async function run() {
    const q = sql().trim();
    if (!q || busy()) return;
    setBusy(true);
    const started = Date.now();
    let entry: LogEntry;

    const tabId = `tab-${Date.now()}`;
    const taskId = activeTaskId();

    let tabName = "结果";
    if (taskId) {
      const list = taskTabs()[taskId] || [];
      let nextNum = 0;
      while (true) {
        const candidate = nextNum === 0 ? "结果" : `结果${nextNum}`;
        if (!list.some((t: QueryResultTab) => t.name === candidate)) {
          tabName = candidate;
          break;
        }
        nextNum++;
      }
    }

    const newTab: QueryResultTab = {
      id: tabId,
      name: tabName,
      sql: q,
      result: null,
      error: null,
      timestamp: started,
    };

    if (taskId) {
      setTaskTabs((prev) => {
        const list = prev[taskId] || [];
        return { ...prev, [taskId]: [...list, newTab] };
      });
      setTaskActiveTabId((prev) => ({ ...prev, [taskId]: tabId }));
    }

    try {
      const res = await executeSql(q, rowCap());
      
      if (taskId) {
        setTaskTabs((prev) => {
          const list = prev[taskId] || [];
          return {
            ...prev,
            [taskId]: list.map((t) => t.id === tabId ? { ...t, result: res } : t),
          };
        });
      }

      entry = {
        id: ++logSeq,
        ts: started,
        sql: q,
        status: "ok",
        rowCount: res.rowCount,
        truncated: res.truncated,
        elapsedMs: res.elapsedMs,
      };

      // Refresh the data tree only when the query may have changed the schema
      // (DDL). A plain SELECT (e.g. table preview) leaves the tree unchanged, so
      // we skip the `list_duckdb_tables` round-trip — that call walks DuckLake
      // catalog metadata (slow, ~1s/custom table) and would block the left nav
      // for a moment after every click.
      const isDdl = /^\s*(create|drop|alter|insert|update|delete|truncate)\b/i.test(q);
      if (isDdl) {
        try {
          const dbTables = await invoke<SourceTable[]>("list_duckdb_tables");
          setSources(dbTables);
        } catch (refreshErr) {
          console.error("refresh table list failed:", refreshErr);
        }
      }
    } catch (e) {
      const msg = String(e);
      if (taskId) {
        setTaskTabs((prev) => {
          const list = prev[taskId] || [];
          return {
            ...prev,
            [taskId]: list.map((t) => t.id === tabId ? { ...t, error: msg } : t),
          };
        });
      }
      entry = { id: ++logSeq, ts: started, sql: q, status: "error", error: msg, elapsedMs: Date.now() - started };
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
    // Fetch dependency info (upstreams + downstreams) for the right panel.
    void invoke<DepInfo>("get_dependencies", { tableName: t.name })
      .then(setDeps)
      .catch(() => setDeps(null));
  }

  /** Select a table by name (used by dependency chip clicks + context menu). */
  function selectTableByName(name: string) {
    const t = sources().find((s) => s.name === name);
    if (t) selectTable(t);
  }

  /** Delete a table/view with dependency protection. */
  async function deleteTable(name: string) {
    try {
      await invoke("drop_table_safe", { tableName: name });
      // Refresh data tree after successful deletion.
      const dbTables = await invoke<SourceTable[]>("list_duckdb_tables");
      setSources(dbTables);
      if (selectedTable()?.name === name) setSelectedTable(null);
      setDeps(null);
      setLogs((prev) => [{
        id: ++logSeq,
        ts: Date.now(),
        sql: `删除 ${name}`,
        status: "ok" as const,
      }, ...prev].slice(0, 100));
    } catch (e) {
      const msg = typeof e === "string" ? e : "删除失败";
      setLogs((prev) => [{
        id: ++logSeq,
        ts: Date.now(),
        sql: `删除 ${name}`,
        status: "error" as const,
        error: msg,
      }, ...prev].slice(0, 100));
    }
  }

  /** Delete a workspace file (cascades to its s_ table + downstreams). */
  async function deleteFile(path: string) {
    const fileName = path.split("/").pop() ?? path;
    try {
      await invoke<string>("delete_file", { filePath: path });
      // Refresh data tree + file tree.
      const dbTables = await invoke<SourceTable[]>("list_duckdb_tables");
      setSources(dbTables);
      setFileTrigger((t) => t + 1);
      if (selectedTable()?.path === path) setSelectedTable(null);
      setDeps(null);
      setLogs((prev) => [{
        id: ++logSeq,
        ts: Date.now(),
        sql: `删除文件 ${fileName}`,
        status: "ok" as const,
      }, ...prev].slice(0, 100));
    } catch (e) {
      const msg = typeof e === "string" ? e : "删除失败";
      setLogs((prev) => [{
        id: ++logSeq,
        ts: Date.now(),
        sql: `删除文件 ${fileName}`,
        status: "error" as const,
        error: msg,
      }, ...prev].slice(0, 100));
    }
  }

  /** 检查器 → 编辑器：注入一段 SQL（格式化后）并自动执行。 */
  function injectSql(s: string) {
    const formatted = tryFormatDuckdbSql(s);
    setSql(formatted);

    const active = activeTask();
    if (!active || active.kind !== "sql") {
      createTask(formatted, "sql");
    } else {
      const activeId = activeTaskId();
      if (activeId) {
        setTasks((prev) => prev.map((t) => (t.id === activeId ? { ...t, sql: formatted } : t)));
      }
    }
    void run();
  }

  /** 检查器 → 直接预览某表（SELECT * LIMIT 50）。 */
  function previewTable(t: SourceTable) {
    injectSql(`SELECT * FROM "${t.name}" LIMIT 50;`);
  }

  async function copySql() {
    try {
      await navigator.clipboard.writeText(sql());
    } catch {
      /* clipboard may be unavailable; ignore */
    }
  }

  function onSettings(tab?: SettingsTab) {
    setSettingsTab(tab ?? "general");
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
            workspacePath={currentWorkspace().path}
            initialTab={settingsTab()}
            onClose={() => { setSettingsOpen(false); setSettingsTab("general"); }}
            titleBar={
              <TitleBar
                inspectorOpen={inspectorOpen()}
                consoleOpen={consoleState() !== "folded"}
                onToggleInspector={() => setInspectorOpen((v) => !v)}
                onToggleConsole={() => setConsoleState((s) => (s === "folded" ? "default" : "folded"))}
                selectedTable={selectedTable()}
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
          registerStatus={registerStatus()}
          onSelectTask={selectTask}
          onDeleteTask={deleteTask}
          onSelectWorkspace={selectWorkspace}
          onRemoveWorkspace={removeWorkspace}
          onAddWorkspace={addWorkspace}
          onImportFile={handleImportFile}
          onRegisterDatabaseTable={setSources}
          sources={sources()}
          fileTrigger={fileTrigger()}
          selected={selectedTable()?.name ?? null}
          busy={busy()}
          onSelect={selectTable}
          onOpenSettings={onSettings}
          onNewQuery={() => createTask("SELECT 1 AS n;", "sql")}
          onNewChat={() => createTask("", "chat")}
          leftOpen={leftOpen()}
          importStatus={importStatus()}
          onDeleteTable={deleteTable}
          onDeleteFile={deleteFile}
          onToggleLeft={() => setLeftOpen(!leftOpen())}
        />

        <div class="right-container">
          <TitleBar
            inspectorOpen={inspectorOpen()}
            consoleOpen={consoleState() !== "folded"}
            onToggleInspector={() => setInspectorOpen((v) => !v)}
            onToggleConsole={() => setConsoleState((s) => (s === "folded" ? "default" : "folded"))}
            selectedTable={selectedTable()}
            busy={busy()}
            leftOpen={leftOpen()}
            onToggleLeft={() => setLeftOpen(!leftOpen())}
          />

          <div
            class="right-content-layout"
            style={{
              "grid-template-columns": `1fr ${rightWidthActual()}`,
              "grid-template-rows": `1fr ${bottomHeightActual()}`,
            }}
          >
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
                    onAddFile={handleSelectFile}
                    onAddFolder={handleSelectFolder}
                    availableModels={availableModels()}
                    selectedModel={selectedModel()}
                    onSelectModel={(model) => {
                      setSelectedModel(model);
                      localStorage.setItem("default_model", model);
                    }}
                    selectedPriority={selectedPriority()}
                    onSelectPriority={setSelectedPriority}
                    selectedConfirm={selectedConfirm()}
                    onSelectConfirm={setSelectedConfirm}
                  />
                }
              >
                <Switch>
                  <Match when={(activeTask()?.kind ?? "sql") === "chat"}>
                    <Show when={activeTaskId()}>
                      {(id) => (
                        <ChatView
                          taskId={id()}
                          messages={activeTask()?.messages ?? []}
                          workspace={currentWorkspace().name}
                          taskName={activeTask()?.name ?? ""}
                          streaming={streamingTaskId() === id()}
                          onSend={sendChatMessage}
                          onStop={() => void stopChat(id())}
                          tokenUsage={tasks().find((t) => t.id === id())?.tokenUsage ?? null}
                          contextWindow={modelCtxWindows()[selectedModel()] ?? 128000}
                          onOpenInSqlPanel={openInSqlPanel}
                          onDelete={() => deleteTask(id())}
                          availableModels={availableModels()}
                          selectedModel={activeTask()?.modelId || selectedModel()}
                          onSelectModel={(model) => {
                            const activeId = id();
                            if (activeId) {
                              setTasks((prev) =>
                                prev.map((t) =>
                                  t.id === activeId ? { ...t, modelId: model } : t
                                )
                              );
                              setTimeout(() => {
                                const updated = tasks().find((t) => t.id === activeId);
                                if (updated) {
                                  void saveChatTaskBackend(activeId, updated.name, updated.messages || []);
                                }
                              }, 0);
                            }
                            setSelectedModel(model);
                            localStorage.setItem("default_model", model);
                          }}
                          selectedPriority={selectedPriority()}
                          onSelectPriority={setSelectedPriority}
                          selectedConfirm={selectedConfirm()}
                          onSelectConfirm={setSelectedConfirm}
                          onConfirmTool={(toolCallId, approved) =>
                            resolveToolConfirmation(id(), toolCallId, approved)
                          }
                        />
                      )}
                    </Show>
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
                      
                      {/* Unified Results Card */}
                      <Show
                        when={currentTabs().length > 0}
                        fallback={<div class="result-empty">暂无执行结果，点击运行开始查询。</div>}
                      >
                        <div class="query-result-card">
                          <div
                            class="result-tabs-bar"
                            ref={resultTabsBarRef}
                            onWheel={(e) => {
                              // Map vertical wheel to horizontal scroll so users
                              // don't have to hold Shift to traverse the tab strip.
                              const bar = e.currentTarget;
                              if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) {
                                bar.scrollLeft += e.deltaY;
                              }
                            }}
                          >
                            <For each={currentTabs()}>
                              {(tab: QueryResultTab) => (
                                <div
                                  class="result-tab-item"
                                  classList={{ active: currentActiveTabId() === tab.id }}
                                  ref={(el: HTMLDivElement) => { tabItemRefs.set(tab.id, el); }}
                                  onClick={() => {
                                    const taskId = activeTaskId();
                                    if (taskId) {
                                      setTaskActiveTabId((prev) => ({ ...prev, [taskId]: tab.id }));
                                    }
                                  }}
                                  onDblClick={(e) => {
                                    e.stopPropagation();
                                    setEditingTabId(tab.id);
                                    setEditingText(tab.name);
                                  }}
                                  title={tab.sql}
                                >
                                  <Show
                                    when={editingTabId() === tab.id}
                                    fallback={<span class="tab-name">{tab.name}</span>}
                                  >
                                    <input
                                      type="text"
                                      class="tab-rename-input"
                                      value={editingText()}
                                      onInput={(e) => setEditingText(e.currentTarget.value)}
                                      onKeyDown={(e) => {
                                        if (e.key === "Enter") {
                                          saveRename(tab.id);
                                        } else if (e.key === "Escape") {
                                          setEditingTabId(null);
                                        }
                                      }}
                                      onBlur={() => saveRename(tab.id)}
                                      ref={(el: HTMLInputElement) => setTimeout(() => { el.focus(); el.select(); }, 0)}
                                      onClick={(e) => e.stopPropagation()}
                                    />
                                  </Show>
                                  <button
                                    class="tab-close-btn"
                                    onClick={(e) => {
                                      e.stopPropagation();
                                      closeTab(tab.id);
                                    }}
                                  >
                                    ✕
                                  </button>
                                </div>
                              )}
                            </For>
                          </div>

                          {/* Active Tab Content */}
                          <Show when={activeTab()}>
                            {(tabVal) => (
                              <div class="result-tab-content">
                                <Show when={tabVal().error}>
                                  <pre class="error-box">{tabVal().error}</pre>
                                </Show>
                                <Show when={tabVal().result}>
                                  <ResultTable result={tabVal().result} />
                                </Show>
                              </div>
                            )}
                          </Show>
                        </div>
                      </Show>
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
                deps={deps()}
                onSelectDep={selectTableByName}
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
