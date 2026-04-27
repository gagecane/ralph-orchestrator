/**
 * Loops router - operations for managing ralph loops.
 */

import { TRPCError } from "@trpc/server";
import { z } from "zod";
import { router, publicProcedure } from "../context";

export const loopsRouter = router({
  /**
   * List all loops, optionally including terminal states
   */
  list: publicProcedure
    .input(
      z
        .object({
          includeTerminal: z.boolean().default(false).optional(),
        })
        .optional()
    )
    .query(async ({ ctx, input }) => {
      if (!ctx.loopsManager) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "LoopsManager is not configured",
        });
      }

      const loops = await ctx.loopsManager.listLoops();

      // Filter out terminal states unless requested
      const filteredLoops = !input?.includeTerminal
        ? loops.filter((loop) => !["merged", "discarded"].includes(loop.status))
        : loops;

      // Enrich worktree loops with merge button state
      const enrichedLoops = await Promise.all(
        filteredLoops.map(async (loop) => {
          // Only worktree loops (not in-place) need merge button state
          if (loop.location === "(in-place)") {
            return loop;
          }
          const mergeButtonState = await ctx.loopsManager!.getMergeButtonState(loop.id);
          return { ...loop, mergeButtonState };
        })
      );

      return enrichedLoops;
    }),

  /**
   * Get manager status (running state, interval, last processed time)
   */
  managerStatus: publicProcedure.query(({ ctx }) => {
    if (!ctx.loopsManager) {
      return { running: false, intervalMs: 0 };
    }
    return ctx.loopsManager.getStatus();
  }),

  /**
   * Process the merge queue
   */
  process: publicProcedure.mutation(async ({ ctx }) => {
    if (!ctx.loopsManager) {
      throw new TRPCError({
        code: "INTERNAL_SERVER_ERROR",
        message: "LoopsManager is not configured",
      });
    }

    await ctx.loopsManager.processMergeQueue();
    return { success: true };
  }),

  /**
   * Prune stale loops from crashed processes
   */
  prune: publicProcedure.mutation(async ({ ctx }) => {
    if (!ctx.loopsManager) {
      throw new TRPCError({
        code: "INTERNAL_SERVER_ERROR",
        message: "LoopsManager is not configured",
      });
    }

    await ctx.loopsManager.pruneStale();
    return { success: true };
  }),

  /**
   * Retry a failed merge with optional user steering input.
   * Steering input provides guidance to the merge-ralph process
   * for resolving conflicts or making merge decisions.
   */
  retry: publicProcedure
    .input(z.object({ id: z.string(), steeringInput: z.string().optional() }))
    .mutation(async ({ ctx, input }) => {
      if (!ctx.loopsManager) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "LoopsManager is not configured",
        });
      }

      await ctx.loopsManager.retryMerge(input.id, input.steeringInput);
      return { success: true };
    }),

  /**
   * Discard a stuck loop
   */
  discard: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(async ({ ctx, input }) => {
      if (!ctx.loopsManager) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "LoopsManager is not configured",
        });
      }

      await ctx.loopsManager.discardLoop(input.id);
      return { success: true };
    }),

  /**
   * Stop a running loop
   */
  stop: publicProcedure
    .input(z.object({ id: z.string(), force: z.boolean().optional() }))
    .mutation(async ({ ctx, input }) => {
      if (!ctx.loopsManager) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "LoopsManager is not configured",
        });
      }

      await ctx.loopsManager.stopLoop(input.id, input.force);
      return { success: true };
    }),

  /**
   * Force merge a loop
   */
  merge: publicProcedure
    .input(z.object({ id: z.string(), force: z.boolean().optional() }))
    .mutation(async ({ ctx, input }) => {
      if (!ctx.loopsManager) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "LoopsManager is not configured",
        });
      }

      await ctx.loopsManager.mergeLoop(input.id, input.force);
      return { success: true };
    }),

  /**
   * Trigger a merge task for a worktree loop.
   * Creates a new task with a predefined merge prompt and auto-executes it.
   * This implements the "Merge Loop as Task" UX pattern where merges are
   * visible as tasks in the task list with full execution tracking.
   */
  triggerMergeTask: publicProcedure
    .input(z.object({ loopId: z.string() }))
    .mutation(async ({ ctx, input }) => {
      if (!ctx.loopsManager) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "LoopsManager is not configured",
        });
      }

      if (!ctx.taskBridge) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "TaskBridge is not configured",
        });
      }

      // Get loop info to build the merge prompt
      const loops = await ctx.loopsManager.listLoops();
      const loop = loops.find((l) => l.id === input.loopId);

      if (!loop) {
        throw new TRPCError({
          code: "NOT_FOUND",
          message: `Loop '${input.loopId}' not found`,
        });
      }

      if (loop.location === "(in-place)") {
        throw new TRPCError({
          code: "BAD_REQUEST",
          message: "Cannot trigger merge for in-place loop (primary)",
        });
      }

      // Build the merge prompt with context about the worktree changes
      const mergePrompt = `Merge worktree loop '${input.loopId}' into main branch.

The worktree is located at: ${loop.location}
Original task: ${loop.prompt || "(no prompt recorded)"}

Instructions:
1. Review the commits in the worktree branch
2. Merge the changes into main branch
3. Resolve any conflicts if present
4. Delete the worktree after successful merge`;

      // Create the task with merge prompt stored in mergeLoopPrompt field
      const taskId = `merge-${input.loopId}-${Date.now()}`;
      const task = ctx.taskRepository.create({
        id: taskId,
        title: `Merge: ${loop.prompt?.slice(0, 50) || input.loopId}`,
        status: "open",
        priority: 1, // High priority for merges
        mergeLoopPrompt: mergePrompt,
      });

      // Auto-execute the task
      const result = ctx.taskBridge.enqueueTask(task);

      if (!result.success) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: result.error || "Failed to enqueue merge task",
        });
      }

      return {
        success: true,
        taskId: task.id,
        queuedTaskId: result.queuedTaskId,
      };
    }),

  /**
   * Get merge button state for a loop
   */
  mergeButtonState: publicProcedure
    .input(z.object({ id: z.string() }))
    .query(async ({ ctx, input }) => {
      if (!ctx.loopsManager) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "LoopsManager is not configured",
        });
      }

      return ctx.loopsManager.getMergeButtonState(input.id);
    }),
});
