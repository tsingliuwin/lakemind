import { For, Show, createMemo, createSignal, onMount, onCleanup, createEffect } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import type { SourceTable, QueryTask, Workspace, FileItem } from "../lib/types";
import { t, currentLanguage, setCurrentLanguage } from "../lib/i18n";
import { currentTheme, setCurrentTheme, currentZoom, setCurrentZoom, logoSrc } from "../lib/theme";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

/**
 * Left navigation styled like ZCode 3.0:
 * - Top-bar with Z logo and navigation arrows (<- and ->).
 * - Quick actions: "新建查询", "快速检索", "扩展函数".
 * - Workspace section header ("工作区" label with buttons).
 * - Tree list grouped by directory.
 * - Bottom footer with a logo ("研途教育"), a layout switcher, and settings gear.
 */
export default function LeftNav(props: {
  workspace: string;
  workspacePath?: string;
  workspaces?: Workspace[];
  tasks?: QueryTask[];
  activeTaskId?: string | null;
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
  onDisconnect?: () => void;
  onImportFile?: (filePath: string) => void;
  leftOpen?: boolean;
  onToggleLeft?: () => void;
}) {
  // Group tables by their parent directory for a tree-like feel.
  const groups = createMemo(() => {
    const map = new Map<string, SourceTable[]>();
    for (const t of props.sources) {
      const slash = Math.max(t.path.lastIndexOf("/"), t.path.lastIndexOf("\\"));
      const group = slash >= 0 ? t.path.slice(0, slash) : t.path;
      const arr = map.get(group) ?? [];
      arr.push(t);
      map.set(group, arr);
    }
    return [...map.entries()];
  });

  const [userMenuOpen, setUserMenuOpen] = createSignal(false);
  const [activeSubmenu, setActiveSubmenu] = createSignal<"language" | "theme" | "zoom" | "quota" | null>(null);
  let userMenuDropdownRef!: HTMLDivElement;
  let userBadgeRef!: HTMLButtonElement;

  // File explorer states
  const [expandedPaths, setExpandedPaths] = createSignal<Record<string, boolean>>({});
  const [directoryContents, setDirectoryContents] = createSignal<Record<string, FileItem[]>>({});
  const [activeActionWsPath, setActiveActionWsPath] = createSignal<string | null>(null);
  const [fileSearchQuery] = createSignal("");

  // File ↔ Data cross-highlighting (linkage). Clicking a table highlights its
  // backing file in the Files tree, and clicking a file highlights its table.
  const [highlightFile, setHighlightFile] = createSignal<string | null>(null);
  const [highlightTable, setHighlightTable] = createSignal<string | null>(null);

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
    setHighlightFile(item.path);
    setHighlightTable(fileToTable().get(item.path) ?? null);
    props.onImportFile?.(item.path);
  };

  // Subsections expanded states
  const [tasksSectionExpanded, setTasksSectionExpanded] = createSignal(true);
  const [filesSectionExpanded, setFilesSectionExpanded] = createSignal(true);
  const [dataSectionExpanded, setDataSectionExpanded] = createSignal(true);

  // Automatically load root directory contents when workspace changes
  createEffect(async () => {
    const wsPath = props.workspacePath;
    if (wsPath) {
      setExpandedPaths((prev) => ({ ...prev, [wsPath]: true }));
      await loadDirContents(wsPath);
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

  const handleClickOutside = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    if (
      userMenuOpen() &&
      userMenuDropdownRef &&
      !userMenuDropdownRef.contains(target) &&
      (!userBadgeRef || !userBadgeRef.contains(target))
    ) {
      setUserMenuOpen(false);
      setActiveSubmenu(null);
    }
    if (!target.closest(".ws-action-icon-btn") && !target.closest(".ws-action-popover")) {
      setActiveActionWsPath(null);
    }
  };

  const renderFileTree = (dirPath: string, depth: number = 0) => {
    const contents = directoryContents()[dirPath] || [];
    const query = fileSearchQuery().trim().toLowerCase();

    const filteredContents = query
      ? contents.filter(item => item.name.toLowerCase().includes(query))
      : contents;

    return (
      <div class="fe-tree-container" style={{ "padding-left": `${depth > 0 ? 12 : 28}px` }}>
        <For each={filteredContents}>
          {(item) => {
            const isExpanded = !!expandedPaths()[item.path];
            return (
              <div class="fe-tree-node">
                <div
                  class="fe-node-row"
                  classList={{ "is-dir": item.is_dir }}
                  style={{
                     display: "flex",
                     "align-items": "center",
                     padding: "4px 8px",
                     "border-radius": "4px",
                     cursor: "pointer",
                     transition: "background 0.1s",
                     background: highlightFile() === item.path ? "rgba(80, 160, 255, 0.14)" : undefined,
                     "box-shadow": highlightFile() === item.path ? "inset 2px 0 0 var(--accent-blue, #50a0ff)" : undefined,
                  }}
                  onClick={() => {
                    if (item.is_dir) {
                      toggleFolder(item.path);
                    } else {
                      handleFileClick(item);
                    }
                  }}
                >
                  <Show
                    when={item.is_dir}
                    fallback={
                      <span class="kind-badge" data-kind={fileKind(item.name)} style="margin-right: 6px;">
                        {fileKind(item.name)}
                      </span>
                    }
                  >
                    <span class="fe-node-icon" style="margin-right: 6px; font-size: 11px;">
                      {isExpanded ? "▾ 📁" : "▸ 📁"}
                    </span>
                  </Show>
                  <span class="fe-node-name" style="flex: 1; font-size: 12px; text-align: left; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--text-secondary);">
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

  onMount(() => {
    document.addEventListener("mousedown", handleClickOutside);
    onCleanup(() => {
      document.removeEventListener("mousedown", handleClickOutside);
    });
  });

  return (
    <nav class="leftnav">
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

          <button class="ln-arrow-btn" title="后退" disabled={props.busy}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <line x1="19" y1="12" x2="5" y2="12"></line>
              <polyline points="12 19 5 12 12 5"></polyline>
            </svg>
          </button>
          <button class="ln-arrow-btn" title="前进" disabled={props.busy}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <line x1="5" y1="12" x2="19" y2="12"></line>
              <polyline points="12 5 19 12 12 19"></polyline>
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
        <button class="ln-action-btn" title={`${t("search")} (Ctrl+K)`} disabled={props.busy}>
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="11" cy="11" r="8"></circle>
              <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
            </svg>
          </span>
          <span class="action-label">搜索</span>
          <span class="action-shortcut">⌘ K</span>
        </button>
        <button class="ln-action-btn" title={t("skills")} disabled={props.busy}>
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polygon points="12 2 2 7 12 12 22 7 12 2"></polygon>
              <polyline points="2 17 12 22 22 17"></polyline>
              <polyline points="2 12 12 17 22 12"></polyline>
            </svg>
          </span>
          <span class="action-label">技能</span>
          <span class="action-shortcut"></span>
        </button>
      </div>

      {/* Workspace header */}
      <div class="ln-section-header">
        <span class="section-title">工作区 <span class="ws-indicator-dot" /></span>
        <div class="section-actions">
          <button class="sec-act-btn" title="筛选/排序">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="4" y1="21" x2="4" y2="14"></line>
              <line x1="4" y1="10" x2="4" y2="3"></line>
              <line x1="12" y1="21" x2="12" y2="12"></line>
              <line x1="12" y1="8" x2="12" y2="3"></line>
              <line x1="20" y1="21" x2="20" y2="16"></line>
              <line x1="20" y1="12" x2="20" y2="3"></line>
              <line x1="1" y1="14" x2="7" y2="14"></line>
              <line x1="9" y1="8" x2="15" y2="8"></line>
              <line x1="17" y1="16" x2="23" y2="16"></line>
            </svg>
          </button>
          <button class="sec-act-btn" title="搜索表">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="11" cy="11" r="8"></circle>
              <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
            </svg>
          </button>
          <button class="sec-act-btn" title="收起全部">
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
                  <span style="flex: 1; text-align: left;">{ws.name}</span>
                  
                  {/* Action buttons shown on hover */}
                  <div class="ws-hover-actions">
                    <button class="ws-action-icon-btn" title="更多" onClick={(e) => {
                      e.stopPropagation();
                      setActiveActionWsPath(activeActionWsPath() === ws.path ? null : ws.path);
                    }}>
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                        <circle cx="12" cy="12" r="1"></circle>
                        <circle cx="19" cy="12" r="1"></circle>
                        <circle cx="5" cy="12" r="1"></circle>
                      </svg>
                    </button>
                    <button class="ws-action-icon-btn" title="查看文件" onClick={(e) => {
                      e.stopPropagation();
                      props.onSelectWorkspace?.(ws.path);
                      setFilesSectionExpanded(true);
                    }}>
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                        <line x1="3" y1="6" x2="18" y2="6"></line>
                        <line x1="6" y1="12" x2="18" y2="12"></line>
                        <line x1="6" y1="18" x2="18" y2="18"></line>
                      </svg>
                    </button>
                    <button class="ws-action-icon-btn" title="新建任务" onClick={(e) => {
                      e.stopPropagation();
                      props.onSelectWorkspace?.(ws.path);
                      props.onNewQuery?.();
                    }}>
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
                        <path d="M21 11.5a8.38 8.38 0 0 1-.9 3.8 8.5 8.5 0 0 1-7.6 4.7 8.38 8.38 0 0 1-3.8-.9L3 21l1.9-5.7a8.38 8.38 0 0 1-.9-3.8 8.5 8.5 0 0 1 4.7-7.6 8.38 8.38 0 0 1 3.8-.9h.5a8.48 8.48 0 0 1 8 8v.5z"></path>
                        <line x1="12" y1="9" x2="12" y2="15"></line>
                        <line x1="9" y1="12" x2="15" y2="12"></line>
                      </svg>
                    </button>
                  </div>

                  {/* Action popover menu */}
                  <Show when={activeActionWsPath() === ws.path}>
                    <div class="ws-action-popover" onClick={(e) => e.stopPropagation()}>
                      <button class="ws-action-popover-item remove-item" onClick={() => {
                        props.onRemoveWorkspace?.(ws.path);
                        setActiveActionWsPath(null);
                      }}>
                        <span class="remove-icon">✕</span>
                        <span>移除</span>
                      </button>
                    </div>
                  </Show>

                  <Show when={isActive() && activeActionWsPath() !== ws.path}>
                    <span style="font-size: 8px; color: var(--accent-blue);">●</span>
                  </Show>
                </div>

                {/* If active, render its tasks, files and data */}
                <Show when={isActive()}>
                  {/* Category 1: 任务 */}
                  <div
                    class="tree-section-header"
                    onClick={(e) => {
                      e.stopPropagation();
                      setTasksSectionExpanded(!tasksSectionExpanded());
                    }}
                  >
                    <span class="tree-section-arrow">{tasksSectionExpanded() ? "▼" : "▶"}</span>
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
                  </div>
                  <Show when={tasksSectionExpanded()}>
                    <div class="tree-section-content" style="display: flex; flex-direction: column; gap: 1px;">
                      <For each={props.tasks ?? []}>
                        {(task) => (
                          <div
                            class="tree-leaf task-leaf"
                            classList={{ selected: props.activeTaskId === task.id }}
                            onClick={() => props.onSelectTask?.(task.id)}
                            style="padding-left: 28px; display: flex; align-items: center; gap: 6px; position: relative;"
                          >
                            <span class="task-kind-icon" title={(task.kind ?? "sql") === "chat" ? "对话" : "SQL 查询"} style="display: inline-flex; align-items: center; justify-content: center; width: 14px; height: 14px;">
                              <Show
                                when={(task.kind ?? "sql") === "chat"}
                                fallback={
                                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: block;">
                                    <polyline points="4 17 10 11 4 5"></polyline>
                                    <line x1="12" y1="19" x2="20" y2="19"></line>
                                  </svg>
                                }
                              >
                                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: block;">
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
                        <div class="empty-section-item" style="padding: 4px 8px 4px 28px; color: var(--text-dim); font-size: 11px; font-style: italic; text-align: left;">
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
                    <span class="tree-section-arrow">{filesSectionExpanded() ? "▼" : "▶"}</span>
                    <span class="tree-section-icon">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: block;">
                        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                      </svg>
                    </span>
                    <span class="tree-section-label">文件</span>
                  </div>
                  <Show when={filesSectionExpanded()}>
                    <div class="tree-section-content" style="display: flex; flex-direction: column; gap: 1px;">
                      {renderFileTree(ws.path)}
                      <Show when={!(directoryContents()[ws.path]?.length > 0)}>
                        <div class="empty-section-item" style="padding: 4px 8px 4px 28px; color: var(--text-dim); font-size: 11px; font-style: italic; text-align: left;">
                          暂无文件
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
                    <span class="tree-section-arrow">{dataSectionExpanded() ? "▼" : "▶"}</span>
                    <span class="tree-section-icon">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px; display: block;">
                        <ellipse cx="12" cy="5" rx="9" ry="3"></ellipse>
                        <path d="M3 5v14c0 1.66 4 3 9 3s9-1.34 9-3V5"></path>
                        <path d="M3 12c0 1.66 4 3 9 3s9-1.34 9-3"></path>
                      </svg>
                    </span>
                    <span class="tree-section-label">数据</span>
                    <span class="leaf-count">{props.sources.length}</span>
                  </div>
                  <Show when={dataSectionExpanded()}>
                    <div class="tree-section-content" style="display: flex; flex-direction: column; gap: 1px;">
                      <For each={groups()}>
                        {(group) => (
                          <div class="tree-subgroup" style="margin-left: 28px;">
                            <Show when={groups().length > 1}>
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
                                  title={t.scanPath}
                                  onClick={() => handleSelectTable(t)}
                                  style={{
                                    "padding-left": "8px",
                                    background:
                                      highlightTable() === t.name && props.selected !== t.name
                                        ? "rgba(80, 160, 255, 0.12)"
                                        : undefined,
                                    "box-shadow":
                                      highlightTable() === t.name && props.selected !== t.name
                                        ? "inset 2px 0 0 var(--accent-blue, #50a0ff)"
                                        : undefined,
                                  }}
                                >
                                  <span class="kind-badge" data-kind={t.kind}>{t.kind}</span>
                                  <span class="leaf-label">{t.name}</span>
                                  <Show when={t.storage === "view"}>
                                    <span class="leaf-storage" title="零拷贝视图(直接读源文件,不复制)">👁</span>
                                  </Show>
                                  <Show when={t.storage === "custom"}>
                                    <span class="leaf-storage" title="用户自建表/视图">✦</span>
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
                        )}
                      </For>
                      <Show when={props.sources.length === 0}>
                        <div class="empty-section-item" style="padding: 4px 8px 4px 28px; color: var(--text-dim); font-size: 11px; font-style: italic; text-align: left;">
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
          ref={userBadgeRef}
          class="ln-user-badge"
          classList={{ active: userMenuOpen() }}
          onClick={() => {
            const open = !userMenuOpen();
            setUserMenuOpen(open);
            if (!open) setActiveSubmenu(null);
          }}
        >
          <span class="user-avatar">研</span>
          <span class="user-name">研途教育</span>
        </button>

        {/* User Dropdown Menu */}
        <Show when={userMenuOpen()}>
          <div class="ln-user-dropdown" ref={userMenuDropdownRef}>
            
            {/* Language Submenu Trigger */}
            <div class="user-menu-item-wrapper">
              <button 
                class="user-menu-item" 
                classList={{ active: activeSubmenu() === "language" }}
                onClick={(e) => { e.stopPropagation(); setActiveSubmenu(activeSubmenu() === "language" ? null : "language"); }}
              >
                <span class="user-menu-icon">🌐</span>
                <span class="user-menu-label">{t("interfaceLanguage")}</span>
                <span class="user-menu-chevron">›</span>
              </button>
              <Show when={activeSubmenu() === "language"}>
                <div class="ln-user-submenu">
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentLanguage() === "zh" }}
                    onClick={() => { setCurrentLanguage("zh"); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    {t("langZh")}
                  </button>
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentLanguage() === "en" }}
                    onClick={() => { setCurrentLanguage("en"); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    {t("langEn")}
                  </button>
                </div>
              </Show>
            </div>

            {/* Theme Submenu Trigger */}
            <div class="user-menu-item-wrapper">
              <button 
                class="user-menu-item" 
                classList={{ active: activeSubmenu() === "theme" }}
                onClick={(e) => { e.stopPropagation(); setActiveSubmenu(activeSubmenu() === "theme" ? null : "theme"); }}
              >
                <span class="user-menu-icon">🎨</span>
                <span class="user-menu-label">{t("interfaceTheme")}</span>
                <span class="user-menu-chevron">›</span>
              </button>
              <Show when={activeSubmenu() === "theme"}>
                <div class="ln-user-submenu">
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentTheme() === "geek-dark" }}
                    onClick={() => { setCurrentTheme("geek-dark"); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    {t("themeGeekDark")}
                  </button>
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentTheme() === "classic-dark" }}
                    onClick={() => { setCurrentTheme("classic-dark"); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    {t("themeClassicDark")}
                  </button>
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentTheme() === "light" }}
                    onClick={() => { setCurrentTheme("light"); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    {t("themeLight")}
                  </button>
                </div>
              </Show>
            </div>

            {/* Zoom Submenu Trigger */}
            <div class="user-menu-item-wrapper">
              <button 
                class="user-menu-item" 
                classList={{ active: activeSubmenu() === "zoom" }}
                onClick={(e) => { e.stopPropagation(); setActiveSubmenu(activeSubmenu() === "zoom" ? null : "zoom"); }}
              >
                <span class="user-menu-icon">🔎</span>
                <span class="user-menu-label">{t("interfaceZoom")}</span>
                <span class="user-menu-chevron">›</span>
              </button>
              <Show when={activeSubmenu() === "zoom"}>
                <div class="ln-user-submenu">
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentZoom() === 80 }}
                    onClick={() => { setCurrentZoom(80); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    80%
                  </button>
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentZoom() === 90 }}
                    onClick={() => { setCurrentZoom(90); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    90%
                  </button>
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentZoom() === 100 }}
                    onClick={() => { setCurrentZoom(100); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    100%
                  </button>
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentZoom() === 110 }}
                    onClick={() => { setCurrentZoom(110); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    110%
                  </button>
                  <button 
                    class="submenu-item" 
                    classList={{ selected: currentZoom() === 120 }}
                    onClick={() => { setCurrentZoom(120); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    120%
                  </button>
                </div>
              </Show>
            </div>

            <div class="user-menu-divider" />

            <button class="user-menu-item" onClick={() => { setUserMenuOpen(false); setActiveSubmenu(null); props.onOpenSettings(); }}>
              <span class="user-menu-icon">⚙️</span>
              <span class="user-menu-label">{t("settings")}</span>
            </button>

            {/* Quota Submenu Trigger */}
            <div class="user-menu-item-wrapper">
              <button 
                class="user-menu-item" 
                classList={{ active: activeSubmenu() === "quota" }}
                onClick={(e) => { e.stopPropagation(); setActiveSubmenu(activeSubmenu() === "quota" ? null : "quota"); }}
              >
                <span class="user-menu-icon">⏳</span>
                <span class="user-menu-label">{t("remainingQuota")}</span>
                <span class="user-menu-chevron">›</span>
              </button>
              <Show when={activeSubmenu() === "quota"}>
                <div class="ln-user-submenu">
                  <button 
                    class="submenu-item" 
                    onClick={() => { alert(t("settingsM1Placeholder")); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    {t("quotaLocal")}
                  </button>
                  <button 
                    class="submenu-item" 
                    onClick={() => { alert(t("settingsM1Placeholder")); setActiveSubmenu(null); setUserMenuOpen(false); }}
                  >
                    {t("quotaUnlimited")}
                  </button>
                </div>
              </Show>
            </div>

            <button class="user-menu-item" onClick={() => { setUserMenuOpen(false); setActiveSubmenu(null); alert(t("settingsM1Placeholder")); }}>
              <span class="user-menu-icon">💬</span>
              <span class="user-menu-label">{t("feedback")}</span>
            </button>
            <button class="user-menu-item" onClick={() => { setUserMenuOpen(false); setActiveSubmenu(null); alert(t("settingsM1Placeholder")); }}>
              <span class="user-menu-icon">👥</span>
              <span class="user-menu-label">{t("community")}</span>
            </button>

            <div class="user-menu-divider" />

            <button class="user-menu-item disconnect-item" onClick={() => { setUserMenuOpen(false); setActiveSubmenu(null); props.onDisconnect?.(); }}>
              <span class="user-menu-icon">🚪</span>
              <span class="user-menu-label">{t("disconnect")}</span>
            </button>
          </div>
        </Show>

        <div class="ln-footer-actions">
          <button class="ln-foot-icon-btn" title={t("settings")} onClick={() => props.onOpenSettings()}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
          </button>
        </div>
      </div>
    </nav>
  );
}

function shortDir(path: string): string {
  if (!path) return "会话与过程表";
  const segs = path.split(/[\\/]/).filter(Boolean);
  return segs.slice(-1)[0] || path; // Show only the directory name for cleaner ZCode layout
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
