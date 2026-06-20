import { createSignal, Show, onMount, onCleanup } from "solid-js";
import { t, currentLanguage, setCurrentLanguage } from "../lib/i18n";
import { currentTheme, setCurrentTheme, currentZoom, setCurrentZoom, logoSrc } from "../lib/theme";

type SettingsTab =
  | "general"
  | "codeIntel"
  | "modelSettings"
  | "skills"
  | "mcp"
  | "plugins"
  | "sync"
  | "stats";

export default function SettingsPage(props: {
  onClose: () => void;
  onOpenSettings?: () => void;
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
    <div class="settings-layout" style={{ "grid-column": "1 / -1", "grid-row": "2 / -1", "z-index": 90 }}>
      {/* Settings Sidebar */}
      <aside class="settings-sidebar">
        <div class="ss-logo-area">
          <div class="ss-logo-box">
            <img src={logoSrc()} alt="LakeMind" style="width: 18px; height: 18px; object-fit: contain;" />
          </div>
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
            <span class="ss-nav-icon">⚙️</span>
            <span>{t("settingsGeneral")}</span>
          </button>
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "codeIntel" }}
            onClick={() => setActiveTab("codeIntel")}
          >
            <span class="ss-nav-icon">💻</span>
            <span>{t("settingsCodeIntel")}</span>
          </button>
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "modelSettings" }}
            onClick={() => setActiveTab("modelSettings")}
          >
            <span class="ss-nav-icon">🧠</span>
            <span>{t("modelSettingsCenter")}</span>
          </button>
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "skills" }}
            onClick={() => setActiveTab("skills")}
          >
            <span class="ss-nav-icon">🚀</span>
            <span>{t("settingsSkills")}</span>
          </button>
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "mcp" }}
            onClick={() => setActiveTab("mcp")}
          >
            <span class="ss-nav-icon">🔌</span>
            <span>{t("settingsMcp")}</span>
          </button>
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "plugins" }}
            onClick={() => setActiveTab("plugins")}
          >
            <span class="ss-nav-icon">🧩</span>
            <span>{t("settingsPlugins")}</span>
          </button>
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "sync" }}
            onClick={() => setActiveTab("sync")}
          >
            <span class="ss-nav-icon">☁️</span>
            <span>{t("settingsSync")}</span>
          </button>
          <button 
            class="ss-nav-item" 
            classList={{ active: activeTab() === "stats" }}
            onClick={() => setActiveTab("stats")}
          >
            <span class="ss-nav-icon">📊</span>
            <span>{t("settingsStats")}</span>
          </button>
        </nav>

        <div class="ss-footer">
          <div class="ss-user-badge">
            <span class="ss-avatar">研</span>
            <span class="ss-username">研途教育</span>
          </div>
          <button class="ss-gear-btn active" title="Settings">
            <span class="gear-icon">⚙️</span>
          </button>
        </div>
      </aside>

      {/* Settings Main Content Area */}
      <main class="settings-content">
        
        {/* Tab 1: General Settings (常规) */}
        <Show when={activeTab() === "general"}>
          <div class="settings-view-header">
            <h2>{t("settingsGeneral")}</h2>
            <p class="settings-view-subtitle">{t("generalSettingsDesc")}</p>
          </div>
          <div class="settings-panel-box single-col">
            <div class="settings-group">
              <label class="settings-row-control">
                <span class="label-title">界面语言 / Language</span>
                <select 
                  value={currentLanguage()} 
                  onChange={(e) => setCurrentLanguage(e.currentTarget.value as any)}
                >
                  <option value="zh">简体中文</option>
                  <option value="en">English</option>
                </select>
              </label>

              <label class="settings-row-control">
                <span class="label-title">界面主题 / Theme</span>
                <select 
                  value={currentTheme()} 
                  onChange={(e) => setCurrentTheme(e.currentTarget.value as any)}
                >
                  <option value="geek-dark">极客暗黑 (ZCode 3.0)</option>
                  <option value="classic-dark">经典深色</option>
                  <option value="light">极致浅色</option>
                </select>
              </label>

              <label class="settings-row-control">
                <span class="label-title">界面缩放 / Zoom</span>
                <select 
                  value={currentZoom()} 
                  onChange={(e) => setCurrentZoom(Number(e.currentTarget.value))}
                >
                  <option value={80}>80%</option>
                  <option value={90}>90%</option>
                  <option value={100}>100%</option>
                  <option value={110}>110%</option>
                  <option value={120}>120%</option>
                </select>
              </label>
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
  );
}
