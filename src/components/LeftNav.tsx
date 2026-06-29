import { For, Show, createMemo, createSignal, createEffect } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import type { SourceTable, QueryTask, Workspace, FileItem, RegisterStatus, ImportProgress, DbConnection } from "../lib/types";
import { t } from "../lib/i18n";
import { logoSrc } from "../lib/theme";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

/**
 * Left navigation styled like ZCode 3.0:
 * - Top-bar with Z logo and navigation arrows (<- and ->).
 * - Quick actions: "新建对话", "新建查询".
 * - Workspace section header ("工作区" label with buttons).
 * - Tree list grouped by directory.
 * - Bottom footer with a logo ("研途教育"), a layout switcher, and settings gear.
 */
/** Map backend import stage codes to Chinese labels for the progress banner. */
function stageLabel(stage: string): string {
  switch (stage) {
    case "copying": return "复制文件";
    case "scanning": return "扫描";
    case "registering": return "映射为表";
    default: return stage;
  }
}

/** SVG icon for a source file kind — recognizable at a glance, compact.
 * The full kind name is preserved in the `title` attribute for hover. */
function KindIcon(props: { kind: string }) {
  const k = props.kind.toLowerCase();
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      style="width: 12px; height: 12px; display: block;"
    >
      <Show when={k === "csv" || k === "tsv"}>
        <rect x="3" y="3" width="18" height="18" rx="2" />
        <line x1="3" y1="9" x2="21" y2="9" />
        <line x1="3" y1="15" x2="21" y2="15" />
        <line x1="9" y1="3" x2="9" y2="21" />
        <line x1="15" y1="3" x2="15" y2="21" />
      </Show>
      <Show when={k === "parquet"}>
        <rect x="3" y="3" width="18" height="18" rx="2" />
        <line x1="9" y1="3" x2="9" y2="21" />
        <line x1="15" y1="3" x2="15" y2="21" />
        <line x1="3" y1="9" x2="21" y2="9" />
      </Show>
      <Show when={k === "json"}>
        <path d="M8 3H7a2 2 0 0 0-2 2v5a2 2 0 0 1-2 2 2 2 0 0 1 2 2v5a2 2 0 0 0 2 2h1" />
        <path d="M16 3h1a2 2 0 0 1 2 2v5a2 2 0 0 0 2 2 2 2 0 0 0-2 2v5a2 2 0 0 1-2 2h-1" />
      </Show>
      <Show when={k === "excel" || k === "table"}>
        <rect x="3" y="3" width="18" height="18" rx="2" />
        <line x1="3" y1="9" x2="21" y2="9" />
        <line x1="3" y1="15" x2="21" y2="15" />
        <line x1="12" y1="3" x2="12" y2="21" />
      </Show>
      <Show when={k === "view"}>
        <rect x="3" y="3" width="18" height="18" rx="2" stroke-dasharray="3 2" />
        <line x1="3" y1="9" x2="21" y2="9" stroke-dasharray="3 2" />
        <line x1="3" y1="15" x2="21" y2="15" stroke-dasharray="3 2" />
        <line x1="12" y1="3" x2="12" y2="21" stroke-dasharray="3 2" />
      </Show>
      <Show when={k === "delta"}>
        <path d="M12 3L3 20h18z" />
      </Show>
      <Show when={k === "postgres" || k === "mysql"}>
        <ellipse cx="12" cy="6" rx="8" ry="3" />
        <path d="M4 6v5c0 1.66 3.58 3 8 3s8-1.34 8-3V6" />
        <path d="M4 11v5c0 1.66 3.58 3 8 3s8-1.34 8-3v-5" />
      </Show>
      <Show when={k !== "csv" && k !== "tsv" && k !== "parquet" && k !== "json" && k !== "excel" && k !== "delta" && k !== "table" && k !== "view" && k !== "postgres" && k !== "mysql"}>
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
        <polyline points="14 2 14 8 20 8" />
      </Show>
    </svg>
  );
}

