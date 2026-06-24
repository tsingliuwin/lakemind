import { Show } from "solid-js";

/**
 * Top bar (40px): brand on the left, collapse toggles on the right. The
 * toggles control the right Inspector drawer and the bottom Console drawer —
 * the only two collapsible regions in the M1 grid.
 *
 * No tabs, no command palette, no window controls — those are M2+.
 */
export default function TopBar(props: {
  inspectorOpen: boolean;
  consoleOpen: boolean;
  onToggleInspector: () => void;
  onToggleConsole: () => void;
}) {
  return (
    <header class="topbar">
      <span class="brand">LakeMind</span>
      <span class="brand-sub">M1 · 纯算力客户端</span>
      <span class="spacer" />
      <div class="toggle-group">
        <Show
          when={props.inspectorOpen}
          fallback={
            <button
              class="icon-btn"
              title="显示 Schema 检查器"
              onClick={() => props.onToggleInspector()}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                <line x1="15" y1="3" x2="15" y2="21"></line>
              </svg>
            </button>
          }
        >
          <button
            class="icon-btn active"
            title="隐藏 Schema 检查器"
            onClick={() => props.onToggleInspector()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="15" y1="3" x2="15" y2="21"></line>
            </svg>
          </button>
        </Show>
        <Show
          when={props.consoleOpen}
          fallback={
            <button
              class="icon-btn"
              title="显示执行日志"
              onClick={() => props.onToggleConsole()}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
                <line x1="3" y1="15" x2="21" y2="15"></line>
              </svg>
            </button>
          }
        >
          <button
            class="icon-btn active"
            title="隐藏执行日志"
            onClick={() => props.onToggleConsole()}
          >
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 14px; height: 14px;">
              <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
              <line x1="3" y1="15" x2="21" y2="15"></line>
            </svg>
          </button>
        </Show>
      </div>
    </header>
  );
}
