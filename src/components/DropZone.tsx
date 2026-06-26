import { createSignal, onCleanup, onMount } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Full-window drag-and-drop target. Listens to Tauri v2's native window
 * drag/drop events (so dropping OS files works, unlike HTML5 DnD which only
 * sees sanitized payloads). On `drop`, hands each path to `onDropFiles`.
 *
 * While the OS is dragging over the window we paint an overlay highlight.
 */
export default function DropZone(props: {
  workspace: string;
  onDropFiles: (paths: string[]) => void;
  busy: boolean;
}) {
  const [dragging, setDragging] = createSignal(false);

  onMount(async () => {
    // Prevent default browser dragover/drop behaviors to allow dropping files in WebView2
    const preventDefault = (e: DragEvent) => {
      e.preventDefault();
    };
    window.addEventListener("dragover", preventDefault);
    window.addEventListener("drop", preventDefault);

    // Listen to native Tauri window drag/drop events
    const unlisten = await getCurrentWindow().onDragDropEvent((event) => {
      const payload = event.payload;
      if (payload.type === "enter" || payload.type === "over") {
        setDragging(true);
      } else if (payload.type === "leave") {
        setDragging(false);
      } else if (payload.type === "drop") {
        setDragging(false);
        if (props.busy) return;
        props.onDropFiles(payload.paths);
      }
    });

    onCleanup(() => {
      window.removeEventListener("dragover", preventDefault);
      window.removeEventListener("drop", preventDefault);
      void unlisten();
    });
  });

  return (
    <div class="dropzone-overlay" classList={{ active: dragging() }} aria-hidden="true">
      <div class="dropzone-hint">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style="width: 32px; height: 32px;">
          <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path>
          <polyline points="17 8 12 3 7 8"></polyline>
          <line x1="12" y1="3" x2="12" y2="15"></line>
        </svg>
        <span>拖入文件夹或文件以注册为数据源</span>
      </div>
    </div>
  );
}
