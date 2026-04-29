/**
 * RalphRunnerProcessController
 *
 * Encapsulates the subprocess-management concerns of {@link RalphRunner}:
 * - Spawning via {@link ProcessSupervisor}
 * - Wiring log-file streaming via {@link FileOutputStreamer}
 * - Polling for process exit
 * - Graceful stop (SIGTERM then SIGKILL after a timeout)
 * - Force kill on dispose
 *
 * The controller is intentionally event-free: consumers pass callbacks for
 * the two outcomes (exit and streamed line). This keeps it decoupled from
 * `events` and makes it easy to unit-test.
 */

import type { LogStream } from "./LogStream";
import type { ProcessHandle, ProcessSupervisor } from "./ProcessSupervisor";
import type { FileOutputStreamer } from "./FileOutputStreamer";

/** How often to poll for subprocess exit (ms). */
const PROCESS_POLL_INTERVAL_MS = 500;

/**
 * Callbacks the controller invokes during the lifecycle of a spawn.
 */
export interface ProcessControllerCallbacks {
  /** Called once the supervisor has successfully spawned the process. */
  onSpawned: (handle: ProcessHandle) => void;
  /**
   * Called once the process has exited. Exactly one of (code, signal) is
   * usually non-null — matching Node's `child_process` exit event semantics.
   */
  onExit: (code: number | null, signal: NodeJS.Signals | null) => void;
}

/**
 * Options for constructing a {@link RalphRunnerProcessController}.
 */
export interface ProcessControllerOptions {
  /** Supervisor that spawns and tracks the detached subprocess. */
  supervisor: ProcessSupervisor;
  /** Streamer that tails the supervisor's log files. */
  outputStreamer: FileOutputStreamer;
  /** Task identifier used by the supervisor for run-dir naming. */
  taskId: string;
  /** Command to execute (e.g. `ralph`). */
  command: string;
  /** Working directory for the subprocess. */
  cwd: string;
  /** How long to wait after SIGTERM before escalating to SIGKILL (ms). */
  gracefulTimeoutMs: number;
  /** LogStream receiving per-line stdout/stderr from the output streamer. */
  logStream: LogStream;
}

/**
 * Manages the subprocess lifecycle for a single run.
 */
export class RalphRunnerProcessController {
  private handle?: ProcessHandle;
  private pollTimer?: NodeJS.Timeout;
  private stopped = false;

  constructor(private readonly options: ProcessControllerOptions) {}

  /**
   * Spawn the process and begin streaming output. Returns the spawned
   * {@link ProcessHandle}. Any spawn error is thrown synchronously to the
   * caller.
   */
  spawn(promptText: string, args: string[], callbacks: ProcessControllerCallbacks): ProcessHandle {
    const { supervisor, outputStreamer, taskId, command, cwd, logStream } = this.options;

    // Spawn via ProcessSupervisor (writes to log files).
    this.handle = supervisor.spawn(taskId, promptText, args, cwd, command);

    // Stream output from the supervisor's log files into the LogStream.
    outputStreamer.stream(taskId, this.handle.taskDir, (line, source) => {
      if (source === "stdout") {
        logStream.writeStdout(Buffer.from(line + "\n"));
      } else {
        logStream.writeStderr(Buffer.from(line + "\n"));
      }
    });

    // Poll for process exit. ProcessSupervisor exposes liveness and the
    // final status.json, not a direct "exited" event, so we poll.
    this.pollTimer = setInterval(() => {
      if (!this.handle) {
        this.clearPoll();
        return;
      }
      if (!supervisor.isAlive(this.handle.pid)) {
        this.clearPoll();
        const status = supervisor.getStatus(taskId);
        callbacks.onExit(status?.exitCode ?? null, (status?.signal as NodeJS.Signals) ?? null);
      }
    }, PROCESS_POLL_INTERVAL_MS);

    callbacks.onSpawned(this.handle);
    return this.handle;
  }

  /**
   * Stop the subprocess gracefully (SIGTERM, escalating to SIGKILL after
   * `gracefulTimeoutMs`) or forcibly (SIGKILL immediately).
   *
   * Safe to call when no process is running — it is a no-op.
   */
  stop(force: boolean = false): void {
    if (!this.handle || this.stopped) {
      return;
    }
    this.stopped = true;
    const signal = force ? "SIGKILL" : "SIGTERM";

    try {
      process.kill(this.handle.pid, signal);
    } catch {
      // Process may have already exited.
    }

    if (force) {
      return;
    }

    // Schedule SIGKILL if the process doesn't honour SIGTERM.
    setTimeout(() => {
      if (!this.handle) {
        return;
      }
      try {
        process.kill(this.handle.pid, "SIGKILL");
      } catch {
        // Process may have already exited.
      }
    }, this.options.gracefulTimeoutMs);
  }

  /**
   * Best-effort SIGKILL used during dispose. Does not wait for exit or
   * schedule escalation — intended for teardown where the caller is already
   * discarding the runner.
   */
  forceKill(): void {
    if (!this.handle) {
      return;
    }
    try {
      process.kill(this.handle.pid, "SIGKILL");
    } catch {
      // Process may have already exited.
    }
  }

  /**
   * Stop log-file streaming for the current task. Safe to call multiple
   * times.
   */
  stopStreaming(): void {
    this.options.outputStreamer.stop(this.options.taskId);
  }

  /**
   * Release internal references and timers. Does not kill the process —
   * call {@link stop} or {@link forceKill} first if needed.
   */
  clear(): void {
    this.clearPoll();
    this.handle = undefined;
    this.stopped = false;
  }

  /** Current process handle, if any. */
  get currentHandle(): ProcessHandle | undefined {
    return this.handle;
  }

  private clearPoll(): void {
    if (this.pollTimer) {
      clearInterval(this.pollTimer);
      this.pollTimer = undefined;
    }
  }
}
