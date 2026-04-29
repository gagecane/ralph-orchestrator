/**
 * RalphRunner public types
 *
 * Interfaces describing the configuration, result, and events for
 * {@link RalphRunner}. Extracted into their own module so that consumers
 * (and collaborator modules inside `runner/`) can import type definitions
 * without pulling in the full runner implementation.
 */

import type { LogEntry, LogCallback } from "./LogStream";
import type { ProcessSupervisor } from "./ProcessSupervisor";
import type { FileOutputStreamer } from "./FileOutputStreamer";
import type { RunnerState } from "./RunnerState";

/**
 * Configuration options for RalphRunner
 */
export interface RalphRunnerOptions {
  /** Command to execute (default: 'ralph') */
  command?: string;
  /** Base arguments (default: ['run']) */
  baseArgs?: string[];
  /** Working directory for the subprocess */
  cwd?: string;
  /** Environment variables (merged with process.env) */
  env?: Record<string, string>;
  /** Graceful stop timeout in ms before SIGKILL (default: 5000) */
  gracefulTimeoutMs?: number;
  /** Maximum output buffer size (default: 10MB) */
  maxOutputSize?: number;
  /** Shell to use (default: false - no shell) */
  shell?: boolean;
  /** Callback for log output */
  onOutput?: LogCallback;
  /** ProcessSupervisor instance (optional, creates default if not provided) */
  supervisor?: ProcessSupervisor;
  /** FileOutputStreamer instance (optional, creates default if not provided) */
  outputStreamer?: FileOutputStreamer;
  /** Task ID for process tracking (optional, generates UUID if not provided) */
  taskId?: string;
}

/**
 * Result of a runner execution
 */
export interface RunnerResult {
  /** Final state */
  state: RunnerState;
  /** Exit code (if process exited normally) */
  exitCode?: number;
  /** Signal that killed the process (if applicable) */
  signal?: string;
  /** Captured stdout */
  stdout: string;
  /** Captured stderr */
  stderr: string;
  /** Combined output (interleaved by timestamp) */
  combined: string;
  /** Duration in milliseconds */
  durationMs: number;
  /** Error message (if failed) */
  error?: string;
}

/**
 * Events emitted by RalphRunner
 */
export interface RalphRunnerEvents {
  /** State changed */
  stateChange: (state: RunnerState, previousState: RunnerState) => void;
  /** Output line received */
  output: (entry: LogEntry) => void;
  /** Process spawned */
  spawned: (pid: number) => void;
  /** Process completed */
  completed: (result: RunnerResult) => void;
  /** Error occurred */
  error: (error: Error) => void;
}
