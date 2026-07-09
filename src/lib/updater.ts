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
