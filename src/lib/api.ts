import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface ReachyDevice {
  device: string;
  display_device: string;
  description: string;
  size: number;
  /** "ready" | "download" | "simulated" */
  mode: string;
}

export interface FlashProgress {
  /** "downloading" | "flashing" | "done" */
  phase: string;
  written: number;
  total: number;
  /** OS version being downloaded (only on "downloading" events). */
  version?: string;
}

export const FLASH_PROGRESS_EVENT = 'flash://progress';

// ---------------------------------------------------------------------------
// Self-update (Tauri bundle updater) - mirrors reachy_mini_tray's app_update.
// ---------------------------------------------------------------------------

export interface AppUpdateInfo {
  /** Version currently running. */
  current: string;
  /** Version offered by the update endpoint. */
  version: string;
  /** Release notes / changelog body, if any. */
  notes: string;
}

export interface AppUpdateProgress {
  downloaded: number;
  total: number | null;
  percent: number | null;
}

export const APP_UPDATE_AVAILABLE_EVENT = 'app-update:available';
export const APP_UPDATE_PROGRESS_EVENT = 'app-update:progress';
export const APP_UPDATE_ERROR_EVENT = 'app-update:error';

/** Pending update metadata cached by the backend (null if none / not checked). */
export function getAppUpdateInfo(): Promise<AppUpdateInfo | null> {
  return invoke<AppUpdateInfo | null>('get_app_update_info');
}

/** Download + verify + install the pending update, then relaunch the app. */
export function installAppUpdate(): Promise<void> {
  return invoke('install_app_update');
}

export function onAppUpdateAvailable(cb: (info: AppUpdateInfo) => void): Promise<UnlistenFn> {
  return listen<AppUpdateInfo>(APP_UPDATE_AVAILABLE_EVENT, (event) => cb(event.payload));
}

export function onAppUpdateProgress(cb: (p: AppUpdateProgress) => void): Promise<UnlistenFn> {
  return listen<AppUpdateProgress>(APP_UPDATE_PROGRESS_EVENT, (event) => cb(event.payload));
}

export function onAppUpdateError(cb: (message: string) => void): Promise<UnlistenFn> {
  return listen<string>(APP_UPDATE_ERROR_EVENT, (event) => cb(event.payload));
}

/** Returns the currently connected Reachy Mini, or null. */
export function detectReachy(): Promise<ReachyDevice | null> {
  return invoke<ReachyDevice | null>('detect_reachy');
}

/**
 * Run rpiboot to expose the CM4 eMMC when the Reachy is in download mode.
 * No-op when already `ready` or in simulation. Resolves once the eMMC is
 * (about to be) exposed; the next `detectReachy` poll then reports `ready`.
 */
export function prepareReachy(): Promise<void> {
  return invoke('prepare_reachy');
}

/** Download the OS image into the cache ahead of time. Streams "downloading" progress. */
export function prefetchImage(): Promise<void> {
  return invoke('prefetch_image');
}

/** Resolve the image and flash the detected Reachy. Streams progress events. */
export function flashReachy(): Promise<void> {
  return invoke('flash_reachy');
}

export function onFlashProgress(cb: (p: FlashProgress) => void): Promise<UnlistenFn> {
  return listen<FlashProgress>(FLASH_PROGRESS_EVENT, (event) => cb(event.payload));
}

/** Open an external URL in the system browser (falls back to window.open in dev). */
export async function openUrl(url: string): Promise<void> {
  try {
    await invoke('open_url', { url });
  } catch {
    window.open(url, '_blank', 'noopener,noreferrer');
  }
}
