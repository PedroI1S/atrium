// Thin wrapper over Tauri; everything degrades to a no-op in a plain browser (vite dev, tests).

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Encoding } from "./wire";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface StartupInfo {
  mainEpochMs: number;
  bench: string | null;
  dir: string | null;
  daemonExe: string | null;
}

export async function startupInfo(): Promise<StartupInfo> {
  if (!inTauri) return { mainEpochMs: 0, bench: null, dir: null, daemonExe: null };
  return invoke<StartupInfo>("startup_info");
}

export interface BridgeConfig {
  threads: number;
  tokensPerSecond: number;
  chunkChars: number;
  durationMs: number;
  toolCallEveryMs: number;
  encoding: Encoding;
  /** 0 = send every event on its own. */
  batchMs: number;
  maxInFlightBytes?: number;
  corpus?: boolean;
  dir?: string | null;
  daemonExe?: string | null;
}

export interface StartInfo {
  threadIds: string[];
  encoding: string;
  startEpochMs: number;
}

export async function bridgeStart(config: BridgeConfig, onMessage: (m: unknown) => void): Promise<StartInfo> {
  const channel = new Channel<unknown>();
  channel.onmessage = onMessage;
  return invoke<StartInfo>("bridge_start", { config, onBatch: channel });
}

export function ack(seq: number, cursors: [number, number][]): Promise<void> {
  if (!inTauri) return Promise.resolve();
  return invoke<void>("bridge_ack", { seq, cursors });
}

export function bridgeStats(): Promise<Record<string, unknown>> {
  return invoke<Record<string, unknown>>("bridge_stats");
}

export function bridgeStop(): Promise<void> {
  return inTauri ? invoke<void>("bridge_stop") : Promise.resolve();
}

/** A batch of `events` deltas of `chars` characters in the given encoding, as bytes (JSON as UTF-8). */
export function encodeSample(encoding: Encoding, events: number, chars: number): Promise<ArrayBuffer> {
  return invoke<ArrayBuffer>("encode_sample", { encoding, events, chars });
}

export async function writeReport(payload: unknown): Promise<void> {
  const json = JSON.stringify(payload);
  if (!inTauri) {
    console.log("ATRIUM_REPORT", json);
    return;
  }
  await invoke("write_report", { json });
}

export async function quitApp(): Promise<void> {
  if (inTauri) await invoke("quit");
}

export function writeProgress(payload: unknown): void {
  if (inTauri) void invoke("write_progress", { json: JSON.stringify(payload) });
}
