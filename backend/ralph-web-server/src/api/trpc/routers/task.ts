/**
 * Task router - CRUD operations for tasks and task execution.
 */

import { TRPCError } from "@trpc/server";
import { z } from "zod";
import { router, publicProcedure } from "../context";

export const taskRouter = router({
  /**
   * List all tasks, optionally filtered by status and archival state
   */
  list: publicProcedure
    .input(
      z
        .object({
          status: z.string().optional(),
          includeArchived: z.boolean().default(false).optional(),
        })
        .optional()
    )
    .query(({ ctx, input }) => {
      return ctx.taskRepository.findAll(input?.status, input?.includeArchived);
    }),

  /**
   * Get a single task by ID
   */
  get: publicProcedure.input(z.object({ id: z.string() })).query(({ ctx, input }) => {
    const task = ctx.taskRepository.findById(input.id);
    if (!task) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Task with id '${input.id}' not found`,
      });
    }
    return task;
  }),

  /**
   * Get tasks that are ready to be worked on (not blocked)
   */
  ready: publicProcedure.query(({ ctx }) => {
    return ctx.taskRepository.findReady();
  }),

  /**
   * Create a new task and auto-execute it
   */
  create: publicProcedure
    .input(
      z.object({
        id: z.string(),
        title: z.string().min(1),
        status: z.string().default("open"),
        priority: z.number().int().min(1).max(5).default(2),
        blockedBy: z.string().nullable().optional(),
        autoExecute: z.boolean().default(true),
        preset: z.string().optional(),
      })
    )
    .mutation(({ ctx, input }) => {
      const { autoExecute, preset, ...taskData } = input;
      const task = ctx.taskRepository.create(taskData);

      // Auto-execute the task if requested and bridge is available
      if (autoExecute && ctx.taskBridge && !task.blockedBy) {
        ctx.taskBridge.enqueueTask(task, preset);
        // Return the updated task with pending status
        return ctx.taskRepository.findById(task.id) ?? task;
      }

      return task;
    }),

  /**
   * Run a specific task (enqueue for execution)
   */
  run: publicProcedure.input(z.object({ id: z.string() })).mutation(({ ctx, input }) => {
    if (!ctx.taskBridge) {
      throw new TRPCError({
        code: "INTERNAL_SERVER_ERROR",
        message: "Task execution is not configured",
      });
    }

    const task = ctx.taskRepository.findById(input.id);
    if (!task) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Task with id '${input.id}' not found`,
      });
    }

    const result = ctx.taskBridge.enqueueTask(task);
    if (!result.success) {
      throw new TRPCError({
        code: "BAD_REQUEST",
        message: result.error || "Failed to enqueue task",
      });
    }

    return {
      success: true,
      queuedTaskId: result.queuedTaskId,
      task: ctx.taskRepository.findById(input.id),
    };
  }),

  /**
   * Run all pending tasks
   */
  runAll: publicProcedure.mutation(({ ctx }) => {
    if (!ctx.taskBridge) {
      throw new TRPCError({
        code: "INTERNAL_SERVER_ERROR",
        message: "Task execution is not configured",
      });
    }

    const result = ctx.taskBridge.enqueueAllPending();
    return {
      enqueued: result.enqueued,
      errors: result.errors,
    };
  }),

  /**
   * Retry a failed task
   */
  retry: publicProcedure.input(z.object({ id: z.string() })).mutation(({ ctx, input }) => {
    if (!ctx.taskBridge) {
      throw new TRPCError({
        code: "INTERNAL_SERVER_ERROR",
        message: "Task execution is not configured",
      });
    }

    const result = ctx.taskBridge.retryTask(input.id);
    if (!result.success) {
      throw new TRPCError({
        code: "BAD_REQUEST",
        message: result.error || "Failed to retry task",
      });
    }

    return {
      success: true,
      queuedTaskId: result.queuedTaskId,
      task: ctx.taskRepository.findById(input.id),
    };
  }),

  /**
   * Get execution status for a task
   */
  executionStatus: publicProcedure.input(z.object({ id: z.string() })).query(({ ctx, input }) => {
    if (!ctx.taskBridge) {
      return { isQueued: false };
    }

    return ctx.taskBridge.getExecutionStatus(input.id);
  }),

  /**
   * Cancel a running task
   */
  cancel: publicProcedure.input(z.object({ id: z.string() })).mutation(({ ctx, input }) => {
    if (!ctx.taskBridge) {
      throw new TRPCError({
        code: "INTERNAL_SERVER_ERROR",
        message: "Task execution is not configured",
      });
    }

    const result = ctx.taskBridge.cancelTask(input.id);
    if (!result.success) {
      throw new TRPCError({
        code: "BAD_REQUEST",
        message: result.error || "Failed to cancel task",
      });
    }

    return {
      success: true,
      task: ctx.taskRepository.findById(input.id),
    };
  }),

  /**
   * Update an existing task
   */
  update: publicProcedure
    .input(
      z.object({
        id: z.string(),
        title: z.string().min(1).optional(),
        status: z.string().optional(),
        priority: z.number().int().min(1).max(5).optional(),
        blockedBy: z.string().nullable().optional(),
      })
    )
    .mutation(({ ctx, input }) => {
      const { id, ...updates } = input;
      const task = ctx.taskRepository.update(id, updates);
      if (!task) {
        throw new TRPCError({
          code: "NOT_FOUND",
          message: `Task with id '${id}' not found`,
        });
      }
      return task;
    }),

  /**
   * Close a task
   */
  close: publicProcedure.input(z.object({ id: z.string() })).mutation(({ ctx, input }) => {
    const task = ctx.taskRepository.close(input.id);
    if (!task) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Task with id '${input.id}' not found`,
      });
    }
    return task;
  }),

  /**
   * Archive a task
   */
  archive: publicProcedure.input(z.object({ id: z.string() })).mutation(({ ctx, input }) => {
    const task = ctx.taskRepository.archive(input.id);
    if (!task) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Task with id '${input.id}' not found`,
      });
    }
    return task;
  }),

  /**
   * Unarchive a task
   */
  unarchive: publicProcedure.input(z.object({ id: z.string() })).mutation(({ ctx, input }) => {
    const task = ctx.taskRepository.unarchive(input.id);
    if (!task) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Task with id '${input.id}' not found`,
      });
    }
    return task;
  }),

  /**
   * Delete a task
   *
   * Security: Only allows deletion of tasks in terminal states (failed, closed)
   * to prevent accidental data loss from running or pending tasks.
   */
  delete: publicProcedure.input(z.object({ id: z.string() })).mutation(({ ctx, input }) => {
    // First verify the task exists and check its state
    const task = ctx.taskRepository.findById(input.id);
    if (!task) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Task with id '${input.id}' not found`,
      });
    }

    // Only allow deletion of tasks in terminal states
    const deletableStates = ["failed", "closed"];
    if (!deletableStates.includes(task.status)) {
      throw new TRPCError({
        code: "PRECONDITION_FAILED",
        message: `Cannot delete task in '${task.status}' state. Only failed or closed tasks can be deleted.`,
      });
    }

    const deleted = ctx.taskRepository.delete(input.id);
    if (!deleted) {
      throw new TRPCError({
        code: "INTERNAL_SERVER_ERROR",
        message: `Failed to delete task '${input.id}'`,
      });
    }
    return { success: true };
  }),

  /**
   * Delete all tasks and task logs.
   */
  clearAll: publicProcedure.mutation(({ ctx }) => {
    const deletedLogs = ctx.taskLogRepository.deleteAll();
    const deletedTasks = ctx.taskRepository.deleteAll();
    return { success: true, deletedTasks, deletedLogs };
  }),
});
