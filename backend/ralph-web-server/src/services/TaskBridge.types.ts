/**
 * TaskBridge shared types.
 *
 * Split out of TaskBridge.ts to keep the service class focused on orchestration
 * while making event payloads and public result shapes easy to locate.
 */

import type { QueuedTask } from "../queue/TaskQueueService";

/**
 * Result from RalphRunner (partial interface for what we need)
 */
export interface RunnerResultPayload {
  stdout?: string;
  stderr?: string;
  combined?: string;
  exitCode?: number;
}

/**
 * Payload for task.started events
 */
export interface TaskStartedPayload {
  taskId: string;
  taskType: string;
  payload: Record<string, unknown>;
  priority: number;
}

/**
 * Payload for task.completed events
 */
export interface TaskCompletedPayload {
  taskId: string;
  taskType: string;
  result: RunnerResultPayload;
  durationMs: number;
}

/**
 * Payload for task.failed events
 */
export interface TaskFailedPayload {
  taskId: string;
  taskType: string;
  error: string;
  durationMs: number;
}

/**
 * Payload for task.timeout events
 */
export interface TaskTimeoutPayload {
  taskId: string;
  taskType: string;
  timeoutMs: number;
  durationMs: number;
}

/**
 * Result of enqueuing a task
 */
export interface EnqueueResult {
  success: boolean;
  queuedTaskId?: string;
  error?: string;
}

/**
 * Result of enqueuing all pending tasks
 */
export interface EnqueueAllResult {
  enqueued: number;
  errors: Array<{ taskId: string; error: string }>;
}

/**
 * Execution status for a database task
 */
export interface ExecutionStatus {
  isQueued: boolean;
  queuedTask?: QueuedTask;
}
