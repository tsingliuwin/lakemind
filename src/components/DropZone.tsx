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
      <div class="dropzone-hint">拖入文件夹或文件以注册为 SOURCE</div>
    </div>
  );
}
