/**
 * RalphRunner result helpers
 *
 * Pure functions for computing the terminal {@link RunnerState} from a
 * process exit and assembling a {@link RunnerResult} from a {@link LogStream}.
 *
 * Kept free of EventEmitter or subprocess concerns so they can be reused
 * and unit-tested in isolation.
 */

import type { LogStream } from "./LogStream";
import { RunnerState } from "./RunnerState";
import type { RunnerResult } from "./RalphRunnerTypes";

/**
 * Inputs describing a process exit.
 */
export interface ExitInfo {
  /** Exit code reported by the OS (null if killed by signal) */
  code: number | null;
  /** Signal that killed the process (null for normal exit) */
  signal: NodeJS.Signals | null;
}

/**
 * Compute the terminal runner state for a given process exit.
 *
 * - SIGTERM / SIGKILL → CANCELLED (we always send these via {@link RalphRunner.stop})
 * - exit code 0 → COMPLETED
 * - anything else → FAILED
 */
export function determineFinalState(exit: ExitInfo): RunnerState {
  if (exit.signal === "SIGTERM" || exit.signal === "SIGKILL") {
    return RunnerState.CANCELLED;
  }
  if (exit.code === 0) {
    return RunnerState.COMPLETED;
  }
  return RunnerState.FAILED;
}

/**
 * Build a {@link RunnerResult} from a process exit, the captured logs,
 * and a start timestamp.
 */
export function buildExitResult(
  finalState: RunnerState,
  exit: ExitInfo,
  logStream: LogStream,
  startedAt: Date | undefined
): RunnerResult {
  const durationMs = startedAt ? Date.now() - startedAt.getTime() : 0;
  const error =
    finalState === RunnerState.FAILED ? `Process exited with code ${exit.code}` : undefined;

  return {
    state: finalState,
    exitCode: exit.code ?? undefined,
    signal: exit.signal ?? undefined,
    stdout: logStream.getStdoutText(),
    stderr: logStream.getStderrText(),
    combined: logStream.getCombinedText(),
    durationMs,
    error,
  };
}

/**
 * Build a {@link RunnerResult} for an error path (spawn failure or other
 * synchronous/runtime error) where we do not have a process exit code.
 */
export function buildErrorResult(
  err: Error,
  logStream: LogStream,
  startedAt: Date | undefined
): RunnerResult {
  const durationMs = startedAt ? Date.now() - startedAt.getTime() : 0;

  return {
    state: RunnerState.FAILED,
    stdout: logStream.getStdoutText(),
    stderr: logStream.getStderrText(),
    combined: logStream.getCombinedText(),
    durationMs,
    error: err.message,
  };
}
