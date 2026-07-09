import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import type { SourceTable } from "../lib/types";
import { t } from "../lib/i18n";
import { logoSrc } from "../lib/theme";
import {
  checkForUpdate,
  downloadAndInstallUpdate,
  relaunchApp,
  openDownloadPage,
  type DownloadProgress,
} from "../lib/updater";

const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Mac");

export default function TitleBar(props: {
  inspectorOpen: boolean;
  consoleOpen: boolean;
  onToggleInspector: () => void;
  onToggleConsole: () => void;
  selectedTable: SourceTable | null;
  busy?: boolean;
  leftOpen: boolean;
  onToggleLeft: () => void;
  hideLayoutToggles?: boolean;
}) {
  const [menuOpen, setMenuOpen] = createSignal(false);
  const [aboutOpen, setAboutOpen] = createSignal(false);
  const [appVersion, setAppVersion] = createSignal("v0.3.0");
  const appWindow = typeof window !== "undefined" && (window as any).__TAURI_INTERNALS__ ? getCurrentWindow() : null;

  // --- Auto-updater UI state ---
  type UpdateStage = "checking" | "available" | "downloading" | "downloaded" | "error";
  const [updateStage, setUpdateStage] = createSignal<UpdateStage | null>(null);
  const [updateVersion, setUpdateVersion] = createSignal("");
  const [updateNotes, setUpdateNotes] = createSignal("");
  const [updateProgress, setUpdateProgress] = createSignal<DownloadProgress>({ fraction: 0, human: "" });
  const [updateErrorMsg, setUpdateErrorMsg] = createSignal("");

  const closeUpdateModal = () => {
    setUpdateStage(null);
    setUpdateVersion("");
    setUpdateNotes("");
    setUpdateProgress({ fraction: 0, human: "" });
    setUpdateErrorMsg("");
  };

  const handleCheckUpdates = async () => {
    setUpdateStage("checking");
    try {
      const info = await checkForUpdate();
      if (!info) {
        alert(t("latestVersionMsg") + ` (${appVersion()})`);
        closeUpdateModal();
        return;
      }
      setUpdateVersion(info.version);
      setUpdateNotes(info.notes);
      setUpdateStage("available");
    } catch (e) {
      console.error("Check updates error:", e);
      alert(t("updateCheckingFailed"));
      closeUpdateModal();
    }
  };

  // Download → install (without relaunch); UI then offers the relaunch button.
  const handleDownloadInstall = async () => {
    setUpdateStage("downloading");
    try {
      await downloadAndInstallUpdate((p) => setUpdateProgress(p));
      setUpdateStage("downloaded");
    } catch (e) {
      console.error("Download/install error:", e);
      setUpdateErrorMsg(e instanceof Error ? e.message : String(e));
      setUpdateStage("error");
    }
  };

  const handleRelaunch = async () => {
    setUpdateStage("downloading"); // reuse as a "busy" state for the button
    try {
      await relaunchApp();
    } catch (e) {
      console.error("Relaunch error:", e);
      setUpdateStage("error");
      setUpdateErrorMsg(e instanceof Error ? e.message : String(e));
    }
  };

  // Fallback: open the download page in the browser.
  const handleFallbackDownload = () => {
    openDownloadPage().catch(console.error);
    closeUpdateModal();
  };

  let menuRef!: HTMLDivElement;

  // Click outside to close menu
  const handleClickOutside = (e: MouseEvent) => {
    if (menuRef && !menuRef.contains(e.target as Node)) {
      setMenuOpen(false);
    }
  };

  onMount(() => {
    document.addEventListener("mousedown", handleClickOutside);
    if (typeof window !== "undefined" && (window as any).__TAURI_INTERNALS__) {
      getVersion().then((v) => setAppVersion(`v${v}`)).catch(console.error);
    }
    onCleanup(() => {
      document.removeEventListener("mousedown", handleClickOutside);
    });
  });

  return (
    <div class="titlebar" data-tauri-drag-region>
      {/* Titlebar Left: Logo, Name, and ZCode-style Dropdown Menu */}
      {/* Titlebar Left: Logo and Name */}
      <div class="titlebar-left" classList={{ "mac-padding": isMac && !props.leftOpen }} data-tauri-drag-region>
        <Show when={!props.leftOpen}>
          <Show when={!isMac} fallback={
            <div class="ln-nav-arrows" style="display: flex; align-items: center; gap: 6px;" data-tauri-drag-region>
              {/* Sidebar toggle button (macOS) */}
              <button 
                class="ln-arrow-btn" 
                classList={{ active: props.leftOpen }}
                title={props.leftOpen ? "隐藏侧边栏" : "显示侧边栏"} 
                onClick={() => props.onToggleLeft()}
              >
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                  <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                  <line x1="9" y1="3" x2="9" y2="21"></line>
                </svg>
              </button>
              
            </div>
          }>
            <span class="tb-logo" data-tauri-drag-region><img src={logoSrc()} alt="LakeMind" style="width: 14px; height: 14px; object-fit: contain; vertical-align: middle;" /></span>
            <span class="tb-brand" data-tauri-drag-region>LakeMind</span>
            
            {/* Sidebar toggle button (Windows/Linux) */}
            <button 
              class="ln-arrow-btn" 
              style="margin-left: 8px;"
              classList={{ active: props.leftOpen }}
              title={props.leftOpen ? "隐藏侧边栏" : "显示侧边栏"} 
              onClick={() => props.onToggleLeft()}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                <line x1="9" y1="3" x2="9" y2="21"></line>
              </svg>
            </button>
          </Show>
        </Show>
      </div>

      {/* Titlebar Middle: Drag Region showing Active Source */}
      <div class="titlebar-middle" data-tauri-drag-region>
        <Show when={props.selectedTable}>
          {(tVal) => (
            <span class="tb-workspace-info" data-tauri-drag-region>
              {t("currentSource")}: {tVal().name} ({tVal().kind})
            </span>
          )}
        </Show>
      </div>

      {/* Titlebar Right: Menu Trigger (with ZCode style chevron icon) + Windows Native Actions */}
      <div class="titlebar-right" ref={menuRef} style="display: flex; align-items: center; gap: 4px; padding-right: 6px;">
        {/* Toggle Bottom Console Button */}
        <Show when={!props.hideLayoutToggles}>
          <button 
            class="ln-arrow-btn"
            classList={{ active: props.consoleOpen }}
            title={props.consoleOpen ? t("hideConsole") : t("showConsole")}
            onClick={() => props.onToggleConsole()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="3" y1="15" x2="21" y2="15"></line>
            </svg>
          </button>
        </Show>

        {/* Toggle Right Sidebar Button */}
        <Show when={!props.hideLayoutToggles}>
          <button 
            class="ln-arrow-btn"
            classList={{ active: props.inspectorOpen }}
            title={props.inspectorOpen ? t("hideInspector") : t("showInspector")}
            onClick={() => props.onToggleInspector()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="15" y1="3" x2="15" y2="21"></line>
            </svg>
          </button>
        </Show>

        <div class="tb-menu-wrap">
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
              onClick={() => { setMenuOpen(false); setAboutOpen(true); }}
            >
              <span class="menu-label">{t("aboutApp")}</span>
              <span class="menu-shortcut"></span>
            </button>

            <button
              class="menu-item"
              onClick={() => { setMenuOpen(false); void handleCheckUpdates(); }}
            >
              <span class="menu-label">{t("checkUpdates")}</span>
              <span class="menu-shortcut"></span>
            </button>

            <div class="menu-divider" />

            <button
              class="menu-item close-item"
              onClick={() => { setMenuOpen(false); void appWindow?.close(); }}
            >
              <span class="menu-label">{t("closeWindow")}</span>
              <span class="menu-shortcut"></span>
            </button>
          </div>
        </Show>
        </div>

        <Show when={!isMac}>
          <button 
            class="tb-win-btn" 
            title={t("minimize")}
            onClick={() => void appWindow?.minimize()}
          >
            <svg viewBox="0 0 10.2 1" style="width: 10px; height: 1px;">
              <rect x="0" y="0" width="10.2" height="1" fill="currentColor" />
            </svg>
          </button>
          <button 
            class="tb-win-btn" 
            title={t("maximize")}
            onClick={() => void appWindow?.toggleMaximize()}
          >
            <svg viewBox="0 0 10 10" style="width: 10px; height: 10px;">
              <path d="M0,0v10h10V0H0z M9,9H1V1h8V9z" fill="currentColor" />
            </svg>
          </button>
          <button 
            class="tb-win-btn close-btn" 
            title={t("close")}
            onClick={() => void appWindow?.close()}
          >
            <svg viewBox="0 0 10 10" style="width: 10px; height: 10px;">
              <polygon points="10,0.7 9.3,0 5,4.3 0.7,0 0,0.7 4.3,5 0,9.3 0.7,10 5,5.7 9.3,10 10,9.3 5.7,5" fill="currentColor" />
            </svg>
          </button>
        </Show>
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
                <div class="spec-row"><span>{t("aboutVersion")}</span><strong>{appVersion()}</strong></div>
                <div class="spec-row"><span>{t("aboutKernel")}</span><strong>DuckDB v1.5.4</strong></div>
                <div class="spec-row"><span>{t("aboutEnv")}</span><strong>Tauri Webview Backend</strong></div>
                <div class="spec-row"><span>{t("aboutArch")}</span><strong>SolidJS Grid Layout</strong></div>
              </div>
            </div>
          </div>
        </div>
      </Show>

      {/* Update Modal */}
      <Show when={updateStage()}>
        <div class="modal-overlay" onClick={() => updateStage() !== "downloading" && closeUpdateModal()}>
          <div class="modal-card" style="width: 420px;" onClick={(e) => e.stopPropagation()}>
            <div class="modal-header">
              <h3>
                {updateStage() === "checking" && t("updateChecking")}
                {updateStage() === "available" && `${t("updateAvailable")} v${updateVersion()}`}
                {updateStage() === "downloading" && t("downloadingUpdate")}
                {updateStage() === "downloaded" && t("updateDownloaded")}
                {updateStage() === "error" && t("updateFailed")}
              </h3>
              <Show when={updateStage() !== "downloading"}>
                <button class="modal-close" onClick={closeUpdateModal}>✕</button>
              </Show>
            </div>
            <div class="modal-body" style="align-items: stretch; text-align: left;">
              <Show when={updateStage() === "checking"}>
                <div style="display: flex; align-items: center; gap: 10px; padding: 12px 0;">
                  <span class="import-banner__spinner" />
                  <span style="color: var(--text-secondary); font-size: 13px;">{t("updateChecking")}</span>
                </div>
              </Show>

              <Show when={updateStage() === "available"}>
                <div class="update-notes">
                  <div class="update-notes-label">{t("updateNotes")} (v{updateVersion()})</div>
                  <pre class="update-notes-body">{updateNotes()}</pre>
                </div>
              </Show>

              <Show when={updateStage() === "downloading"}>
                <div class="update-progress-wrap">
                  <div class="update-progress-bar">
                    <div
                      class="update-progress-fill"
                      style={`width: ${Math.round(updateProgress().fraction * 100)}%`}
                    />
                  </div>
                  <div class="update-progress-meta">
                    {Math.round(updateProgress().fraction * 100)}% {updateProgress().human && `· ${updateProgress().human}`}
                  </div>
                </div>
              </Show>

              <Show when={updateStage() === "downloaded"}>
                <p style="margin: 8px 0; color: var(--text-secondary); font-size: 13px; line-height: 1.6;">
                  {t("updateDownloaded")}
                </p>
              </Show>

              <Show when={updateStage() === "error"}>
                <p style="margin: 8px 0; color: var(--text-danger, #e06c75); font-size: 13px; line-height: 1.6;">
                  {updateErrorMsg()}
                </p>
              </Show>
            </div>
            <Show when={updateStage() === "available" || updateStage() === "downloaded" || updateStage() === "error"}>
              <div class="modal-footer">
                <Show when={updateStage() === "error"}>
                  <button class="modal-btn-secondary" onClick={handleFallbackDownload}>{t("goDownload")}</button>
                </Show>
                <Show when={updateStage() === "available"}>
                  <button class="modal-btn-secondary" onClick={closeUpdateModal}>{t("close")}</button>
                  <button class="modal-btn-primary" onClick={() => void handleDownloadInstall()}>{t("downloadingUpdate")}</button>
                </Show>
                <Show when={updateStage() === "downloaded"}>
                  <button class="modal-btn-primary" onClick={() => void handleRelaunch()}>{t("installAndRelaunch")}</button>
                </Show>
              </div>
            </Show>
          </div>
        </div>
      </Show>
    </div>
  );
}
