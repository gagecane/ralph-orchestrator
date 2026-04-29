/**
 * DispatcherTypes
 *
 * Shared type definitions for the Dispatcher and its collaborators.
 * Extracted to keep Dispatcher.ts focused on runtime logic.
 */

import { EventBus } from "./EventBus";
import { QueuedTask } from "./TaskQueueService";

/**
 * Handler function for executing a specific task type.
 * Receives the task and returns a result or throws an error.
 */
export type TaskHandler<TPayload = Record<string, unknown>, TResult = unknown> = (
  task: QueuedTask,
  context: TaskExecutionContext
) => Promise<TResult> | TResult;

/**
 * Context provided to task handlers during execution
 */
export interface TaskExecutionContext {
  /** EventBus for publishing events during execution */
  eventBus: EventBus;
  /** Correlation ID for tracing */
  correlationId: string;
  /** Signal that can be checked for cancellation */
  signal: AbortSignal;
}

/**
 * Result of a task execution
 */
export interface TaskExecutionResult {
  /** The executed task */
  task: QueuedTask;
  /** Whether execution succeeded */
  success: boolean;
  /** Result returned by the handler (if successful) */
  result?: unknown;
  /** Error message (if failed) */
  error?: string;
  /** Execution duration in milliseconds */
  durationMs: number;
}

/**
 * Dispatcher configuration options
 */
export interface DispatcherOptions {
  /** Polling interval in milliseconds (default: 100ms) */
  pollIntervalMs?: number;
  /** Maximum concurrent tasks (default: 1 for sequential execution) */
  maxConcurrent?: number;
  /** Task timeout in milliseconds (default: 7200000ms = 2 hours) */
  taskTimeoutMs?: number;
  /** Whether to auto-start on construction (default: false) */
  autoStart?: boolean;
}

/**
 * Event types published by the Dispatcher
 */
export type DispatcherEventType =
  | "dispatcher.started"
  | "dispatcher.stopped"
  | "dispatcher.idle"
  | "task.started"
  | "task.completed"
  | "task.failed"
  | "task.cancelled"
  | "task.timeout";

/**
 * Dispatcher statistics
 */
export interface DispatcherStats {
  /** Whether the dispatcher is running */
  isRunning: boolean;
  /** Total tasks processed */
  totalProcessed: number;
  /** Tasks that completed successfully */
  successCount: number;
  /** Tasks that failed */
  failureCount: number;
  /** Tasks that were cancelled */
  cancelledCount: number;
  /** Currently running tasks */
  runningCount: number;
  /** Tasks that timed out */
  timeoutCount: number;
  /** Average execution time in ms */
  avgDurationMs: number;
  /** Uptime in milliseconds */
  uptimeMs: number;
}

/**
 * Mutable counters tracked by TaskExecutor and aggregated into DispatcherStats.
 * Kept separate from DispatcherStats so the executor can maintain counters
 * without needing to know about dispatcher runtime fields (isRunning, uptime, etc).
 */
export interface ExecutorCounters {
  totalProcessed: number;
  successCount: number;
  failureCount: number;
  cancelledCount: number;
  timeoutCount: number;
  totalDurationMs: number;
}
