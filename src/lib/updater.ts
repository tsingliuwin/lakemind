/**
 * Auto-update helpers built on top of @tauri-apps/plugin-updater.
 *
 * Replaces the legacy "fetch update.json + openUrl" flow with real in-app
 * download/install. Falls back to opening the download page when the updater
 * plugin is unavailable (e.g. running outside Tauri / in the browser).
 */
import { check, type DownloadEvent } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { createSignal, createRoot } from "solid-js";

/** True when the Tauri webview internals exist (i.e. running inside the app). */
const inTauri = typeof window !== "undefined" && (window as any).__TAURI_INTERNALS__;

/** Legacy manifest kept on the site for v0.3.0 clients that lack the plugin. */
const LEGACY_MANIFEST_URL = "https://lakemind.xi-n.com/update.json";
/** Fallback download page when the updater cannot run. */
const DOWNLOAD_PAGE_URL = "https://lakemind.xi-n.com/";

export interface UpdateInfo {
  /** New version string, without a leading "v" (from the updater). */
  version: string;
  /** Release notes / changelog body. */
  notes: string;
}

export type ProgressState = "idle" | "checking" | "downloading" | "installing" | "done" | "error";

export interface DownloadProgress {
  /** Fraction downloaded so far in [0, 1]. Stays 0 if total size is unknown. */
  fraction: number;
  /** Human-readable downloaded / total, e.g. "3.2 / 20 MB". Empty until Started. */
  human: string;
}

/**
 * Compare semver-ish versions (supports an optional leading "v").
 * Returns true when `latest` is strictly newer than `current`.
 */
export function isNewerVersion(current: string, latest: string): boolean {
  const curParts = current.replace(/^v/, "").split(".").map((x) => parseInt(x, 10) || 0);
  const latParts = latest.replace(/^v/, "").split(".").map((x) => parseInt(x, 10) || 0);
  for (let i = 0; i < 3; i++) {
    const c = curParts[i] ?? 0;
    const l = latParts[i] ?? 0;
    if (l > c) return true;
    if (c > l) return false;
  }
  return false;
}

function fmtBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Check for an available update.
 *
 * - Inside Tauri: uses the official updater plugin (verifies signature, etc.).
 * - Outside Tauri: falls back to the legacy JSON manifest (best-effort), and
 *   resolves to `null` (no in-place install possible).
 *
 * Resolves to `{ version, notes }` when a newer version exists, or `null`.
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  if (!inTauri) {
    return legacyCheck();
  }
  try {
    const update = await check();
    if (!update) return null;
    return { version: update.version, notes: update.body ?? "" };
  } catch (e) {
    console.warn("Updater plugin check failed, falling back to legacy manifest:", e);
    return legacyCheck();
  }
}

/** Legacy fetch of update.json — used as a fallback or outside Tauri. */
async function legacyCheck(): Promise<UpdateInfo | null> {
  try {
    const res = await fetch(LEGACY_MANIFEST_URL);
    if (!res.ok) return null;
    const data = await res.json();
    const current = inTauri ? await getVersion().catch(() => "0.0.0") : "0.0.0";
    if (data.version && isNewerVersion(current, data.version)) {
      return { version: data.version.replace(/^v/, ""), notes: data.changelog || "" };
    }
    return null;
  } catch {
    return null;
  }
}

/**
 * Perform the full update flow: check → download (with progress) → install.
 * Does NOT relaunch; the caller decides when to restart via {@link relaunchApp}.
 *
 * `onProgress` is called during download with a normalized fraction + label.
 * Throws on any failure (caller should offer the openUrl fallback).
 */
export async function downloadAndInstallUpdate(
  onProgress?: (p: DownloadProgress) => void,
): Promise<void> {
  const update = await check();
  if (!update) throw new Error("No update available");

  let total = 0;
  let downloaded = 0;

  await update.downloadAndInstall((ev: DownloadEvent) => {
    if (ev.event === "Started" && ev.data.contentLength) {
      total = ev.data.contentLength;
    } else if (ev.event === "Progress") {
      downloaded += ev.data.chunkLength;
      const fraction = total > 0 ? Math.min(downloaded / total, 1) : 0;
      const human = total > 0 ? `${fmtBytes(downloaded)} / ${fmtBytes(total)}` : fmtBytes(downloaded);
      onProgress?.({ fraction, human });
    }
    // 'Finished' → the promise resolves; no callback needed.
  });

  await update.close();
}

/** Restart the app to apply an installed update. */
export async function relaunchApp(): Promise<void> {
  await relaunch();
}

/** Open the download page in the browser (fallback path). */
export async function openDownloadPage(): Promise<void> {
  await openUrl(DOWNLOAD_PAGE_URL);
}

/* ------------------------------------------------------------------ *
 * Global update store
 *
 * A single source of truth for update state, shared by the TitleBar
 * menu entry and the LeftNav badge. Drives:
 *   - a background poll (30s after start, then every 4h)
 *   - a silent download when a new version is found
 *   - a modal that both entry points can open
 * ------------------------------------------------------------------ */

import { t } from "./i18n";

/** Coarse state of the update state machine. */
export type UpdateStatus =
  | "idle" // nothing happened yet / reset
  | "checking" // a check() is in flight
  | "available" // new version known, not yet downloaded
  | "downloading" // silent download in progress
  | "ready" // downloaded & staged; waiting for user to relaunch
  | "installing" // relaunch in progress
  | "error";

