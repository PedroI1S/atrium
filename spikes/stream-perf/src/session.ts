import { Pipeline } from "./pipeline";
import { type BridgeConfig, type StartInfo, type StartupInfo, bridgeStart } from "./platform";

export interface RunConfig extends BridgeConfig {
  /** Minimum gap between acknowledgements to the backend (ms). */
  ackIntervalMs: number;
}

export const DEFAULT_RUN: RunConfig = {
  threads: 1,
  tokensPerSecond: 10000,
  chunkChars: 4,
  durationMs: 60000,
  toolCallEveryMs: 4000,
  encoding: "bin",
  batchMs: 8,
  ackIntervalMs: 30,
};

export async function startSession(cfg: RunConfig, info: StartupInfo): Promise<{ pipeline: Pipeline; start: StartInfo }> {
  const pipeline = new Pipeline({ encoding: cfg.encoding, ackIntervalMs: cfg.ackIntervalMs });
  const start = await bridgeStart({ ...cfg, dir: info.dir, daemonExe: info.daemonExe }, pipeline.onMessage);
  pipeline.setThreads(start.threadIds);
  return { pipeline, start };
}
