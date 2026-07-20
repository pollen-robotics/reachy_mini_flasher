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
