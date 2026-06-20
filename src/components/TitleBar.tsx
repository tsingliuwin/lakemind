import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { SourceTable } from "../lib/types";
import { t } from "../lib/i18n";
import { logoSrc } from "../lib/theme";

export default function TitleBar(props: {
  inspectorOpen: boolean;
  consoleOpen: boolean;
  onToggleInspector: () => void;
  onToggleConsole: () => void;
  onNewQuery: () => void;
  selectedTable: SourceTable | null;
  onOpenSettings: () => void;
}) {
  const [menuOpen, setMenuOpen] = createSignal(false);
  const [aboutOpen, setAboutOpen] = createSignal(false);
  const appWindow = getCurrentWindow();

  let menuRef!: HTMLDivElement;

  // Click outside to close menu
  const handleClickOutside = (e: MouseEvent) => {
    if (menuRef && !menuRef.contains(e.target as Node)) {
      setMenuOpen(false);
    }
  };

  onMount(() => {
    document.addEventListener("mousedown", handleClickOutside);
    onCleanup(() => {
      document.removeEventListener("mousedown", handleClickOutside);
    });
  });

  const handleOpenExplorer = async () => {
    setMenuOpen(false);
    if (props.selectedTable && props.selectedTable.path) {
      try {
        // Use Tauri's opener plugin to open the directory path
        const { openPath } = await import("@tauri-apps/plugin-opener");
        await openPath(props.selectedTable.path);
      } catch (err) {
        console.error("Failed to open explorer", err);
      }
    } else {
      alert(t("openExplorerSelectFirst"));
    }
  };

  return (
    <div class="titlebar" data-tauri-drag-region>
      {/* Titlebar Left: Logo, Name, and ZCode-style Dropdown Menu */}
      {/* Titlebar Left: Logo and Name */}
      <div class="titlebar-left" data-tauri-drag-region>
        <span class="tb-logo" data-tauri-drag-region><img src={logoSrc()} alt="LakeMind" style="width: 14px; height: 14px; object-fit: contain; vertical-align: middle;" /></span>
        <span class="tb-brand" data-tauri-drag-region>LakeMind</span>
      </div>

      {/* Titlebar Middle: Drag Region showing Active Source */}
      <div class="titlebar-middle" data-tauri-drag-region>
        <Show when={props.selectedTable}>
          {(tVal) => (
            <span class="tb-workspace-info" data-tauri-drag-region>
              {t("currentSource")}: {tVal().label} ({tVal().kind})
            </span>
          )}
        </Show>
      </div>

      {/* Titlebar Right: Menu Trigger (with ZCode style chevron icon) + Windows Native Actions */}
      <div class="titlebar-right" ref={menuRef}>
        <button 
          class="tb-win-btn tb-menu-trigger-btn" 
          classList={{ active: menuOpen() }}
          title={t("menu")}
          onClick={() => setMenuOpen(!menuOpen())}
        >
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" style="width: 12px; height: 12px;">
            <polyline points="6 9 12 15 18 9"></polyline>
          </svg>
        </button>

        {/* Custom ZCode Dropdown Menu */}
        <Show when={menuOpen()}>
          <div class="tb-dropdown-menu right-aligned">
            <button 
              class="menu-item" 
              onClick={() => { setMenuOpen(false); props.onNewQuery(); }}
            >
              <span class="menu-label">{t("newQueryTask")}</span>
              <span class="menu-shortcut">Ctrl+N</span>
            </button>
            
            <button 
              class="menu-item" 
              onClick={handleOpenExplorer}
              disabled={!props.selectedTable}
            >
              <span class="menu-label">{t("openInExplorer")}</span>
              <span class="menu-shortcut"></span>
            </button>

            <div class="menu-divider" />

            <button 
              class="menu-item" 
              onClick={() => { setMenuOpen(false); props.onToggleInspector(); }}
            >
              <span class="menu-label">{props.inspectorOpen ? t("hideInspector") : t("showInspector")}</span>
              <span class="menu-shortcut"></span>
            </button>

            <button 
              class="menu-item" 
              onClick={() => { setMenuOpen(false); props.onToggleConsole(); }}
            >
              <span class="menu-label">{props.consoleOpen ? t("hideConsole") : t("showConsole")}</span>
              <span class="menu-shortcut"></span>
            </button>

            <button 
              class="menu-item" 
              onClick={() => { setMenuOpen(false); props.onOpenSettings(); }}
            >
              <span class="menu-label">{t("modelSettingsCenter")}</span>
              <span class="menu-shortcut"></span>
            </button>

            <div class="menu-divider" />

            <button 
              class="menu-item" 
              onClick={() => { setMenuOpen(false); setAboutOpen(true); }}
            >
              <span class="menu-label">{t("aboutApp")}</span>
              <span class="menu-shortcut"></span>
            </button>
            
            <button 
              class="menu-item" 
              onClick={() => { setMenuOpen(false); alert(t("latestVersionMsg")); }}
            >
              <span class="menu-label">{t("checkUpdates")}</span>
              <span class="menu-shortcut"></span>
            </button>

            <div class="menu-divider" />

            <button 
              class="menu-item close-item" 
              onClick={() => { setMenuOpen(false); void appWindow.close(); }}
            >
              <span class="menu-label">{t("closeWindow")}</span>
              <span class="menu-shortcut"></span>
            </button>
          </div>
        </Show>

        <button 
          class="tb-win-btn" 
          title={t("minimize")}
          onClick={() => void appWindow.minimize()}
        >
          <svg viewBox="0 0 10.2 1" style="width: 10px; height: 1px;">
            <rect x="0" y="0" width="10.2" height="1" fill="currentColor" />
          </svg>
        </button>
        <button 
          class="tb-win-btn" 
          title={t("maximize")}
          onClick={() => void appWindow.toggleMaximize()}
        >
          <svg viewBox="0 0 10 10" style="width: 10px; height: 10px;">
            <path d="M0,0v10h10V0H0z M9,9H1V1h8V9z" fill="currentColor" />
          </svg>
        </button>
        <button 
          class="tb-win-btn close-btn" 
          title={t("close")}
          onClick={() => void appWindow.close()}
        >
          <svg viewBox="0 0 10 10" style="width: 10px; height: 10px;">
            <polygon points="10,0.7 9.3,0 5,4.3 0.7,0 0,0.7 4.3,5 0,9.3 0.7,10 5,5.7 9.3,10 10,9.3 5.7,5" fill="currentColor" />
          </svg>
        </button>
      </div>

      {/* About Modal Dialog */}
      <Show when={aboutOpen()}>
        <div class="modal-overlay" onClick={() => setAboutOpen(false)}>
          <div class="modal-card" onClick={(e) => e.stopPropagation()}>
            <div class="modal-header">
              <h3>{t("aboutTitle")}</h3>
              <button class="modal-close" onClick={() => setAboutOpen(false)}>✕</button>
            </div>
            <div class="modal-body">
              <div class="about-logo"><img src={logoSrc()} alt="LakeMind" style="width: 48px; height: 48px; object-fit: contain;" /></div>
              <h4>{t("aboutCore")}</h4>
              <p class="about-desc">{t("aboutDesc")}</p>
              <div class="about-specs">
                <div class="spec-row"><span>{t("aboutVersion")}</span><strong>v0.1.0 (M1 Stable)</strong></div>
                <div class="spec-row"><span>{t("aboutKernel")}</span><strong>DuckDB v1.10.5</strong></div>
                <div class="spec-row"><span>{t("aboutEnv")}</span><strong>Tauri Webview Backend</strong></div>
                <div class="spec-row"><span>{t("aboutArch")}</span><strong>ZCode 3.0 Grid System</strong></div>
              </div>
            </div>
            <div class="modal-footer">
              <button class="modal-btn-primary" onClick={() => setAboutOpen(false)}>{t("okBtn")}</button>
            </div>
          </div>
        </div>
      </Show>
    </div>
  );
}
