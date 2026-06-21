import { createSignal, Show, onMount, For } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { t, currentLanguage, setCurrentLanguage } from "../lib/i18n";
import { currentTheme, setCurrentTheme, currentZoom, setCurrentZoom, logoSrc } from "../lib/theme";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

type SettingsTab =
  | "general"
  | "codePreview"
  | "modelSettings"
  | "skills"
  | "mcp"
  | "plugins"
  | "commands"
  | "indexDb"
  | "stats"
  | "guide";

export interface ModelItem {
  id: string;
  contextWindow: number;
  maxTokens?: number;
}

export interface ModelProvider {
  id: string;
  name: string;
  endpoint: string;
  apiKey: string;
  apiFormat: "openai" | "anthropic" | "responses";
  models: ModelItem[];
  enabled: boolean;
  isPredefined?: boolean;
}

export interface AppSettings {
  theme?: string;
  language?: string;
  zoom?: string;
  providers?: ModelProvider[];
  [key: string]: any;
}

export default function SettingsPage(props: {
  onClose: () => void;
  onOpenSettings?: () => void;
  titleBar?: any;
}) {
  const [activeTab, setActiveTab] = createSignal<SettingsTab>("modelSettings");

  // Selection signals
  const [selectedProvider, setSelectedProvider] = createSignal<string>("");
  const [isAddingProvider, setIsAddingProvider] = createSignal(false);

  const [settings, setSettings] = createSignal<AppSettings>({
    providers: []
  });

  const [showApiKey, setShowApiKey] = createSignal(false);

  // Rename fields inline
  const [editingProviderId, setEditingProviderId] = createSignal<string | null>(null);
  const [tempName, setTempName] = createSignal("");



  // New provider temp states
  const [newProviderName, setNewProviderName] = createSignal("");
  const [newProviderEndpoint, setNewProviderEndpoint] = createSignal("");
  const [newProviderApiKey, setNewProviderApiKey] = createSignal("");
  const [newProviderFormat, setNewProviderFormat] = createSignal<"openai" | "anthropic" | "responses">("openai");
  const [newProviderModels, setNewProviderModels] = createSignal<ModelItem[]>([]);

  // Dialog popups for adding/editing models
  const [isModelModalOpen, setIsModelModalOpen] = createSignal(false);
  const [isAddingToTempProvider, setIsAddingToTempProvider] = createSignal(false);
  const [modalMode, setModalMode] = createSignal<"add" | "edit">("add");
  const [editingModelId, setEditingModelId] = createSignal<string>("");
  const [modelFormId, setModelFormId] = createSignal("");
  const [modelFormWindow, setModelFormWindow] = createSignal(200000);
  const [modelFormMaxTokens, setModelFormMaxTokens] = createSignal(4096);

  onMount(async () => {
    try {
      const json = await invoke<string>("load_settings_json");
      if (json && json !== "{}") {
        const loaded = JSON.parse(json);
        setSettings(loaded);
        
        if (loaded.providers && loaded.providers.length > 0) {
          setSelectedProvider(loaded.providers[0].id);
        }
      }
    } catch (err) {
      console.error("Failed to load settings:", err);
    }
  });

  // Save settings helper
  const updateSetting = (key: keyof AppSettings, value: any) => {
    const updated = { ...settings(), [key]: value };
    setSettings(updated);
    invoke("save_settings_json", { json: JSON.stringify(updated, null, 2) }).catch(err => {
      console.error("Failed to save settings:", err);
    });
  };

  const updateProviderProperty = (providerId: string, property: keyof ModelProvider, value: any) => {
    const updatedProviders = (settings().providers || []).map(p => {
      if (p.id === providerId) {
        return { ...p, [property]: value };
      }
      return p;
    });
    updateSetting("providers", updatedProviders);
  };

  const handleSaveProviderName = () => {
    const val = tempName().trim();
    if (val && editingProviderId()) {
      updateProviderProperty(editingProviderId()!, "name", val);
    }
    setEditingProviderId(null);
  };

  const handleDeleteProvider = (id: string) => {
    const updated = (settings().providers || []).filter(p => p.id !== id);
    updateSetting("providers", updated);
    if (updated.length > 0) {
      setSelectedProvider(updated[0].id);
    } else {
      setSelectedProvider("");
    }
  };

  const handleCreateNewProvider = () => {
    const name = newProviderName().trim();
    const endpoint = newProviderEndpoint().trim();
    const apiKey = newProviderApiKey().trim();
    const format = newProviderFormat();

    if (!name) {
      alert("请输入服务商名称");
      return;
    }
    if (!endpoint) {
      alert("请输入 Base URL");
      return;
    }

    const newId = "custom_" + Date.now();
    const newProvider: ModelProvider = {
      id: newId,
      name,
      endpoint,
      apiKey,
      apiFormat: format,
      models: newProviderModels(),
      enabled: true
    };

    const updated = [...(settings().providers || []), newProvider];
    updateSetting("providers", updated);
    setSelectedProvider(newId);
    setIsAddingProvider(false);

    // Reset temp states
    setNewProviderName("");
    setNewProviderEndpoint("");
    setNewProviderApiKey("");
    setNewProviderFormat("openai");
    setNewProviderModels([]);
  };

  // Model actions handlers
  const handleOpenAddModel = () => {
    setIsAddingToTempProvider(false);
    setModalMode("add");
    setModelFormId("");
    setModelFormWindow(200000);
    setIsModelModalOpen(true);
  };

  const handleOpenAddTempModel = () => {
    setIsAddingToTempProvider(true);
    setModalMode("add");
    setModelFormId("");
    setModelFormWindow(200000);
    setModelFormMaxTokens(4096);
    setIsModelModalOpen(true);
  };

  const handleOpenEditModel = (model: ModelItem) => {
    setIsAddingToTempProvider(false);
    setModalMode("edit");
    setEditingModelId(model.id);
    setModelFormId(model.id);
    setModelFormWindow(model.contextWindow);
    setModelFormMaxTokens(model.maxTokens || 4096);
    setIsModelModalOpen(true);
  };

  const handleOpenEditTempModel = (model: ModelItem) => {
    setIsAddingToTempProvider(true);
    setModalMode("edit");
    setEditingModelId(model.id);
    setModelFormId(model.id);
    setModelFormWindow(model.contextWindow);
    setModelFormMaxTokens(model.maxTokens || 4096);
    setIsModelModalOpen(true);
  };

  const handleDeleteModel = (modelId: string) => {
    const currentProv = (settings().providers || []).find(p => p.id === selectedProvider());
    if (!currentProv) return;
    const updatedModels = currentProv.models.filter(m => m.id !== modelId);
    updateProviderProperty(selectedProvider(), "models", updatedModels);
  };

  const handleSaveModel = () => {
    const mId = modelFormId().trim();
    if (!mId) return;

    if (isAddingToTempProvider()) {
      if (modalMode() === "add") {
        if (newProviderModels().some(m => m.id === mId)) {
          alert("模型已存在");
          return;
        }
        setNewProviderModels([...newProviderModels(), { id: mId, contextWindow: modelFormWindow(), maxTokens: modelFormMaxTokens() }]);
      } else {
        setNewProviderModels(newProviderModels().map(m => {
          if (m.id === editingModelId()) {
            return { id: mId, contextWindow: modelFormWindow(), maxTokens: modelFormMaxTokens() };
          }
          return m;
        }));
      }
      setIsModelModalOpen(false);
      return;
    }

    const currentProv = (settings().providers || []).find(p => p.id === selectedProvider());
    if (!currentProv) return;

    let updatedModels: ModelItem[] = [];
    if (modalMode() === "add") {
      if (currentProv.models.some(m => m.id === mId)) {
        alert("模型已存在");
        return;
      }
      updatedModels = [...currentProv.models, { id: mId, contextWindow: modelFormWindow(), maxTokens: modelFormMaxTokens() }];
    } else {
      updatedModels = currentProv.models.map(m => {
        if (m.id === editingModelId()) {
          return { id: mId, contextWindow: modelFormWindow(), maxTokens: modelFormMaxTokens() };
        }
        return m;
      });
    }

    updateProviderProperty(selectedProvider(), "models", updatedModels);
    setIsModelModalOpen(false);
  };



  return (
    <div class="settings-layout-wrapper">
      {/* Settings Sidebar */}
      <aside class="settings-sidebar">
        <div class="ss-logo-area" classList={{ "mac-nav": isMac }}>
          <Show when={!isMac}>
            <div class="ss-logo-box">
              <img src={logoSrc()} alt="LakeMind" style="width: 18px; height: 18px; object-fit: contain;" />
            </div>
          </Show>
        </div>

        <button class="ss-back-btn" onClick={() => props.onClose()}>
          <span class="back-icon">←</span>
          <span class="back-label">{t("backToWorkspace")}</span>
        </button>

        <nav class="ss-nav">
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "general" }}
            onClick={() => setActiveTab("general")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="4" cy="5" r="1.5" />
                <path d="M5.5 5h8M2 5h0.5" />
                <circle cx="12" cy="11" r="1.5" />
                <path d="M2 11h8.5M13.5 11h0.5" />
              </svg>
            </span>
            <span>{t("settingsGeneral")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "codePreview" }}
            onClick={() => setActiveTab("codePreview")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M5 11L2 8l3-3M11 5l3 3-3 3M9.5 4l-3 8" />
              </svg>
            </span>
            <span>{t("settingsCodePreview")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "modelSettings" }}
            onClick={() => setActiveTab("modelSettings")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <rect x="2" y="3" width="12" height="4" rx="1" />
                <rect x="2" y="9" width="12" height="4" rx="1" />
                <circle cx="4.5" cy="5" r="0.5" fill="currentColor" />
                <circle cx="4.5" cy="11" r="0.5" fill="currentColor" />
              </svg>
            </span>
            <span>{t("modelSettings")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "skills" }}
            onClick={() => setActiveTab("skills")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 13l7.5-7.5" />
                <path d="M12.5 1.5l.5 1.5.5-1.5zM14 3.5l-1.5.5 1.5.5zM10.5 3l.5-.5-.5-.5z" />
                <circle cx="10.5" cy="5.5" r="0.75" fill="currentColor" />
                <circle cx="7.5" cy="2.5" r="0.75" fill="currentColor" />
              </svg>
            </span>
            <span>{t("settingsSkills")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "mcp" }}
            onClick={() => setActiveTab("mcp")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M9 3v4a3 3 0 0 1-6 0V3M4 1v2M8 1v2M6 10v4" />
              </svg>
            </span>
            <span>{t("settingsMcp")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "plugins" }}
            onClick={() => setActiveTab("plugins")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M10 2.5c0-.83-.67-1.5-1.5-1.5S7 1.67 7 2.5v1.2H4.5A1.3 1.3 0 0 0 3.2 5v2.5h1.2c.83 0 1.5.67 1.5 1.5S5.23 10.5 4.4 10.5H3.2V13a1.3 1.3 0 0 0 1.3 1.3H7v-1.2c0-.83.67-1.5 1.5-1.5s1.5.67 1.5 1.5v1.2h2.5a1.3 1.3 0 0 0 1.3-1.3V10.5h-1.2c-.83 0-1.5-.67-1.5-1.5s.67-1.5 1.5-1.5h1.2V5A1.3 1.3 0 0 0 12.5 3.7H10V2.5z" />
              </svg>
            </span>
            <span>{t("settingsPlugins")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "commands" }}
            onClick={() => setActiveTab("commands")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 4l4 4-4 4M9 12h4" />
              </svg>
            </span>
            <span>{t("settingsCommands")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "indexDb" }}
            onClick={() => setActiveTab("indexDb")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M8 2s4.5 1.5 4.5 4.5v3.5c0 2.5-2.5 4.5-4.5 5-2-.5-4.5-2.5-4.5-5V6.5C3.5 3.5 8 2 8 2z" />
                <path d="M6 8.5l1.5 1.5 3-3" />
              </svg>
            </span>
            <span>{t("settingsIndexDb")}</span>
          </button>

          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "stats" }}
            onClick={() => setActiveTab("stats")}
          >
            <span class="ss-nav-icon">
              <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                <path d="M2.5 13.5h11M4.5 13.5v-3M8 13.5v-6M11.5 13.5V3.5" />
              </svg>
            </span>
            <span>{t("settingsStats")}</span>
          </button>

          <div class="ss-guide-container">
            <button 
              class="ss-nav-item" 
              classList={{ active: activeTab() === "guide" }}
              onClick={() => setActiveTab("guide")}
            >
              <span class="ss-nav-icon">
                <svg class="ss-nav-svg" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M12.5 3.5c-1.5-1.5-5 0-6 1a25 25 0 0 0-2.5 3.5L2 9.5l2.5 2L6 14l1.5-2c1.5-.8 2.8-1.8 3.8-3.3 1-1 2.5-4.5 1.2-5.7zM4.5 11.5L2 14M9.5 6.5l.5.5" />
                </svg>
              </span>
              <span>{t("settingsGuide")}</span>
            </button>
          </div>
        </nav>

        <div class="ss-footer">
          <div class="ss-user-badge" onClick={() => alert("研途教育")}>
            <span class="ss-avatar">研</span>
            <span class="ss-username">研途教育</span>
          </div>
          <button class="ln-foot-icon-btn active" title="Settings" onClick={() => props.onClose()}>
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="12" cy="12" r="3"></circle>
              <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
            </svg>
          </button>
        </div>
      </aside>

      {/* Settings Right Area */}
      <div class="settings-right-container">
        {props.titleBar}
        {/* Settings Main Content Area */}
        <main class="settings-content">
        
        {/* Tab 1: General Settings (常规) */}
        <Show when={activeTab() === "general"}>
          <div class="settings-view-header">
            <h2>{t("settingsGeneral")}</h2>
            <p class="settings-view-subtitle">{t("generalSettingsDesc")}</p>
          </div>
          
          <div class="settings-section-card">
            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">界面主题</span>
                <p class="settings-row-desc">切换应用界面使用的主题外观。</p>
              </div>
              <div class="select-wrapper">
                <select 
                  value={currentTheme()} 
                  onChange={(e) => setCurrentTheme(e.currentTarget.value as any)}
                >
                  <option value="geek-dark">🌙 极客暗黑</option>
                  <option value="classic-dark">🌙 经典深色</option>
                  <option value="light">☀️ 极致浅色</option>
                </select>
              </div>
            </div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">界面语言</span>
                <p class="settings-row-desc">选择应用 UI 的显示语言。</p>
              </div>
              <div class="select-wrapper">
                <select 
                  value={currentLanguage()} 
                  onChange={(e) => setCurrentLanguage(e.currentTarget.value as any)}
                >
                  <option value="zh">简体中文</option>
                  <option value="en">English</option>
                </select>
              </div>
            </div>

            <div class="settings-row-control">
              <div class="settings-row-info">
                <span class="label-title">界面缩放</span>
                <p class="settings-row-desc">调整当前窗口中文本和控件的整体显示大小。</p>
              </div>
              <div class="segmented-control">
                <button 
                  classList={{ active: currentZoom() === 90 }} 
                  onClick={() => setCurrentZoom(90)}
                >
                  偏小
                </button>
                <button 
                  classList={{ active: currentZoom() === 100 }} 
                  onClick={() => setCurrentZoom(100)}
                >
                  正常
                </button>
                <button 
                  classList={{ active: currentZoom() === 110 }} 
                  onClick={() => setCurrentZoom(110)}
                >
                  偏大
                </button>
              </div>
            </div>
          </div>
        </Show>

        {/* Tab 2: Model Settings (模型设置) - Premium High-Fidelity Details */}
        <Show when={activeTab() === "modelSettings"}>
          <div class="settings-view-header">
            <h2>{t("modelSettings")}</h2>
            <p class="settings-view-subtitle">{t("settingsSubtitle")}</p>
          </div>

          <div class="settings-panel-box two-cols">
            
            {/* Left Column: Providers List */}
            <div class="sp-left-panel">
              <div class="sp-section-lbl">模型供应商</div>
              
              <For each={settings().providers}>
                {(prov) => (
                  <button 
                    class="sp-provider-item" 
                    classList={{ active: selectedProvider() === prov.id && !isAddingProvider() }}
                    onClick={() => { setSelectedProvider(prov.id); setIsAddingProvider(false); }}
                  >
                    <span class="provider-dot" classList={{ active: prov.enabled && !!prov.apiKey }} />
                    <span class="provider-icon-lbl">
                      {prov.id === "bigmodel" ? "🔹" : prov.id === "deepseek" ? "🐳" : prov.id === "siliconflow" ? "⚡" : prov.id === "openai" ? "🤖" : "⚙️"}
                    </span>
                    <span class="provider-name">{prov.name}</span>
                  </button>
                )}
              </For>

              <button class="sp-add-btn" onClick={() => { setIsAddingProvider(true); setSelectedProvider(""); }}>
                <span class="add-icon">+</span>
                <span>添加供应商</span>
              </button>
            </div>

            {/* Right Column: Provider Details */}
            <div class="sp-right-panel">
              <Show
                when={!isAddingProvider()}
                fallback={
                  <>
                    <div class="provider-detail-header">
                      <div class="pd-title-group">
                        <span class="pd-icon">⚙️</span>
                        <h3>添加模型供应商</h3>
                      </div>
                    </div>
                    <p class="settings-view-desc" style="margin-top: -10px; margin-bottom: 20px; color: var(--text-dim); font-size: 12px;">
                      配置一个完全自定义的 API 端点和初始模型。
                    </p>

                    <div class="sp-form-section" style="margin-top: 0;">
                      <div class="sp-form-row">
                        <span class="sp-form-label">名称</span>
                        <input 
                          type="text" 
                          class="sp-input" 
                          placeholder="如：智谱 GLM"
                          value={newProviderName()}
                          onInput={(e) => setNewProviderName(e.currentTarget.value)}
                        />
                      </div>

                      <div class="sp-form-row">
                        <span class="sp-form-label">Base URL</span>
                        <input 
                          type="text" 
                          class="sp-input" 
                          placeholder="https://api.example.com/v1"
                          value={newProviderEndpoint()}
                          onInput={(e) => setNewProviderEndpoint(e.currentTarget.value)}
                        />
                      </div>

                      <div class="sp-form-row">
                        <span class="sp-form-label">API Key</span>
                        <div class="sp-input-wrapper">
                          <input 
                            type={showApiKey() ? "text" : "password"} 
                            class="sp-input password-input" 
                            placeholder="输入 API Key"
                            value={newProviderApiKey()}
                            onInput={(e) => setNewProviderApiKey(e.currentTarget.value)}
                          />
                          <button class="sp-pwd-toggle" onClick={() => setShowApiKey(!showApiKey())}>
                            {showApiKey() ? "👁️" : "🙈"}
                          </button>
                        </div>
                      </div>

                      <div class="sp-form-row">
                        <span class="sp-form-label">API 格式</span>
                        <div class="select-wrapper" style="width: 100%;">
                          <select 
                            value={newProviderFormat()}
                            onChange={(e) => setNewProviderFormat(e.currentTarget.value as any)}
                          >
                            <option value="anthropic">Anthropic Messages (/v1/messages)</option>
                            <option value="openai">Chat Completions (/chat/completions)</option>
                            <option value="responses">Responses (/responses)</option>
                          </select>
                        </div>
                      </div>

                      <div class="sp-form-row" style="margin-top: 10px;">
                        <span class="sp-form-label">模型列表</span>
                        <div class="models-list">
                          <For each={newProviderModels()}>
                            {(model) => (
                              <div class="model-row">
                                <span class="model-name-lbl">{model.id}</span>
                                <div style="display: flex; align-items: center; gap: 12px;">
                                  <span class="sp-context-badge" title="上下文窗口">
                                    {model.contextWindow >= 10000 ? `${model.contextWindow / 10000}万` : model.contextWindow}
                                  </span>
                                  <span class="sp-context-badge" title="最大输出 Token">
                                    Out: {model.maxTokens || 4096}
                                  </span>
                                  <div class="model-actions-btns" style="display: flex; align-items: center; gap: 8px;">
                                    <button class="sp-action-icon-btn" title="编辑模型" onClick={() => handleOpenEditTempModel(model)}>✏️</button>
                                    <button class="sp-action-icon-btn" title="删除模型" onClick={() => setNewProviderModels(newProviderModels().filter(m => m.id !== model.id))}>🗑️</button>
                                  </div>
                                </div>
                              </div>
                            )}
                          </For>
                          <button class="sp-add-model-inline-btn" onClick={handleOpenAddTempModel}>
                            + 添加模型
                          </button>
                        </div>
                      </div>
                    </div>

                    <div style="margin-top: 20px; display: flex; gap: 12px;">
                      <button class="sp-btn-primary" onClick={handleCreateNewProvider}>
                        添加供应商
                      </button>
                      <button class="sp-btn-secondary" onClick={() => setIsAddingProvider(false)}>
                        取消
                      </button>
                    </div>
                  </>
                }
              >
                {(() => {
                  const prov = (settings().providers || []).find(p => p.id === selectedProvider());
                  if (!prov) {
                    return (
                      <div class="sp-empty-provider" style="display: flex; flex-direction: column; align-items: center; justify-content: center; height: 100%; color: var(--text-dim); text-align: center; padding: 40px;">
                        <span style="font-size: 40px; margin-bottom: 12px;">🔌</span>
                        <h4 style="color: var(--text); font-weight: 500;">暂无模型供应商</h4>
                        <p style="font-size: 12px; margin-top: 6px; max-width: 280px; line-height: 1.5;">点击左侧「添加供应商」按钮配置您的 AI API 接口和模型</p>
                      </div>
                    );
                  }

                  return (
                    <>
                      <div class="provider-detail-header" style="border-bottom: none; padding-bottom: 0; margin-bottom: 12px;">
                        <div class="pd-title-group">
                          <span class="pd-icon">
                            {prov.id === "bigmodel" ? "🔹" : prov.id === "deepseek" ? "🐳" : prov.id === "siliconflow" ? "⚡" : prov.id === "openai" ? "🤖" : "⚙️"}
                          </span>
                          <Show
                            when={editingProviderId() === prov.id}
                            fallback={
                              <div style="display: flex; align-items: center; gap: 6px;">
                                <h3>{prov.name}</h3>
                                <button class="sp-edit-title-btn" onClick={() => {
                                  setEditingProviderId(prov.id);
                                  setTempName(prov.name);
                                }}>✏️</button>
                              </div>
                            }
                          >
                            <div style="display: flex; align-items: center; gap: 6px;">
                              <input 
                                type="text" 
                                class="sp-input" 
                                style="width: 150px; height: 26px; padding: 2px 8px;"
                                value={tempName()}
                                onInput={(e) => setTempName(e.currentTarget.value)}
                                onBlur={handleSaveProviderName}
                                onKeyDown={(e) => {
                                  if (e.key === "Enter") handleSaveProviderName();
                                  if (e.key === "Escape") setEditingProviderId(null);
                                }}
                                autoFocus
                              />
                            </div>
                          </Show>

                          <div class="sp-status-btn-group">
                            <button 
                              class={`status-btn enabled-btn ${prov.enabled ? "active" : ""}`}
                              onClick={() => updateProviderProperty(prov.id, "enabled", true)}
                            >
                              已启用
                            </button>
                            <button 
                              class={`status-btn disabled-btn ${!prov.enabled ? "active" : ""}`}
                              onClick={() => updateProviderProperty(prov.id, "enabled", false)}
                            >
                              禁用
                            </button>
                          </div>
                        </div>

                        <button class="sp-btn-danger" style="margin-top: 0;" onClick={() => handleDeleteProvider(prov.id)}>
                          删除
                        </button>
                      </div>

                      <div class="sp-form-section" style="margin-top: 0;">
                        <div class="sp-form-row">
                          <span class="sp-form-label">Base URL</span>
                          <input 
                            type="text" 
                            class="sp-input" 
                            value={prov.endpoint}
                            onInput={(e) => updateProviderProperty(prov.id, "endpoint", e.currentTarget.value)}
                          />
                        </div>

                        <div class="sp-form-row">
                          <span class="sp-form-label">API 格式</span>
                          <div class="select-wrapper" style="width: 100%;">
                            <select 
                              value={prov.apiFormat}
                              onChange={(e) => updateProviderProperty(prov.id, "apiFormat", e.currentTarget.value as any)}
                            >
                              <option value="anthropic">Anthropic Messages (/v1/messages)</option>
                              <option value="openai">Chat Completions (/chat/completions)</option>
                              <option value="responses">Responses (/responses)</option>
                            </select>
                          </div>
                        </div>

                        <div class="sp-form-row">
                          <span class="sp-form-label">API Key</span>
                          <div class="sp-input-wrapper">
                            <input 
                              type={showApiKey() ? "text" : "password"} 
                              class="sp-input password-input" 
                              placeholder={t("apiKeyPlaceholder")}
                              value={prov.apiKey}
                              onInput={(e) => updateProviderProperty(prov.id, "apiKey", e.currentTarget.value)}
                            />
                            <button class="sp-pwd-toggle" onClick={() => setShowApiKey(!showApiKey())}>
                              {showApiKey() ? "👁️" : "🙈"}
                            </button>
                          </div>
                        </div>

                        <div class="sp-form-row" style="margin-top: 10px;">
                          <span class="sp-form-label">模型列表</span>
                          <div class="models-list">
                            <For each={prov.models}>
                              {(model) => (
                                <div class="model-row">
                                  <span class="model-name-lbl">{model.id}</span>
                                  <div style="display: flex; align-items: center; gap: 12px;">
                                    <span class="sp-context-badge" title="上下文窗口">
                                      {model.contextWindow >= 10000 ? `${model.contextWindow / 10000}万` : model.contextWindow}
                                    </span>
                                    <span class="sp-context-badge" title="最大输出 Token">
                                      Out: {model.maxTokens || 4096}
                                    </span>
                                    <div class="model-actions-btns" style="display: flex; align-items: center; gap: 8px;">

                                      <button class="sp-action-icon-btn" title="编辑模型" onClick={() => handleOpenEditModel(model)}>✏️</button>
                                      <button class="sp-action-icon-btn" title="删除模型" onClick={() => handleDeleteModel(model.id)}>🗑️</button>
                                    </div>
                                  </div>
                                </div>
                              )}
                            </For>
                            <button class="sp-add-model-inline-btn" onClick={handleOpenAddModel}>
                              + 添加模型
                            </button>
                          </div>
                        </div>
                      </div>
                    </>
                  );
                })()}
              </Show>
            </div>
          </div>
        </Show>

        {/* Placeholders for other tabs */}
        <Show when={activeTab() !== "general" && activeTab() !== "modelSettings"}>
          <div class="settings-view-header">
            <h2>{t("settings")}</h2>
            <p class="settings-view-subtitle">{t("moduleDeveloping")}...</p>
          </div>
          <div class="settings-panel-box single-col">
            <div class="sp-empty-state">
              <span class="empty-icon">🚧</span>
              <h4>{t("moduleUnderConstruction")}</h4>
              <p>{t("m1WorkingMsg")}</p>
            </div>
          </div>
        </Show>
        </main>
      </div>

      {/* Modal Dialog for Add/Edit Model */}
      <Show when={isModelModalOpen()}>
        <div class="sp-modal-overlay" onClick={() => setIsModelModalOpen(false)}>
          <div class="sp-modal-box" onClick={(e) => e.stopPropagation()}>
            <div class="sp-modal-header">
              <h3>{modalMode() === "add" ? "添加模型" : "编辑模型"}</h3>
              <button class="sp-modal-close" onClick={() => setIsModelModalOpen(false)}>×</button>
            </div>
            
            <div class="sp-modal-body">
              <div class="sp-form-row" style="margin-bottom: 16px;">
                <span class="sp-form-label">模型 ID</span>
                <input 
                  type="text" 
                  class="sp-input" 
                  placeholder="模型 ID"
                  value={modelFormId()}
                  onInput={(e) => setModelFormId(e.currentTarget.value)}
                />
              </div>
              
              <div class="sp-form-row" style="margin-bottom: 16px;">
                <span class="sp-form-label">上下文窗口</span>
                <input 
                  type="number" 
                  class="sp-input" 
                  placeholder="200000"
                  value={modelFormWindow()}
                  onInput={(e) => setModelFormWindow(parseInt(e.currentTarget.value) || 0)}
                />
              </div>

              <div class="sp-form-row">
                <span class="sp-form-label">最大输出 Token</span>
                <input 
                  type="number" 
                  class="sp-input" 
                  placeholder="4096"
                  value={modelFormMaxTokens()}
                  onInput={(e) => setModelFormMaxTokens(parseInt(e.currentTarget.value) || 0)}
                />
              </div>
            </div>
            
            <div class="sp-modal-footer">
              <button class="sp-modal-btn cancel-btn" onClick={() => setIsModelModalOpen(false)}>取消</button>
              <button class="sp-modal-btn save-btn" onClick={handleSaveModel}>保存</button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}
