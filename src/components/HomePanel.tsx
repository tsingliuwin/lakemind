import { createSignal, Show, For, onMount, onCleanup } from "solid-js";
import type { Workspace } from "../lib/types";

interface HomePanelProps {
  workspace: string;
  workspaces: Workspace[];
  onSelectWorkspace: (path: string) => void;
  onAddWorkspace: (path: string) => void;
  onCreateTask: (prompt: string, modelId: string) => void;
  onAddSource?: () => void;
  availableModels: string[];
  selectedModel: string;
  onSelectModel: (model: string) => void;
  selectedPriority: string;
  onSelectPriority: (priority: string) => void;
  selectedConfirm: string;
  onSelectConfirm: (mode: string) => void;
}

export default function HomePanel(props: HomePanelProps) {
  const [inputValue, setInputValue] = createSignal("");
  const [wsMenuOpen, setWsMenuOpen] = createSignal(false);
  const [searchQuery, setSearchQuery] = createSignal("");
  
  // Custom dropdown states for model, confirmation, priority
  const [modelDropdownOpen, setModelDropdownOpen] = createSignal(false);

  const [confirmDropdownOpen, setConfirmDropdownOpen] = createSignal(false);

  const [priorityDropdownOpen, setPriorityDropdownOpen] = createSignal(false);

  let wsRef!: HTMLDivElement;
  let modelRef!: HTMLDivElement;
  let confirmRef!: HTMLDivElement;
  let priorityRef!: HTMLDivElement;

  const handleClickOutside = (e: MouseEvent) => {
    if (wsRef && !wsRef.contains(e.target as Node)) {
      setWsMenuOpen(false);
    }
    if (modelRef && !modelRef.contains(e.target as Node)) {
      setModelDropdownOpen(false);
    }
    if (confirmRef && !confirmRef.contains(e.target as Node)) {
      setConfirmDropdownOpen(false);
    }
    if (priorityRef && !priorityRef.contains(e.target as Node)) {
      setPriorityDropdownOpen(false);
    }
  };

  onMount(() => {
    document.addEventListener("mousedown", handleClickOutside);
    onCleanup(() => {
      document.removeEventListener("mousedown", handleClickOutside);
    });
  });

  const filteredWorkspaces = () => {
    const query = searchQuery().toLowerCase().trim();
    if (!query) return props.workspaces;
    return props.workspaces.filter(ws => ws.name.toLowerCase().includes(query));
  };

  const handleOpenFolder = async () => {
    setWsMenuOpen(false);
    try {
      const { selectDirectory } = await import("../lib/duckdb");
      const path = await selectDirectory();
      if (path) {
        props.onAddWorkspace(path);
      }
    } catch (err) {
      const path = prompt("请输入要打开的本地文件夹路径或项目名称：", "new_project");
      if (path) {
        const name = path.split(/[\\/]/).filter(Boolean).pop() || path;
        props.onAddWorkspace(name);
      }
    }
  };


  const handleSubmit = () => {
    const val = inputValue().trim();
    if (!val) return;
    props.onCreateTask(val, props.selectedModel);
    setInputValue("");
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    }
  };

  return (
    <div class="home-panel">

      <div class="home-content">
        <h2 class="home-title">开始在 {props.workspace} 项目新建任务</h2>

        {/* Input Pill Container */}
        <div class="input-pill">
          {/* Top Row: Workspace Selector */}
          <div class="pill-header">
            <div class="ws-dropdown-wrapper" ref={wsRef}>
              <button class="ws-trigger-btn" onClick={() => setWsMenuOpen(!wsMenuOpen())}>
                <span class="ws-icon">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                  </svg>
                </span>
                <span class="ws-name">{props.workspace}</span>
                <span class="ws-caret">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                    <polyline points="6 9 12 15 18 9"></polyline>
                  </svg>
                </span>
              </button>

              {/* Workspace Switcher Popover */}
              <Show when={wsMenuOpen()}>
                <div class="ws-popover">
                  <div class="ws-search-box">
                    <span class="search-icon">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                        <circle cx="11" cy="11" r="8"></circle>
                        <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
                      </svg>
                    </span>
                    <input 
                      type="text" 
                      placeholder="搜索工作区" 
                      value={searchQuery()} 
                      onInput={(e) => setSearchQuery(e.currentTarget.value)}
                      ref={(el) => setTimeout(() => el.focus(), 50)}
                    />
                  </div>
                  
                  <div class="ws-list">
                    <For each={filteredWorkspaces()}>
                      {(ws) => (
                        <button 
                          class="ws-item" 
                          classList={{ active: ws.name === props.workspace }}
                          onClick={() => {
                            props.onSelectWorkspace(ws.path);
                            setWsMenuOpen(false);
                          }}
                        >
                          <span class="ws-item-icon">
                            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                            </svg>
                          </span>
                          <span class="ws-item-label">{ws.name}</span>
                        </button>
                      )}
                    </For>
                    <button class="ws-item" onClick={handleOpenFolder}>
                      <span class="ws-item-icon">
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"></path>
                          <line x1="12" y1="11" x2="12" y2="17"></line>
                          <polyline points="9 14 12 11 15 14"></polyline>
                        </svg>
                      </span>
                      <span class="ws-item-label">打开文件夹</span>
                    </button>
                  </div>
                </div>
              </Show>
            </div>
          </div>

          {/* Inner Card wrapping Text Area and Toolbar */}
          <div class="pill-inner-card">
            {/* Text Area */}
            <div class="pill-body">
              <textarea
                placeholder="向 LakeMind 提问，或点击 + 添加数据文件"
                value={inputValue()}
                onInput={(e) => setInputValue(e.currentTarget.value)}
                onKeyDown={handleKeyDown}
                rows={2}
              />
            </div>

            {/* Bottom Toolbar Row */}
            <div class="pill-footer">
              <div class="footer-left">
                {/* Attachment Button */}
                <button 
                  class="pill-btn attachment-btn" 
                  title="添加文件 / @"
                  onClick={() => props.onAddSource?.()}
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <path d="M12 5v14M5 12h14"/>
                  </svg>
                </button>

                {/* Confirmation Mode Selector Dropdown */}
                <div class="dropdown-wrapper" ref={confirmRef}>
                  <button class="pill-btn select-btn" onClick={() => setConfirmDropdownOpen(!confirmDropdownOpen())}>
                    <span class="btn-prefix">
                      {props.selectedConfirm === "自动执行" ? (
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                          <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon>
                        </svg>
                      ) : (
                        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                          <path d="M9 11V6a2 2 0 0 1 4 0v5"></path>
                          <path d="M13 6a2 2 0 0 1 4 0v5"></path>
                          <path d="M17 6a2 2 0 0 1 4 0v8a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"></path>
                        </svg>
                      )}
                    </span>
                    <span>{props.selectedConfirm}</span>
                    <span class="btn-caret">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                        <polyline points="6 9 12 15 18 9"></polyline>
                      </svg>
                    </span>
                  </button>
                  <Show when={confirmDropdownOpen()}>
                    <div class="custom-dropdown-list fit-trigger">
                      <button class="dropdown-item" onClick={() => { props.onSelectConfirm("变更前确认"); setConfirmDropdownOpen(false); }}>
                        <span class="btn-prefix">
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                            <path d="M9 11V6a2 2 0 0 1 4 0v5"></path>
                            <path d="M13 6a2 2 0 0 1 4 0v5"></path>
                            <path d="M17 6a2 2 0 0 1 4 0v8a8 8 0 0 1-8 8h-2c-2.8 0-4.5-.86-5.99-2.34l-3.6-3.6a2 2 0 0 1 2.83-2.82L7 15"></path>
                          </svg>
                        </span> 变更前确认
                      </button>
                      <button class="dropdown-item" onClick={() => { props.onSelectConfirm("自动执行"); setConfirmDropdownOpen(false); }}>
                        <span class="btn-prefix">
                          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                            <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2"></polygon>
                          </svg>
                        </span> 自动执行
                      </button>
                    </div>
                  </Show>
                </div>
              </div>

              <div class="footer-right">
                {/* Model Selector Dropdown */}
                <div class="dropdown-wrapper" ref={modelRef}>
                  <button class="pill-btn select-btn" onClick={() => setModelDropdownOpen(!modelDropdownOpen())}>
                    <span class="model-icon">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                        <rect x="4" y="4" width="16" height="16" rx="2"></rect>
                        <rect x="9" y="9" width="6" height="6"></rect>
                        <line x1="9" y1="1" x2="9" y2="4"></line>
                        <line x1="15" y1="1" x2="15" y2="4"></line>
                        <line x1="9" y1="20" x2="9" y2="23"></line>
                        <line x1="15" y1="20" x2="15" y2="23"></line>
                        <line x1="20" y1="9" x2="23" y2="9"></line>
                        <line x1="20" y1="14" x2="23" y2="14"></line>
                        <line x1="1" y1="9" x2="4" y2="9"></line>
                        <line x1="1" y1="14" x2="4" y2="14"></line>
                      </svg>
                    </span>
                    <span>{props.selectedModel || "选择模型"}</span>
                    <span class="btn-caret">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                        <polyline points="6 9 12 15 18 9"></polyline>
                      </svg>
                    </span>
                  </button>
                  <Show when={modelDropdownOpen()}>
                    <div class="custom-dropdown-list">
                      <Show
                        when={props.availableModels.length > 0}
                        fallback={
                          <div class="dropdown-item muted" style="font-size: 12px; pointer-events: none; padding: 6px 12px;">
                            无可用模型，请先去设置配置
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
                <div class="dropdown-wrapper" ref={priorityRef}>
                  <button class="pill-btn select-btn" onClick={() => setPriorityDropdownOpen(!priorityDropdownOpen())}>
                    <span class="btn-prefix">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 13px; height: 13px;">
                        <path d="M12 5a3 3 0 1 0-5.997.125 4 4 0 0 0-2.526 5.77 4 4 0 0 0 .556 6.588A4 4 0 0 0 12 18a3 3 0 0 0 0-6 3 3 0 0 0 0-6Z"></path>
                        <path d="M12 5a3 3 0 1 1 5.997.125 4 4 0 0 1 2.526 5.77 4 4 0 0 1-.556 6.588A4 4 0 0 1 12 18"></path>
                      </svg>
                    </span>
                    <span>{props.selectedPriority}</span>
                    <span class="btn-caret">
                      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                        <polyline points="6 9 12 15 18 9"></polyline>
                      </svg>
                    </span>
                  </button>
                  <Show when={priorityDropdownOpen()}>
                    <div class="custom-dropdown-list fit-trigger">
                      <button class="dropdown-item" onClick={() => { props.onSelectPriority("最高"); setPriorityDropdownOpen(false); }}>
                        最高
                      </button>
                      <button class="dropdown-item" onClick={() => { props.onSelectPriority("均衡"); setPriorityDropdownOpen(false); }}>
                        均衡
                      </button>
                      <button class="dropdown-item" onClick={() => { props.onSelectPriority("最快"); setPriorityDropdownOpen(false); }}>
                        最快
                      </button>
                    </div>
                  </Show>
                </div>

                {/* Send Button */}
                <button 
                  class="send-btn" 
                  classList={{ active: inputValue().trim().length > 0 }}
                  disabled={inputValue().trim().length === 0}
                  onClick={handleSubmit}
                  title="发送任务"
                >
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                    <line x1="12" y1="19" x2="12" y2="5"></line>
                    <polyline points="5 12 12 5 19 12"></polyline>
                  </svg>
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