export default function LeftNav(props: {
  workspace: string;
  workspacePath?: string;
  workspaces?: Workspace[];
  tasks?: QueryTask[];
  activeTaskId?: string | null;
  /** File-registration coverage of the active workspace → colored dot by the
   * project name. Only rendered for the active workspace. */
  registerStatus?: RegisterStatus;
  onSelectTask?: (id: string) => void;
  onDeleteTask?: (id: string) => void;
  onSelectWorkspace?: (ws: string) => void;
  onRemoveWorkspace?: (wsPath: string) => void;
  onAddWorkspace?: (name: string) => void;
  sources: SourceTable[];
  fileTrigger?: number;
  selected: string | null;
  busy: boolean;
  onSelect: (table: SourceTable) => void;
  onOpenSettings: () => void;
  onNewQuery?: () => void;
  onNewChat?: () => void;
  onImportFile?: (filePath: string) => void;
  onRegisterDatabaseTable?: (sources: SourceTable[]) => void;
  leftOpen?: boolean;
  onToggleLeft?: () => void;
  /** Current file-import progress (null = idle). Shown as a status banner. */
  importStatus?: ImportProgress | null;
  /** Delete a table/view (with dependency check on the backend). */
  onDeleteTable?: (name: string) => void;
  /** Delete a workspace file (cascades to its s_ table + downstreams). */
  onDeleteFile?: (path: string) => void;
}) {
  // Group tables by their parent directory for a tree-like feel.
  // Two kinds of objects are collected into the flat (empty-path) group that
  // renders without a header:
  //  - agent-created tables/views (path is empty), and
  //  - source files sitting directly in the workspace root, so the workspace
  //    name (e.g. "DefaultProject") isn't shown as a redundant group header.
  //    We detect this by comparing the group's directory name to the workspace
  //    name, because `workspacePath` is a relative key (e.g. "DefaultProject")
  //    while source `path`s are absolute (e.g. ~/.lakemind/DefaultProject/x.csv).
  const groups = createMemo(() => {
    const wsName = props.workspace;
    const map = new Map<string, SourceTable[]>();
    for (const t of props.sources) {
      const slash = Math.max(t.path.lastIndexOf("/"), t.path.lastIndexOf("\\"));
      let group = slash >= 0 ? t.path.slice(0, slash) : t.path;
      // Files directly under the workspace root collapse into the flat group.
      if (group && wsName && shortDir(group) === wsName) group = "";
      const arr = map.get(group) ?? [];
      arr.push(t);
      map.set(group, arr);
    }
    // Keep directory groups first (stable insertion order), empty-path group last.
    const entries = [...map.entries()];
    entries.sort((a, b) => {
      const aEmpty = !a[0], bEmpty = !b[0];
      if (aEmpty !== bEmpty) return aEmpty ? 1 : -1;
      return 0;
    });
    return entries;
  });

  // File explorer states
  const [expandedPaths, setExpandedPaths] = createSignal<Record<string, boolean>>({});
  const [directoryContents, setDirectoryContents] = createSignal<Record<string, FileItem[]>>({});
  const [fileSearchQuery] = createSignal("");

  // Database connection explorer states
  interface DbTableItem {
    schema: string;
    name: string;
    kind: string;
  }
  const [workspaceConns, setWorkspaceConns] = createSignal<DbConnection[]>([]);
  const [dbTables, setDbTables] = createSignal<Record<string, DbTableItem[]>>({});
  const [expandedDbConns, setExpandedDbConns] = createSignal<Record<string, boolean>>({});
  const [loadingDbConns, setLoadingDbConns] = createSignal<Record<string, boolean>>({});
  const [dbSectionExpanded, setDbSectionExpanded] = createSignal(true);

  const loadWorkspaceConnections = async () => {
    const ws = props.workspacePath;
    if (!ws) return;
    try {
      const list = await invoke<DbConnection[]>("list_workspace_connections", { wsPath: ws });
      setWorkspaceConns(list);
    } catch (err) {
      console.error("Failed to list workspace db connections:", err);
    }
  };

  const toggleDbConn = async (c: DbConnection) => {
    const id = c.id;
    const isExpanded = expandedDbConns()[id];
    setExpandedDbConns({ ...expandedDbConns(), [id]: !isExpanded });
    
    if (!isExpanded && !dbTables()[id]) {
      setLoadingDbConns({ ...loadingDbConns(), [id]: true });
      try {
        const list = await invoke<DbTableItem[]>("list_db_connection_tables", { config: c });
        setDbTables({ ...dbTables(), [id]: list });
      } catch (err) {
        console.error("Failed to load tables for connection " + c.name, err);
      } finally {
        setLoadingDbConns({ ...loadingDbConns(), [id]: false });
      }
    }
  };

  const handleRegisterDbTable = async (c: DbConnection, table: DbTableItem) => {
    if (!props.workspacePath) return;
    try {
      const updatedSources = await invoke<SourceTable[]>("register_database_table", {
        workspace: props.workspacePath,
        connectionId: c.id,
        schemaName: table.schema,
        tableName: table.name,
        dbType: c.dbType,
      });
      props.onRegisterDatabaseTable?.(updatedSources);
    } catch (err) {
      alert("注册外部表失败: " + err);
    }
  };

  // File ↔ Data cross-highlighting (linkage). Clicking a table highlights its
  // backing file in the Files tree, and clicking a file highlights its table.
  const [highlightFile, setHighlightFile] = createSignal<string | null>(null);
  const [highlightTable, setHighlightTable] = createSignal<string | null>(null);
  // The file the user actively clicked — shown with the same dark "selected"
  // treatment as task/data leaves (distinct from the soft cross-link highlight).
  const [selectedFile, setSelectedFile] = createSignal<string | null>(null);
  // Right-click context menu for data tree leaves.
  const [ctxMenu, setCtxMenu] = createSignal<{ name: string; x: number; y: number } | null>(null);
  // Right-click context menu for file tree leaves.
  const [fileCtxMenu, setFileCtxMenu] = createSignal<{ path: string; name: string; x: number; y: number } | null>(null);

  const fileToTable = createMemo(() => {
    const m = new Map<string, string>();
    for (const s of props.sources) {
      if (s.path) m.set(s.path, s.name);
    }
    return m;
  });

  const handleSelectTable = (t: SourceTable) => {
    setHighlightTable(t.name);
    setHighlightFile(t.path || null);
    props.onSelect(t);
  };

  const handleFileClick = (item: FileItem) => {
    setSelectedFile(item.path);
    setHighlightFile(item.path);
    setHighlightTable(fileToTable().get(item.path) ?? null);
    props.onImportFile?.(item.path);
  };

  // Subsections expanded states
  const [tasksSectionExpanded, setTasksSectionExpanded] = createSignal(true);
  const [filesSectionExpanded, setFilesSectionExpanded] = createSignal(true);
  const [dataSectionExpanded, setDataSectionExpanded] = createSignal(true);

  // Workspace-level collapse: when false the whole subtree (任务/文件/数据) is
  // hidden, regardless of the individual section states above. Reset to open on
  // workspace switch so switching into a project never starts collapsed.
  const [wsCollapsed, setWsCollapsed] = createSignal(false);

  // True only when all three subsections are open. Drives the collapse/expand
  // toggle button next to the workspace node (chevron direction + tooltip).
  const allSectionsExpanded = createMemo(
    () => tasksSectionExpanded() && filesSectionExpanded() && dataSectionExpanded(),
  );

  // Toggle all three subsections at once. When any is collapsed, expand all;
  // otherwise collapse all. This is the single hover button on a workspace row.
  const toggleAllSections = () => {
    const open = allSectionsExpanded();
    setTasksSectionExpanded(!open);
    setFilesSectionExpanded(!open);
    setDataSectionExpanded(!open);
  };

  // Automatically load root directory contents when workspace changes.
  // Also reset the workspace-level collapse so a freshly-switched project is open.
  createEffect(async () => {
    const wsPath = props.workspacePath;
    if (wsPath) {
      setWsCollapsed(false);
      setExpandedPaths((prev) => ({ ...prev, [wsPath]: true }));
      await loadDirContents(wsPath);
      await loadWorkspaceConnections();
    }
  });

  // Auto-refresh directory lists when files are imported/dropped
  createEffect(async () => {
    const trigger = props.fileTrigger;
    if (trigger !== undefined && trigger > 0) {
      const paths = Object.keys(expandedPaths()).filter((p) => expandedPaths()[p]);
      for (const p of paths) {
        await loadDirContents(p);
      }
    }
  });

  const loadDirContents = async (dirPath: string) => {
    try {
      const items = await invoke<FileItem[]>("read_directory", { path: dirPath });
      setDirectoryContents((prev) => ({ ...prev, [dirPath]: items }));
    } catch (e) {
      console.error("Failed to read dir:", e);
    }
  };

  const toggleFolder = async (dirPath: string) => {
    const isExpanded = !!expandedPaths()[dirPath];
    setExpandedPaths((prev) => ({ ...prev, [dirPath]: !isExpanded }));
    if (!isExpanded && !directoryContents()[dirPath]) {
      await loadDirContents(dirPath);
    }
  };

  const renderFileTree = (dirPath: string, depth: number = 0) => {
    const contents = directoryContents()[dirPath] || [];
    const query = fileSearchQuery().trim().toLowerCase();

    const filteredContents = query
      ? contents.filter(item => item.name.toLowerCase().includes(query))
      : contents;

    return (
      <div class="fe-tree-container" style={{ "padding-left": `${depth > 0 ? 12 : 0}px` }}>
        <For each={filteredContents}>
          {(item) => {
            const isExpanded = !!expandedPaths()[item.path];
            return (
              <div class="fe-tree-node">
                <div
                  class="tree-leaf fe-node-row"
                  classList={{
                    "is-dir": item.is_dir,
                    selected: selectedFile() === item.path,
                    "no-active-bar": true,
                  }}
                  style={{
                    display: "flex",
                    "align-items": "center",
                    padding: "4px 8px",
                    "border-radius": "var(--radius-sm)",
                    cursor: "pointer",
                    transition: "background 0.12s ease",
                    background:
                      highlightFile() === item.path && selectedFile() !== item.path
                        ? "rgba(80, 160, 255, 0.14)"
                        : undefined,
                  }}
                  onClick={() => {
                    if (item.is_dir) {
                      toggleFolder(item.path);
                    } else {
                      handleFileClick(item);
                    }
                  }}
                  onContextMenu={(e) => {
                    if (!item.is_dir) {
                      e.preventDefault();
                      setFileCtxMenu({ path: item.path, name: item.name, x: e.clientX, y: e.clientY });
                    }
                  }}
                >
                  <Show
                    when={item.is_dir}
                    fallback={
                      <span class="kind-badge kind-badge--icon" data-kind={fileKind(item.name)} title={fileKind(item.name)}>
                        <KindIcon kind={fileKind(item.name)} />
                      </span>
                    }
                  >
                    <span class="fe-node-icon" style="font-size: 11px;">
                      {isExpanded ? "▾ 📁" : "▸ 📁"}
                    </span>
                  </Show>
                  <span class="fe-node-name" style="flex: 1; font-size: 12px; text-align: left; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
                    {item.name}
                  </span>
                  <Show when={item.is_modified}>
                    <Show when={item.is_dir} fallback={<span class="fe-modified-badge" style="color: var(--accent-orange); font-size: 11px; font-weight: bold; margin-left: 6px; padding-right: 4px;">M</span>}>
                      <span class="fe-modified-dot" style="display: inline-block; width: 6px; height: 6px; border-radius: 50%; background-color: var(--accent-orange); margin-left: 6px; margin-right: 4px;" />
                    </Show>
                  </Show>
                </div>
                <Show when={item.is_dir && isExpanded}>
                  {renderFileTree(item.path, depth + 1)}
                </Show>
              </div>
            );
          }}
        </For>
      </div>
    );
  };

  return (
    <nav class="leftnav">
      {/* Right-click context menu for data tree leaves. */}
      <Show when={ctxMenu()}>
        {(m) => (
          <>
            <div class="ctx-overlay" onClick={() => setCtxMenu(null)} />
            <div
              class="ctx-menu"
              style={{ left: `${m().x}px`, top: `${m().y}px` }}
            >
              <button
                class="ctx-item ctx-item--danger"
                onClick={() => {
                  const name = m().name;
                  setCtxMenu(null);
                  props.onDeleteTable?.(name);
                }}
              >
                删除
              </button>
            </div>
          </>
        )}
      </Show>
      {/* Right-click context menu for file tree leaves. */}
      <Show when={fileCtxMenu()}>
        {(m) => (
          <>
            <div class="ctx-overlay" onClick={() => setFileCtxMenu(null)} />
            <div
              class="ctx-menu"
              style={{ left: `${m().x}px`, top: `${m().y}px` }}
            >
              <button
                class="ctx-item ctx-item--danger"
                onClick={() => {
                  const path = m().path;
                  setFileCtxMenu(null);
                  props.onDeleteFile?.(path);
                }}
              >
                删除文件
              </button>
            </div>
          </>
        )}
      </Show>
      {/* ZCode style top header with Z logo and history arrows */}
      <div class="ln-top-bar" classList={{ "mac-nav": isMac }}>
        <Show when={!isMac}>
          <div class="ln-logo-box" title="ZCode 3.0 / LakeMind">
            <img src={logoSrc()} alt="LakeMind" style="width: 18px; height: 18px; object-fit: contain;" />
          </div>
        </Show>
        <div class="ln-nav-arrows" data-tauri-drag-region>
          {/* Sidebar toggle button (always show in the sidebar) */}
          <button 
            class="ln-arrow-btn" 
            classList={{ active: props.leftOpen }}
            title={props.leftOpen ? "隐藏侧边栏" : "显示侧边栏"} 
            onClick={() => props.onToggleLeft?.()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="9" y1="3" x2="9" y2="21"></line>
            </svg>
          </button>
        </div>
      </div>

      {/* Quick Action links */}
      <div class="ln-quick-actions">
        <button class="ln-action-btn" title="新建对话 (Ctrl+Shift+N)" onClick={() => props.onNewChat?.()} disabled={props.busy}>
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"/>
            </svg>
          </span>
          <span class="action-label">新建对话</span>
          <span class="action-shortcut">⇧⌘ N</span>
        </button>
        <button class="ln-action-btn" title="新建查询 (Ctrl+N)" onClick={() => props.onNewQuery?.()} disabled={props.busy}>
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M12 5v14M5 12h14"/>
            </svg>
          </span>
          <span class="action-label">新建查询</span>
          <span class="action-shortcut">⌘ N</span>
        </button>
      </div>

      {/* Workspace header */}
      <div class="ln-section-header">
        <span class="section-title">工作区</span>
        <div class="section-actions">
          <button
            class="sec-act-btn"
            title={wsCollapsed() ? "展开项目" : "收起项目"}
            onClick={() => setWsCollapsed(!wsCollapsed())}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polyline points="4 14 10 14 10 20"></polyline>
              <polyline points="20 10 14 10 14 4"></polyline>
              <line x1="14" y1="10" x2="21" y2="3"></line>
              <line x1="10" y1="14" x2="3" y2="21"></line>
            </svg>
          </button>
        </div>
      </div>

      {/* Tree content */}
      <div class="tree">
        <For each={props.workspaces ?? []}>
          {(ws) => {
            const isActive = () => ws.path === props.workspacePath;
            return (
              <div class="tree-group" style={{ "margin-bottom": isActive() ? "12px" : "4px" }}>
                {/* Workspace Folder Node */}
                <div 
                  class="tree-group-label workspace-root-node"
                  classList={{ active: isActive() }}
                  title={ws.path}
                  onClick={() => props.onSelectWorkspace?.(ws.path)}
                  style={{
                    display: "flex", 
                    "align-items": "center", 
                    "font-weight": isActive() ? "600" : "500", 
                    "font-size": "12px", 
                    color: isActive() ? "var(--text-primary)" : "var(--text-secondary)", 
                    cursor: "pointer", 
                    padding: "6px 8px", 
                    "border-radius": "var(--radius-sm)",
                    background: isActive() ? "var(--bg-hover)" : "transparent",
                    position: "relative"
                  }}
                >
                  <span style="margin-right: 6px; display: inline-flex; align-items: center; justify-content: center; color: var(--text-secondary);">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                      <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                    </svg>
                  </span>
                  <span style="flex: 1; text-align: left; display: inline-flex; align-items: center; gap: 6px;">
                    {ws.name}
                    {/* File-registration coverage dot — green=all / orange=partial /
                        red=none. Only on the active workspace; placed right after
                        the name so it never collides with the hover actions that
                        float on the far right. */}
                    <Show when={isActive()}>
                      <span
                        class="ws-indicator-dot"
                        classList={{
                          all: props.registerStatus !== "partial" && props.registerStatus !== "none",
                          partial: props.registerStatus === "partial",
                          none: props.registerStatus === "none",
                        }}
                        title={
                          props.registerStatus === "all" ? "全部文件已注册"
                          : props.registerStatus === "partial" ? "部分文件未注册"
                          : "文件均未注册"
                        }
                      />
                    </Show>
                  </span>

                  {/* Toggle expand/collapse all subsections (任务/文件/数据) and
                      remove this workspace. Project-level collapse is handled by
                      the "收起全部" button in the workspace section header. */}
                  <div class="ws-hover-actions">
                    <button
                      class="ws-action-icon-btn"
                      title={allSectionsExpanded() ? "全部折叠" : "全部展开"}
                      onClick={(e) => {
                        e.stopPropagation();
                        toggleAllSections();
                      }}
                    >
                      <Show
                        when={allSectionsExpanded()}
                        fallback={
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                            <polyline points="6 9 12 15 18 9"></polyline>
                          </svg>
                        }
                      >
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                          <polyline points="18 15 12 9 6 15"></polyline>
                        </svg>
                      </Show>
                    </button>
                    <button
                      class="ws-action-icon-btn remove-ws-btn"
                      title="移除工作区"
                      onClick={(e) => {
                        e.stopPropagation();
                        props.onRemoveWorkspace?.(ws.path);
                      }}
                    >
                      <svg
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        stroke-linecap="round"
                        stroke-linejoin="round"
                        style="width: 12px; height: 12px;"
                      >
                        <polyline points="3 6 5 6 21 6"></polyline>
                        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"></path>
                      </svg>
                    </button>
                  </div>
                </div>

                {/* If active (and not collapsed), render its tasks, files and data */}
                <Show when={isActive() && !wsCollapsed()}>
                  {/* Category 1: 任务 */}
                  <div
                    class="tree-section-header"
                    onClick={(e) => {
                      e.stopPropagation();
                      setTasksSectionExpanded(!tasksSectionExpanded());
                    }}
                  >
                    <span class="tree-section-icon">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: block;">
                        <path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2"></path>
                        <rect x="8" y="2" width="8" height="4" rx="1" ry="1"></rect>
                        <line x1="9" y1="12" x2="10" y2="12"></line>
                        <line x1="14" y1="12" x2="15" y2="12"></line>
                        <line x1="9" y1="16" x2="10" y2="16"></line>
                        <line x1="14" y1="16" x2="15" y2="16"></line>
                      </svg>
                    </span>
                    <span class="tree-section-label">任务</span>
                    <span class="leaf-count">{(props.tasks ?? []).length}</span>
                    <span class="tree-section-arrow">{tasksSectionExpanded() ? "▼" : "▶"}</span>
                  </div>
                  <Show when={tasksSectionExpanded()}>
                    <div class="tree-section-content" style="display: flex; flex-direction: column; gap: 1px;">
                      <For each={props.tasks ?? []}>
                        {(task) => (
                          <div
                            class="tree-leaf task-leaf"
                            classList={{ selected: props.activeTaskId === task.id }}
                            onClick={() => props.onSelectTask?.(task.id)}
                            style="padding-left: 8px; display: flex; align-items: center; gap: 8px; position: relative;"
                          >
                            <span
                              class="task-kind-icon"
                              classList={{
                                "chat-icon": (task.kind ?? "sql") === "chat",
                                "sql-icon": (task.kind ?? "sql") !== "chat",
                              }}
                              title={(task.kind ?? "sql") === "chat" ? "对话" : "SQL 查询"}
                            >
                              <Show
                                when={(task.kind ?? "sql") === "chat"}
                                fallback={
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px; display: block;">
                                    <polyline points="4 17 10 11 4 5"></polyline>
                                    <line x1="12" y1="19" x2="20" y2="19"></line>
                                  </svg>
                                }
                              >
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px; display: block;">
                                  <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path>
                                </svg>
                              </Show>
                            </span>
                            <span class="leaf-label">{task.name}</span>
                            <span class="task-time" style="font-size: 10px; color: var(--text-dim); margin-left: auto; padding-left: 8px; flex-shrink: 0;">
                              {formatRelativeTime(task.createdAt)}
                            </span>
                          </div>
                        )}
                      </For>
                      <Show when={(props.tasks ?? []).length === 0}>
                        <div class="empty-section-item" style="padding: 4px 8px 4px 8px; color: var(--text-dim); font-size: 11px; font-style: italic; text-align: left;">
                          暂无任务
                        </div>
                      </Show>
                    </div>
                  </Show>

                  {/* Category 2: 文件 */}
                  <div
                    class="tree-section-header"
                    onClick={(e) => {
                      e.stopPropagation();
                      setFilesSectionExpanded(!filesSectionExpanded());
                    }}
                  >
                    <span class="tree-section-icon">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: block;">
                        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                      </svg>
                    </span>
                    <span class="tree-section-label">文件</span>
                    <span class="tree-section-arrow">{filesSectionExpanded() ? "▼" : "▶"}</span>
                  </div>
                  <Show when={filesSectionExpanded()}>
                    <div class="tree-section-content" style="display: flex; flex-direction: column; gap: 1px;">
                      {renderFileTree(ws.path)}
                      <Show when={!(directoryContents()[ws.path]?.length > 0)}>
                        <div class="empty-section-item" style="padding: 4px 8px 4px 8px; color: var(--text-dim); font-size: 11px; font-style: italic; text-align: left;">
                          暂无文件
                        </div>
                      </Show>
                    </div>
                  </Show>

                  {/* Import progress banner */}
                  <Show when={props.importStatus}>
                    {(st) => (
                      <div
                        class="import-banner"
                        classList={{
                          "import-banner--done": st().stage === "done",
                          "import-banner--error": st().stage === "error",
                        }}
                      >
                        <Show when={st().stage === "done"} fallback={
                          <Show when={st().stage === "error"} fallback={
                            <span class="import-banner__spinner" />
                          }>
                            <span class="import-banner__icon import-banner__icon--error">✕</span>
                          </Show>
                        }>
                          <span class="import-banner__icon import-banner__icon--done">✓</span>
                        </Show>
                        <span class="import-banner__text">
                          {st().stage === "done"
                            ? `${st().file} → ${st().table ?? ""}（${st().columns ?? 0}列${st().rows != null ? `, ${st().rows}行` : ""}）`
                            : st().stage === "error"
                            ? `${st().file}：${st().error ?? "导入失败"}`
                            : `${st().file} → ${stageLabel(st().stage)}${st().table ? ` ${st().table}` : ""}…`}
                        </span>
                      </div>
                    )}
                  </Show>

                  {/* Category: 外部数据库 */}
                  <div
                    class="tree-section-header"
                    onClick={(e) => {
                      e.stopPropagation();
                      setDbSectionExpanded(!dbSectionExpanded());
                    }}
                  >
                    <span class="tree-section-icon">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: block;">
                        <rect x="2" y="3" width="20" height="8" rx="2" ry="2"></rect>
                        <rect x="2" y="13" width="20" height="8" rx="2" ry="2"></rect>
                        <line x1="6" y1="7" x2="6.01" y2="7"></line>
                        <line x1="6" y1="17" x2="6.01" y2="17"></line>
                      </svg>
                    </span>
                    <span class="tree-section-label">{t("externalData")}</span>
                    <span class="leaf-count">{workspaceConns().length}</span>
                    <span class="tree-section-arrow">{dbSectionExpanded() ? "▼" : "▶"}</span>
                  </div>
                  <Show when={dbSectionExpanded()}>
                    <div class="tree-section-content" style="display: flex; flex-direction: column; gap: 1px;">
                      <For each={workspaceConns()}>
                        {(conn) => {
                          const connId = conn.id;
                          const isExpanded = () => expandedDbConns()[connId];
                          const isLoading = () => loadingDbConns()[connId];
                          const tables = () => dbTables()[connId] || [];
                          
                          const schemas = () => {
                            const map: Record<string, DbTableItem[]> = {};
                            for (const t of tables()) {
                              const sch = t.schema || "public";
                              if (!map[sch]) map[sch] = [];
                              map[sch].push(t);
                            }
                            return Object.entries(map);
                          };

                          return (
                            <div class="tree-subgroup">
                              <div 
                                class="tree-group-label" 
                                style="display: flex; align-items: center; justify-content: space-between; padding: 4px 8px; cursor: pointer; border-radius: 4px; hover: background: rgba(255,255,255,0.02);"
                                onClick={() => toggleDbConn(conn)}
                              >
                                <div style="display: flex; align-items: center; gap: 6px;">
                                  <span style="display: inline-flex; align-items: center;">
                                    <Show when={conn.dbType === "postgres"} fallback={
                                      <span style="font-size: 8px; font-weight: 800; background: rgba(255, 140, 0, 0.16); color: #ffa500; width: 13px; height: 13px; display: inline-flex; align-items: center; justify-content: center; border-radius: 3px; line-height: 1; font-family: system-ui, -apple-system, sans-serif; flex-shrink: 0;">M</span>
                                    }>
                                      <span style="font-size: 8px; font-weight: 800; background: rgba(80, 160, 255, 0.16); color: var(--brand); width: 13px; height: 13px; display: inline-flex; align-items: center; justify-content: center; border-radius: 3px; line-height: 1; font-family: system-ui, -apple-system, sans-serif; flex-shrink: 0;">P</span>
                                    </Show>
                                  </span>
                                  <span style="font-weight: 500; font-size: 12px;">{conn.name}</span>
                                </div>
                                <div style="display: flex; align-items: center; gap: 6px;">
                                  <Show when={isLoading()}>
                                    <span class="import-banner__spinner" style="width: 10px; height: 10px; border-width: 1.5px;" />
                                  </Show>
                                  <span style="font-size: 10px; color: var(--text-dim);">{isExpanded() ? "▼" : "▶"}</span>
                                </div>
                              </div>

                              <Show when={isExpanded() && !isLoading()}>
                                <div style="margin-left: 12px; display: flex; flex-direction: column; gap: 2px;">
                                  <Show when={tables().length === 0}>
                                    <div style="font-size: 11px; font-style: italic; color: var(--text-dim); padding: 4px 8px;">
                                      {t("noTables")}
                                    </div>
                                  </Show>

                                  <Show when={conn.dbType === "postgres"} fallback={
                                    <For each={tables()}>
                                      {(t) => {
                                        const registered = () => props.sources.some(s => s.path === `db://${connId}/${t.schema}/${t.name}`);
                                        return (
                                          <div style="display: flex; align-items: center; justify-content: space-between; padding: 2px 4px 2px 8px; border-radius: 4px;" class="tree-leaf">
                                            <div style="display: flex; align-items: center; gap: 6px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
                                              <span style="color: var(--text-dim); display: inline-flex;"><KindIcon kind="table" /></span>
                                              <span style="font-size: 11.5px;">{t.name}</span>
                                            </div>
                                            <Show when={registered()} fallback={
                                              <button 
                                                class="ss-btn" 
                                                style="padding: 1px 6px; font-size: 10px; height: 18px; border-radius: 3px;"
                                                onClick={(e) => { e.stopPropagation(); handleRegisterDbTable(conn, t); }}
                                                disabled={props.busy}
                                              >
                                                {t("addBtn")}
                                              </button>
                                            }>
                                              <span style="color: var(--text-success); font-size: 10px; padding-right: 4px;">{t("addedLabel")}</span>
                                            </Show>
                                          </div>
                                        );
                                      }}
                                    </For>
                                  }>
                                    <For each={schemas()}>
                                      {([schemaName, schemaTables]) => {
                                        const [schemaExpanded, setSchemaExpanded] = createSignal(schemaName === "public");
                                        return (
                                          <div style="display: flex; flex-direction: column; gap: 1px;">
                                            <div 
                                              style="display: flex; align-items: center; gap: 4px; padding: 2px 4px; cursor: pointer; color: var(--text-dim);"
                                              onClick={() => setSchemaExpanded(!schemaExpanded())}
                                            >
                                              <span style="font-size: 10px;">{schemaExpanded() ? "▼" : "▶"}</span>
                                              <span style="font-size: 11px; font-weight: 500;">📁 {schemaName}</span>
                                            </div>
                                            <Show when={schemaExpanded()}>
                                              <div style="margin-left: 10px; display: flex; flex-direction: column; gap: 1px;">
                                                <For each={schemaTables}>
                                                  {(t) => {
                                                    const registered = () => props.sources.some(s => s.path === `db://${connId}/${t.schema}/${t.name}`);
                                                    return (
                                                      <div style="display: flex; align-items: center; justify-content: space-between; padding: 2px 4px 2px 8px; border-radius: 4px;" class="tree-leaf">
                                                        <div style="display: flex; align-items: center; gap: 6px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">
                                                          <span style="color: var(--text-dim); display: inline-flex;"><KindIcon kind="table" /></span>
                                                          <span style="font-size: 11.5px;" title={t.name}>{t.name}</span>
                                                        </div>
                                                        <Show when={registered()} fallback={
                                                          <button 
                                                            class="ss-btn" 
                                                            style="padding: 1px 6px; font-size: 10px; height: 18px; border-radius: 3px;"
                                                            onClick={(e) => { e.stopPropagation(); handleRegisterDbTable(conn, t); }}
                                                            disabled={props.busy}
                                                          >
                                                            {t("addBtn")}
                                                          </button>
                                                        }>
                                                          <span style="color: var(--text-success); font-size: 10px; padding-right: 4px;">{t("addedLabel")}</span>
                                                        </Show>
                                                      </div>
                                                    );
                                                  }}
                                                </For>
                                              </div>
                                            </Show>
                                          </div>
                                        );
                                      }}
                                    </For>
                                  </Show>
                                </div>
                              </Show>
                            </div>
                          );
                        }}
                      </For>
                      <Show when={workspaceConns().length === 0}>
                        <div style="padding: 10px; text-align: center; color: var(--text-dim); font-size: 11px; font-style: italic;">
                          {t("noLinkedConns")}
                          <a href="#" style="color: var(--brand); text-decoration: underline; margin-left: 4px;" onClick={(e) => { e.preventDefault(); props.onOpenSettings(); }}>
                            {t("settingsPageLink")}
                          </a>
                        </div>
                      </Show>
                    </div>
                  </Show>

                  {/* Category 3: 数据 */}
                  <div
                    class="tree-section-header"
                    onClick={(e) => {
                      e.stopPropagation();
                      setDataSectionExpanded(!dataSectionExpanded());
                    }}
                  >
                    <span class="tree-section-icon">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: block;">
                        <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"></path>
                        <path d="M3 12c0 1.66 4 3 9 3s9-1.34 9-3"></path>
                      </svg>
                    </span>
                    <span class="tree-section-label">数据</span>
                    <span class="leaf-count">{props.sources.length}</span>
                    <span class="tree-section-arrow">{dataSectionExpanded() ? "▼" : "▶"}</span>
                  </div>
                  <Show when={dataSectionExpanded()}>
                    <div class="tree-section-content" style="display: flex; flex-direction: column; gap: 1px;">
                      <For each={groups()}>
                        {(group) => {
                          const hasDir = !!group[0]; // directory group vs flat (agent-created)
                          return (
                          <div class="tree-subgroup" style={{ "margin-left": hasDir ? "12px" : "0" }}>
                            <Show when={hasDir}>
                              <div class="tree-group-label" title={group[0]} style="display: flex; align-items: center; gap: 6px; padding: 4px 8px 4px 0;">
                                <span style="display: inline-flex; align-items: center; justify-content: center; color: var(--text-dim);">
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                                    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                                  </svg>
                                </span>
                                <span>{shortDir(group[0])}</span>
                              </div>
                            </Show>
                            <For each={group[1]}>
                              {(t) => (
                                <button
                                  class="tree-leaf"
                                  classList={{ selected: props.selected === t.name }}
                                  disabled={props.busy}
                                  title={t.path.split("/").pop() ?? t.path}
                                  onClick={() => handleSelectTable(t)}
                                  onContextMenu={(e) => {
                                    e.preventDefault();
                                    setCtxMenu({ name: t.name, x: e.clientX, y: e.clientY });
                                  }}
                                  style={{
                                    "padding-left": "8px",
                                    background:
                                      highlightTable() === t.name && props.selected !== t.name
                                        ? "rgba(80, 160, 255, 0.12)"
                                        : undefined,
                                  }}
                                >
                                  <Show when={categoryOf(t.name)} fallback={
                                    <span class="kind-badge kind-badge--icon" data-kind={t.kind} title={t.kind}><KindIcon kind={t.kind} /></span>
                                  }>
                                    {(cat) => (
                                      <span class="kind-badge kind-badge--icon kind-badge--category-icon" data-category={cat().label} data-kind={t.kind} title={cat().title}>
                                        <KindIcon kind={t.kind} />
                                      </span>
                                    )}
                                  </Show>
                                  <span class="leaf-label">{t.name}</span>
                                  <Show when={t.storage === "view"}>
                                    <span class="leaf-storage" title="零拷贝视图(直接读源文件,不复制)">👁</span>
                                  </Show>
                                  <Show when={t.rowCountEstimate != null}>
                                    <span class="leaf-count">{formatCount(t.rowCountEstimate!)}</span>
                                  </Show>
                                  <Show when={t.partitionKeys.length > 0}>
                                    <span class="leaf-part" title={`Hive partitions: ${t.partitionKeys.join(", ")}`}>
                                      🗂 {t.partitionKeys.length}
                                    </span>
                                  </Show>
                                </button>
                              )}
                            </For>
                          </div>
                          );
                        }}
                      </For>
                      <Show when={props.sources.length === 0}>
                        <div class="empty-section-item" style="padding: 4px 8px 4px 8px; color: var(--text-dim); font-size: 11px; font-style: italic; text-align: left;">
                          暂无数据
                        </div>
                      </Show>
                    </div>
                  </Show>
                </Show>
              </div>
            );
          }}
        </For>
      </div>

      <div class="ln-footer">
        <button
          class="ln-brand"
          title={t("settings")}
          onClick={() => props.onOpenSettings()}
        >
          <img src={logoSrc()} alt="LakeMind" style="width: 18px; height: 18px; object-fit: contain;" />
          <span class="ln-brand-name">LakeMind</span>
        </button>
      </div>
    </nav>
  );
}

function shortDir(path: string): string {
  if (!path) return ""; // agent-created objects (empty path) are not grouped
  const segs = path.split(/[\\/]/).filter(Boolean);
  return segs.slice(-1)[0] || path; // Show only the directory name for cleaner ZCode layout
}

/** Classify an agent-created table/view by its naming prefix.
 * Returns null for source objects (`s_`) and anything unrecognized — those
 * keep their existing kind badge instead of a category badge. */
function categoryOf(name: string): { label: string; title: string } | null {
  if (name.startsWith("tmp_v_")) return { label: "TMPV", title: "中间过渡虚拟视图" };
  if (name.startsWith("tmp_")) return { label: "TMP", title: "中间过渡物理表" };
  if (name.startsWith("v_")) return { label: "VIEW", title: "最终清洗加工后的虚拟视图" };
  if (name.startsWith("t_")) return { label: "TABLE", title: "最终清洗加工后的物理表" };
  return null;
}

/** Map a filename's extension to the same `kind` label/badge used in the Data tree,
 * so a file and its registered table share an identical icon. */
function fileKind(name: string): string {
  const ext = name.split('.').pop()?.toLowerCase() ?? '';
  if (ext === 'csv' || ext === 'tsv') return 'csv';
  if (ext === 'parquet' || ext === 'parq') return 'parquet';
  if (ext === 'json' || ext === 'ndjson') return 'json';
  if (ext === 'xlsx' || ext === 'xls') return 'excel';
  if (ext === 'delta') return 'delta';
  return ext || 'file';
}

function formatCount(n: number): string {
  if (n >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(0) + "K";
  return String(n);
}

function formatRelativeTime(ts: number): string {
  const diffMs = Date.now() - ts;
  const diffMins = Math.floor(diffMs / 60000);
  if (diffMins < 1) return "刚刚";
  if (diffMins < 60) return `${diffMins}分钟`;
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}小时`;
  const diffDays = Math.floor(diffHours / 24);
  return `${diffDays}天`;
}
