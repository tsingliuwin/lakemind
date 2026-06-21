import { createSignal, Show, onMount, onCleanup } from "solid-js";
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

export default function SettingsPage(props: {
  onClose: () => void;
  onOpenSettings?: () => void;
  titleBar?: any;
}) {
  const [activeTab, setActiveTab] = createSignal<SettingsTab>("modelSettings");
  const [connectionType, setConnectionType] = createSignal<string>("coding");
  
  // Custom dropdown selector inside BigModel
  const [showDropdown, setShowDropdown] = createSignal(false);

  // Left predefined providers
  const [selectedProvider, setSelectedProvider] = createSignal<string>("bigmodel");

  let connDropdownRef!: HTMLDivElement;
  let connTriggerRef!: HTMLButtonElement;

  const handleClickOutside = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    if (
      showDropdown() &&
      connDropdownRef &&
      !connDropdownRef.contains(target) &&
      (!connTriggerRef || !connTriggerRef.contains(target))
    ) {
      setShowDropdown(false);
    }
  };

  onMount(() => {
    document.addEventListener("mousedown", handleClickOutside);
    onCleanup(() => {
      document.removeEventListener("mousedown", handleClickOutside);
    });
  });

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
              <div class="sp-section-lbl">{t("predefined")}</div>
              <button 
                class="sp-provider-item" 
                classList={{ active: selectedProvider() === "bigmodel" }}
                onClick={() => setSelectedProvider("bigmodel")}
              >
                <span class="provider-dot active" />
                <span class="provider-icon-lbl">🔹</span>
                <span class="provider-name">BigModel</span>
              </button>

              <div class="sp-section-lbl" style="margin-top: 20px;">{t("customProviders")}</div>
              <button class="sp-add-btn" onClick={() => alert(t("settingsM1Placeholder"))}>
                <span class="add-icon">+</span>
                <span>{t("addProvider")}</span>
              </button>
            </div>

            {/* Right Column: Provider Details */}
            <div class="sp-right-panel">
              <Show 
                when={selectedProvider() === "bigmodel"}
                fallback={<div class="sp-empty-provider">{t("selectProviderPrompt")}</div>}
              >
                {/* BigModel Provider view */}
                <div class="provider-detail-header">
                  <div class="pd-title-group">
                    <span class="pd-icon">🔹</span>
                    <h3>BigModel</h3>
                    <span class="pd-status-badge">{t("enabledBadge")}</span>
                  </div>

                  <div class="pd-connection">
                    <span class="pd-conn-lbl">{t("connectionType")}</span>
                    <div class="pd-select-container">
                      <button ref={connTriggerRef} class="pd-select-trigger" onClick={() => setShowDropdown(!showDropdown())}>
                        {connectionType() === "coding" ? t("codingPackage") : t("defaultPackage")}
                        <span class="pd-select-chevron">▼</span>
                      </button>
                      <Show when={showDropdown()}>
                        <div class="pd-select-dropdown" ref={connDropdownRef}>
                          <button 
                            class="pd-dropdown-opt" 
                            onClick={() => { setConnectionType("coding"); setShowDropdown(false); }}
                          >
                            {t("codingPackage")}
                          </button>
                          <button 
                            class="pd-dropdown-opt" 
                            onClick={() => { setConnectionType("default"); setShowDropdown(false); }}
                          >
                            {t("defaultPackage")}
                          </button>
                        </div>
                      </Show>
                    </div>
                  </div>
                </div>

                {/* GLM Coding Pro card */}
                <div class="sp-card glm-card">
                  <div class="glm-card-left">
                    <div class="glm-card-title-row">
                      <span class="glm-card-title">GLM Coding Pro</span>
                      <span class="glm-card-badge">~ 150% 扣额</span>
                    </div>
                    <div class="glm-card-subtext">
                      {t("expirationInfo")}
                    </div>
                  </div>
                  <button class="glm-card-btn" onClick={() => alert(t("alreadyPremiumMsg"))}>
                    <span class="rocket-icon">🚀</span>
                    {t("upgradeBtn")}
                  </button>
                </div>

                {/* Quota Statistics cards group */}
                <div class="sp-stats-grid">
                  
                  {/* Hours Left */}
                  <div class="stat-card">
                    <div class="stat-header">
                      <span class="stat-val">99%</span>
                      <span class="stat-lbl">{t("hoursRemaining")}</span>
                    </div>
                    <div class="progress-container">
                      <div class="progress-bar bar-blue" style={{ width: "99%" }} />
                    </div>
                  </div>

                  {/* Quota Left */}
                  <div class="stat-card">
                    <div class="stat-header">
                      <span class="stat-val">34%</span>
                      <span class="stat-lbl">{t("dailyRemaining")}</span>
                    </div>
                    <div class="progress-container">
                      <div class="progress-bar bar-green" style={{ width: "34%" }} />
                    </div>
                  </div>

                  {/* MCP Left */}
                  <div class="stat-card">
                    <div class="stat-header">
                      <span class="stat-val">98%</span>
                      <span class="stat-lbl">{t("mcpDailyRemaining")}</span>
                    </div>
                    <div class="progress-container">
                      <div class="progress-bar bar-purple" style={{ width: "98%" }} />
                    </div>
                  </div>
                </div>

                {/* Models List */}
                <div class="sp-models-section">
                  <div class="models-section-title">{t("modelListTitle")}</div>
                  <div class="models-list">
                    <div class="model-row">
                      <span class="model-name-lbl">GLM-4.0</span>
                      <span class="model-token-badge">100万</span>
                    </div>
                    <div class="model-row">
                      <span class="model-name-lbl">GLM-4-Turbo</span>
                      <span class="model-token-badge">20万</span>
                    </div>
                  </div>
                </div>
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
    </div>
  );
}
