import { createSignal, Show, For, onMount, onCleanup } from "solid-js";
import type { Workspace } from "../lib/types";

interface HomePanelProps {
  workspace: string;
  workspaces: Workspace[];
  onSelectWorkspace: (path: string) => void;
  onAddWorkspace: (path: string) => void;
  onCreateTask: (prompt: string) => void;
}

export default function HomePanel(props: HomePanelProps) {
  const [inputValue, setInputValue] = createSignal("");
  const [wsMenuOpen, setWsMenuOpen] = createSignal(false);
  const [searchQuery, setSearchQuery] = createSignal("");
  
  // Custom dropdown states for model, confirmation, priority
  const [selectedModel, setSelectedModel] = createSignal("GLM-5.2");
  const [modelDropdownOpen, setModelDropdownOpen] = createSignal(false);
  
  const [selectedConfirm, setSelectedConfirm] = createSignal("变更前确认");

  const [selectedPriority, setSelectedPriority] = createSignal("最高");
  const [priorityDropdownOpen, setPriorityDropdownOpen] = createSignal(false);

  let wsRef!: HTMLDivElement;
  let modelRef!: HTMLDivElement;
  let priorityRef!: HTMLDivElement;

  const handleClickOutside = (e: MouseEvent) => {
    if (wsRef && !wsRef.contains(e.target as Node)) {
      setWsMenuOpen(false);
    }
    if (modelRef && !modelRef.contains(e.target as Node)) {
      setModelDropdownOpen(false);
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
    props.onCreateTask(val);
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
                <span class="ws-icon">📁</span>
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
                    <span class="search-icon">🔍</span>
                    <input 
                      type="text" 
                      placeholder="搜索工作区" 
                      value={searchQuery()} 
                      onInput={(e) => setSearchQuery(e.currentTarget.value)}
                      autofocus
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
                          <span class="ws-item-icon">📁</span>
                          <span class="ws-item-label">{ws.name}</span>
                          <Show when={ws.name === props.workspace}>
                            <span class="ws-item-check">✓</span>
                          </Show>
                        </button>
                      )}
                    </For>
                    <button class="ws-item" onClick={handleOpenFolder}>
                      <span class="ws-item-icon">📂</span>
                      <span class="ws-item-label">打开文件夹</span>
                    </button>
                  </div>
                </div>
              </Show>
            </div>
          </div>

          {/* Text Area */}
          <div class="pill-body">
            <textarea
              placeholder="向 ZCode 提问，输入 @ 添加文件, / 使用命令, $ 使用技能, # 关联对话"
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
              <button class="pill-btn attachment-btn" title="添加文件 / @">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                  <path d="M12 5v14M5 12h14"/>
                </svg>
              </button>

              {/* Confirmation Mode Selector (Parallel Buttons) */}
              <button 
                class="pill-btn select-btn" 
                classList={{ active: selectedConfirm() === "变更前确认" }}
                onClick={() => setSelectedConfirm("变更前确认")}
              >
                <span class="btn-prefix">✋</span>
                <span>变更前确认</span>
              </button>
              <button 
                class="pill-btn select-btn" 
                classList={{ active: selectedConfirm() === "自动执行" }}
                onClick={() => setSelectedConfirm("自动执行")}
              >
                <span class="btn-prefix">⚡</span>
                <span>自动执行</span>
              </button>
            </div>

            <div class="footer-right">
              {/* Model Selector Dropdown */}
              <div class="dropdown-wrapper" ref={modelRef}>
                <button class="pill-btn select-btn" onClick={() => setModelDropdownOpen(!modelDropdownOpen())}>
                  <span class="model-status-dot" />
                  <span>{selectedModel()}</span>
                  <span class="btn-caret">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                      <polyline points="6 9 12 15 18 9"></polyline>
                    </svg>
                  </span>
                </button>
                <Show when={modelDropdownOpen()}>
                  <div class="custom-dropdown-list">
                    <button class="dropdown-item" onClick={() => { setSelectedModel("GLM-5.2"); setModelDropdownOpen(false); }}>
                      GLM-5.2
                    </button>
                    <button class="dropdown-item" onClick={() => { setSelectedModel("GLM-4.0"); setModelDropdownOpen(false); }}>
                      GLM-4.0
                    </button>
                    <button class="dropdown-item" onClick={() => { setSelectedModel("GLM-4-Turbo"); setModelDropdownOpen(false); }}>
                      GLM-4-Turbo
                    </button>
                  </div>
                </Show>
              </div>

              {/* Priority Selector Dropdown */}
              <div class="dropdown-wrapper" ref={priorityRef}>
                <button class="pill-btn select-btn" onClick={() => setPriorityDropdownOpen(!priorityDropdownOpen())}>
                  <span class="btn-prefix">⚙️</span>
                  <span>{selectedPriority()}</span>
                  <span class="btn-caret">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                      <polyline points="6 9 12 15 18 9"></polyline>
                    </svg>
                  </span>
                </button>
                <Show when={priorityDropdownOpen()}>
                  <div class="custom-dropdown-list">
                    <button class="dropdown-item" onClick={() => { setSelectedPriority("最高"); setPriorityDropdownOpen(false); }}>
                      最高
                    </button>
                    <button class="dropdown-item" onClick={() => { setSelectedPriority("均衡"); setPriorityDropdownOpen(false); }}>
                      均衡
                    </button>
                    <button class="dropdown-item" onClick={() => { setSelectedPriority("最快"); setPriorityDropdownOpen(false); }}>
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
  );
}