export interface UpdateStateInfo {
  /** New version string (no leading "v"). Empty until known. */
  version: string;
  /** Release notes / changelog. */
  notes: string;
}

const POLL_INITIAL_DELAY_MS = 30_000; // first check 30s after start
const POLL_INTERVAL_MS = 4 * 60 * 60 * 1000; // then every 4 hours

const store = createRoot(() => {
  const [status, setStatus] = createSignal<UpdateStatus>("idle");
  const [info, setInfo] = createSignal<UpdateStateInfo>({ version: "", notes: "" });
  const [progress, setProgress] = createSignal<DownloadProgress>({ fraction: 0, human: "" });
  const [error, setError] = createSignal("");
  /** Whether the update modal is open (driven by either entry point). */
  const [modalOpen, setModalOpen] = createSignal(false);

  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let started = false; // idempotent guard: start() may be called from multiple TitleBars

  const resetTransient = () => {
    setError("");
    setProgress({ fraction: 0, human: "" });
  };

  /** Schedule the next background poll. */
  const schedulePoll = (delay: number) => {
    if (pollTimer) clearTimeout(pollTimer);
    pollTimer = setTimeout(() => {
      void runCheck(false);
      schedulePoll(POLL_INTERVAL_MS);
    }, delay);
  };

  /**
   * Run an update check.
   * - `userInitiated=true`: opens the modal and shows a spinner; on "no update"
   *   shows the "already latest" alert. This is the right-menu path.
   * - `userInitiated=false`: silent background poll; on a new version it kicks
   *   off a silent download. Never disturbs the user unless ready.
   *
   * Concurrency: if a background download is already in progress or the update
   * is already staged, a user-initiated check just opens the modal on the
   * current state instead of re-checking (avoids a double check()/download()).
   */
  const runCheck = async (userInitiated: boolean) => {
    if (!inTauri) return;
    const prev = status();
    if (userInitiated && (prev === "downloading" || prev === "ready" || prev === "checking")) {
      // Already knows about / is fetching the update — just surface it.
      setModalOpen(true);
      return;
    }
    if (userInitiated) setModalOpen(true);
    setStatus("checking");
    resetTransient();
    try {
      const found = await checkForUpdate();
      if (!found) {
        if (userInitiated) {
          setModalOpen(false);
          const cur = await getVersion().catch(() => "");
          alert(t("latestVersionMsg") + (cur ? ` (v${cur})` : ""));
        }
        setStatus("idle");
        return;
      }
      setInfo({ version: found.version, notes: found.notes });
      setStatus("available");
      // Background path: silently start downloading right away.
      if (!userInitiated) {
        void runDownload(false);
      }
    } catch (e) {
      console.error("Update check failed:", e);
      if (userInitiated) {
        setModalOpen(false);
        alert(t("updateCheckingFailed"));
      }
      setStatus("error");
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  /**
   * Download (and stage) the update without relaunching.
   * `userInitiated=true` keeps the modal open with a progress bar.
   * `userInitiated=false` runs truly silently; on success it flips to "ready".
   * Reentrant-safe: a download already in progress is a no-op.
   */
  const runDownload = async (userInitiated: boolean) => {
    if (status() === "downloading") return; // already fetching
    if (status() === "ready") return; // already staged
    setStatus("downloading");
    resetTransient();
    try {
      await downloadAndInstallUpdate((p) => setProgress(p));
      setStatus("ready");
      // Silent success: do NOT auto-open the modal; the badge reflects "ready".
    } catch (e) {
      console.error("Silent/interactive download failed:", e);
      setStatus("error");
      setError(e instanceof Error ? e.message : String(e));
      // Background failures are swallowed (next poll retries). Interactive ones
      // surface in the already-open modal.
      if (!userInitiated) {
        setStatus("idle"); // back off silently; badge hides
      }
    }
  };

  /** Apply the staged update by relaunching. */
  const runInstall = async () => {
    setStatus("installing");
    try {
      await relaunch();
    } catch (e) {
      console.error("Relaunch failed:", e);
      setStatus("error");
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  /** User clicked the menu "check for updates" — interactive flow. */
  const checkInteractively = () => void runCheck(true);

  /** User clicked "download" in the modal. */
  const downloadInteractively = () => void runDownload(true);

  /** User clicked "install & relaunch" in the modal. */
  const installAndRelaunch = () => void runInstall();

  /** Either entry point opens the modal. */
  const openModal = () => setModalOpen(true);
  const closeModal = () => setModalOpen(false);

  /** Open the browser download page and close the modal (fallback). */
  const fallbackDownload = () => {
    void openDownloadPage();
    setModalOpen(false);
  };

  /** Boot the background poller. Idempotent: safe to call from multiple mounts. */
  const start = () => {
    if (!inTauri || started) return;
    started = true;
    schedulePoll(POLL_INITIAL_DELAY_MS);
  };

  return {
    status,
    info,
    progress,
    error,
    modalOpen,
    start,
    checkInteractively,
    downloadInteractively,
    installAndRelaunch,
    openModal,
    closeModal,
    fallbackDownload,
  };
});

export const updater = store;
