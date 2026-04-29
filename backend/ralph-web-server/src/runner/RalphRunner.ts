/**
 * RalphRunner
 *
 * Spawns and manages ralph run child processes. This is the execution engine
 * that bridges the Dispatcher's task model with actual CLI subprocess invocation.
 *
 * Lifecycle:
 *   IDLE → start() → SPAWNING → spawn complete → RUNNING → exit → COMPLETED/FAILED
 *                                                      ↓
 *                                              stop() → CANCELLED
 *
 * Integration:
 * - Can be registered as a TaskHandler with the Dispatcher
 * - Emits events for progress tracking via EventBus
 * - Uses LogStream to capture stdout/stderr
 * - Uses PromptWriter to pass prompt content to subprocess
 *
 * Design Notes:
 * - Single process per RalphRunner instance
 * - Supports cancellation via AbortSignal
 * - Graceful shutdown with SIGTERM, then SIGKILL after timeout
 * - Configurable command and arguments
 *
 * Module layout:
 * - `RalphRunnerTypes.ts`   — public interfaces (options, result, events)
 * - `RalphRunnerProcess.ts` — subprocess controller (spawn / stop / poll)
 * - `RalphRunnerResult.ts`  — pure result-building helpers
 * - `RalphRunner.ts`        — this file: state machine + event emission
 */

import { EventEmitter } from "events";
import {
  RunnerState,
  isTerminalRunnerState,
  isValidRunnerTransition,
} from "./RunnerState";
import { LogStream, LogCallback } from "./LogStream";
import { PromptWriter, PromptContent } from "./PromptWriter";
import { ProcessSupervisor } from "./ProcessSupervisor";
import { FileOutputStreamer } from "./FileOutputStreamer";
import { RalphRunnerProcessController } from "./RalphRunnerProcess";
import type { RalphRunnerOptions, RunnerResult } from "./RalphRunnerTypes";
import {
  buildErrorResult,
  buildExitResult,
  determineFinalState,
} from "./RalphRunnerResult";

// Re-export public types so existing import paths (`./RalphRunner`)
// continue to work unchanged.
export type {
  RalphRunnerOptions,
  RunnerResult,
  RalphRunnerEvents,
} from "./RalphRunnerTypes";

/**
 * RalphRunner
 *
 * Manages the lifecycle of a ralph run child process.
 */
export class RalphRunner extends EventEmitter {
  /** Current state */
  private _state: RunnerState = RunnerState.IDLE;
  /** Log stream for output capture */
  private logStream: LogStream;
  /** Prompt writer for temp files */
  private promptWriter: PromptWriter;
  /** Subprocess controller (spawn / stop / poll) */
  private processController: RalphRunnerProcessController;
  /** Current prompt file path */
  private promptFilePath?: string;
  /** Start timestamp */
  private startedAt?: Date;
  /** Resolve function for run() promise */
  private runResolve?: (result: RunnerResult) => void;
  /** Configuration */
  private readonly taskId: string;
  private readonly command: string;
  private readonly baseArgs: string[];
  private readonly cwd?: string;
  private readonly gracefulTimeoutMs: number;
  private readonly maxOutputSize: number;
  private readonly onOutput?: LogCallback;
  private readonly supervisor: ProcessSupervisor;
  private readonly outputStreamer: FileOutputStreamer;

