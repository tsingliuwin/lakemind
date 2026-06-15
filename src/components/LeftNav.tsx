import { For, Show, createMemo, createSignal, onMount, onCleanup } from "solid-js";
import type { SourceTable } from "../lib/types";
import { t, currentLanguage, setCurrentLanguage } from "../lib/i18n";
import { currentTheme, setCurrentTheme, currentZoom, setCurrentZoom } from "../lib/theme";

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
  sources: SourceTable[];
  selected: string | null;
  busy: boolean;
  onSelect: (table: SourceTable) => void;
  onOpenSettings: () => void;
  onNewQuery?: () => void;
  inspectorOpen?: boolean;
  consoleOpen?: boolean;
  onToggleInspector?: () => void;
  onToggleConsole?: () => void;
  onDisconnect?: () => void;
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
  let userMenuRef!: HTMLDivElement;

  const handleClickOutside = (e: MouseEvent) => {
    if (userMenuRef && !userMenuRef.contains(e.target as Node)) {
      setUserMenuOpen(false);
      setActiveSubmenu(null);
    }
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
      <div class="ln-top-bar">
        <div class="ln-logo-box" title="ZCode 3.0 / LakeMind">
          <svg class="ln-logo-z" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
            <path d="M4 4H20V7L8 17H20V20H4V17L16 7H4V4Z" fill="currentColor"/>
          </svg>
        </div>
        <div class="ln-nav-arrows">
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
        <button class="ln-action-btn" title="新建查询 (Ctrl+N)" onClick={() => props.onNewQuery?.()} disabled={props.busy}>
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M12 5v14M5 12h14"/>
            </svg>
          </span>
          <span class="action-label">{t("newQueryTask")}</span>
          <span class="action-shortcut">Ctrl+N</span>
        </button>
        <button class="ln-action-btn" title={`${t("search")} (Ctrl+K)`} disabled={props.busy}>
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <circle cx="11" cy="11" r="8"></circle>
              <line x1="21" y1="21" x2="16.65" y2="16.65"></line>
            </svg>
          </span>
          <span class="action-label">{t("search")}</span>
          <span class="action-shortcut">Ctrl+K</span>
        </button>
        <button class="ln-action-btn" title={t("skills")} disabled={props.busy}>
          <span class="action-icon">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <polygon points="12 2 2 7 12 12 22 7 12 2"></polygon>
              <polyline points="2 17 12 22 22 17"></polyline>
              <polyline points="2 12 12 17 22 12"></polyline>
            </svg>
          </span>
          <span class="action-label">{t("skills")}</span>
          <span class="action-shortcut"></span>
        </button>
      </div>

      {/* Workspace header */}
      <div class="ln-section-header">
        <span class="section-title">{t("workspace")} <span class="ws-indicator-dot" /></span>
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
      <Show
        when={props.sources.length > 0}
        fallback={
          <div class="empty-hint">
            <div class="empty-icon">📂</div>
            {t("dragHint")}
          </div>
        }
      >
        <div class="tree">
          <For each={groups()}>
            {(group) => (
              <div class="tree-group">
                <div class="tree-group-label" title={group[0]}>
                  📁 {shortDir(group[0])}
                </div>
                <For each={group[1]}>
                  {(t) => (
                    <button
                      class="tree-leaf"
                      classList={{ selected: props.selected === t.name }}
                      disabled={props.busy}
                      title={t.scanPath}
                      onClick={() => props.onSelect(t)}
                    >
                      <span class="kind-badge" data-kind={t.kind}>{t.kind}</span>
                      <span class="leaf-label">{t.label}</span>
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
        </div>
      </Show>

      <div class="ln-footer" ref={userMenuRef}>
        <button 
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
          <div class="ln-user-dropdown">
            
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
          <button
            class="ln-foot-icon-btn"
            classList={{ active: props.inspectorOpen }}
            title={props.inspectorOpen ? t("hideInspector") : t("showInspector")}
            onClick={() => props.onToggleInspector?.()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="15" y1="3" x2="15" y2="21"></line>
            </svg>
          </button>
          <button
            class="ln-foot-icon-btn"
            classList={{ active: props.consoleOpen }}
            title={props.consoleOpen ? t("hideConsole") : t("showConsole")}
            onClick={() => props.onToggleConsole?.()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="3" y1="15" x2="21" y2="15"></line>
            </svg>
          </button>
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
  const segs = path.split(/[\\/]/).filter(Boolean);
  return segs.slice(-1)[0] || path; // Show only the directory name for cleaner ZCode layout
}

function formatCount(n: number): string {
  if (n >= 1_000_000_000) return (n / 1_000_000_000).toFixed(1) + "B";
  if (n >= 1_000_000) return (n / 1_000_000).toFixed(1) + "M";
  if (n >= 1_000) return (n / 1_000).toFixed(0) + "K";
  return String(n);
}
