import { createSignal, onCleanup, onMount } from "solid-js";
import { getCurrentWebview } from "@tauri-apps/api/webview";

/**
 * Full-window drag-and-drop target. Listens to Tauri v2's native webview
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
    const unlisten = await getCurrentWebview().onDragDropEvent((event) => {
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
      void unlisten();
    });
  });

  return (
    <div class="dropzone-overlay" classList={{ active: dragging() }} aria-hidden="true">
      <div class="dropzone-hint">拖入文件夹或文件以注册为 SOURCE</div>
    </div>
  );
}