  constructor(options: RalphRunnerOptions = {}) {
    super();

    // Prevent unhandled 'error' events from crashing the process.
    // Errors are still emitted for listeners that want them, but if no
    // listener is attached, the error is captured in the result.
    this.on("error", () => {
      // Intentionally empty - prevents Node.js from throwing.
    });

    this.taskId =
      options.taskId ?? `runner-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
    this.command = options.command ?? "ralph";
    this.baseArgs = options.baseArgs ?? ["run"];
    this.cwd = options.cwd;
    this.gracefulTimeoutMs = options.gracefulTimeoutMs ?? 5000;
    this.maxOutputSize = options.maxOutputSize ?? 10 * 1024 * 1024;
    this.onOutput = options.onOutput;
    this.supervisor = options.supervisor ?? new ProcessSupervisor();
    this.outputStreamer = options.outputStreamer ?? new FileOutputStreamer();

    this.logStream = new LogStream({
      maxBufferSize: this.maxOutputSize,
      onLine: (entry) => {
        this.emit("output", entry);
        if (this.onOutput) {
          this.onOutput(entry);
        }
      },
    });

    this.promptWriter = new PromptWriter();

    this.processController = new RalphRunnerProcessController({
      supervisor: this.supervisor,
      outputStreamer: this.outputStreamer,
      taskId: this.taskId,
      command: this.command,
      cwd: this.cwd ?? process.cwd(),
      gracefulTimeoutMs: this.gracefulTimeoutMs,
      logStream: this.logStream,
    });
  }

  /**
   * Get the current state
   */
  get state(): RunnerState {
    return this._state;
  }

  /**
   * Get the child process PID (if running)
   */
  get pid(): number | undefined {
    return this.processController.currentHandle?.pid;
  }

  /**
   * Transition to a new state
   */
  private setState(newState: RunnerState): void {
    if (!isValidRunnerTransition(this._state, newState)) {
      throw new Error(`Invalid state transition: ${this._state} -> ${newState}`);
    }

    const previousState = this._state;
    this._state = newState;
    this.emit("stateChange", newState, previousState);
  }

  /**
   * Run ralph with a text prompt
   *
   * @param prompt - The prompt text or structured content
   * @param additionalArgs - Additional CLI arguments
   * @param signal - Optional AbortSignal for cancellation
   * @returns Promise resolving to the execution result
   */
  async run(
    prompt: string | PromptContent,
    additionalArgs: string[] = [],
    signal?: AbortSignal
  ): Promise<RunnerResult> {
    // Check current state
    if (this._state !== RunnerState.IDLE) {
      throw new Error(`Cannot start runner in state: ${this._state}`);
    }

    // Reset for new run
    this.logStream.clear();
    this.startedAt = new Date();

    // Extract prompt text for ProcessSupervisor
    const promptText = typeof prompt === "string" ? prompt : JSON.stringify(prompt);

    // Write prompt to temp file for -P flag
    if (typeof prompt === "string") {
      this.promptFilePath = this.promptWriter.writeText(prompt);
    } else {
      this.promptFilePath = this.promptWriter.writePrompt(prompt);
    }

    // Build arguments (use -P flag for prompt file)
    const args = [...this.baseArgs, "-P", this.promptFilePath, ...additionalArgs];

    // Transition to SPAWNING
    this.setState(RunnerState.SPAWNING);

    return new Promise<RunnerResult>((resolve, reject) => {
      this.runResolve = resolve;

      try {
        this.processController.spawn(promptText, args, {
          onSpawned: (handle) => {
            // Transition to RUNNING
            this.setState(RunnerState.RUNNING);
            this.emit("spawned", handle.pid);
          },
          onExit: (code, signalName) => {
            this.handleExit(code, signalName);
          },
        });

        // Set up abort signal handler
        if (signal) {
          if (signal.aborted) {
            this.stop();
          } else {
            signal.addEventListener("abort", () => {
              this.stop();
            });
          }
        }
      } catch (err) {
        this.handleError(err instanceof Error ? err : new Error(String(err)));
        reject(err);
      }
    });
  }

  /**
   * Stop the running process
   *
   * @param force - If true, skip graceful shutdown and SIGKILL immediately
   */
  async stop(force: boolean = false): Promise<void> {
    if (isTerminalRunnerState(this._state)) {
      return;
    }
    this.processController.stop(force);
  }

  /**
   * Handle process exit
   */
  private handleExit(code: number | null, signal: NodeJS.Signals | null): void {
    // Stop output streaming.
    this.processController.stopStreaming();

    // Flush any remaining output.
    this.logStream.close();

    // Clean up prompt file.
    this.cleanupPromptFile();

    // Determine final state and build the result.
    const finalState = determineFinalState({ code, signal });

    // Only transition if not already terminal (could have errored during spawn).
    if (!isTerminalRunnerState(this._state)) {
      this.setState(finalState);
    }

    const result = buildExitResult(this._state, { code, signal }, this.logStream, this.startedAt);

    // Clear process references.
    this.processController.clear();

    // Emit completion.
    this.emit("completed", result);

    // Resolve the run() promise.
    if (this.runResolve) {
      this.runResolve(result);
      this.runResolve = undefined;
    }
  }

  /**
   * Handle spawn/runtime errors
   */
  private handleError(err: Error): void {
    // Stop output streaming.
    this.processController.stopStreaming();

    // Flush any output we might have.
    this.logStream.close();

    // Clean up prompt file.
    this.cleanupPromptFile();

    // Transition to FAILED if not already terminal.
    if (!isTerminalRunnerState(this._state)) {
      this.setState(RunnerState.FAILED);
    }

    // Emit error.
    this.emit("error", err);

    // Build result.
    const result = buildErrorResult(err, this.logStream, this.startedAt);

    // Clear process references.
    this.processController.clear();

    // Emit completion.
    this.emit("completed", result);

    // Resolve the run() promise.
    if (this.runResolve) {
      this.runResolve(result);
      this.runResolve = undefined;
    }
  }

  /**
   * Delete the current prompt file, if any.
   */
  private cleanupPromptFile(): void {
    if (this.promptFilePath) {
      this.promptWriter.delete(this.promptFilePath);
      this.promptFilePath = undefined;
    }
  }

  /**
   * Get the current output without waiting for completion
   */
  getOutput(): { stdout: string; stderr: string; combined: string } {
    return {
      stdout: this.logStream.getStdoutText(),
      stderr: this.logStream.getStderrText(),
      combined: this.logStream.getCombinedText(),
    };
  }

  /**
   * Get output line count
   */
  getLineCount(): { stdout: number; stderr: number; total: number } {
    return this.logStream.getLineCount();
  }

  /**
   * Check if the runner is in a terminal state
   */
  isTerminal(): boolean {
    return isTerminalRunnerState(this._state);
  }

  /**
   * Check if the runner is currently running
   */
  isRunning(): boolean {
    return this._state === RunnerState.RUNNING;
  }

  /**
   * Reset the runner to IDLE state for reuse.
   * Only works if in a terminal state.
   */
  reset(): void {
    if (!isTerminalRunnerState(this._state) && this._state !== RunnerState.IDLE) {
      throw new Error(`Cannot reset runner in state: ${this._state}`);
    }

    this.processController.clear();
    this.promptFilePath = undefined;
    this.startedAt = undefined;
    this.runResolve = undefined;
    this.logStream.clear();
    this._state = RunnerState.IDLE;
  }

  /**
   * Clean up resources
   */
  dispose(): void {
    // Force stop if running.
    if (!isTerminalRunnerState(this._state)) {
      this.processController.forceKill();
    }

    // Stop output streaming.
    this.processController.stopStreaming();

    // Clean up prompt files.
    this.promptWriter.cleanupAll();

    // Close log stream.
    this.logStream.close();

    // Remove all listeners.
    this.removeAllListeners();
  }
}
