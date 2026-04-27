/**
 * Planning router - operations for planning sessions.
 */

import { TRPCError } from "@trpc/server";
import { z } from "zod";
import { router, publicProcedure } from "../context";

export const planningRouter = router({
  /**
   * List all planning sessions.
   */
  list: publicProcedure.query(async ({ ctx }) => {
    if (!ctx.planningService) {
      throw new TRPCError({
        code: "INTERNAL_SERVER_ERROR",
        message: "PlanningService is not configured",
      });
    }
    return ctx.planningService.listSessions();
  }),

  /**
   * Get a specific planning session with conversation history.
   */
  get: publicProcedure
    .input(z.object({ id: z.string() }))
    .query(async ({ input, ctx }) => {
      if (!ctx.planningService) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "PlanningService is not configured",
        });
      }
      return ctx.planningService.getSession(input.id);
    }),

  /**
   * Start a new planning session.
   */
  start: publicProcedure
    .input(z.object({ prompt: z.string().min(1) }))
    .mutation(async ({ input, ctx }) => {
      if (!ctx.planningService) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "PlanningService is not configured",
        });
      }
      return ctx.planningService.startSession(input.prompt);
    }),

  /**
   * Submit a user response to a planning session.
   */
  respond: publicProcedure
    .input(
      z.object({
        sessionId: z.string(),
        promptId: z.string(),
        response: z.string(),
      })
    )
    .mutation(async ({ input, ctx }) => {
      if (!ctx.planningService) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "PlanningService is not configured",
        });
      }

      await ctx.planningService.submitResponse(
        input.sessionId,
        input.promptId,
        input.response
      );
      return { success: true };
    }),

  /**
   * Resume a paused planning session.
   */
  resume: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(async ({ input, ctx }) => {
      if (!ctx.planningService) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "PlanningService is not configured",
        });
      }

      await ctx.planningService.resumeSession(input.id);
      return { success: true };
    }),

  /**
   * Delete a planning session.
   */
  delete: publicProcedure
    .input(z.object({ id: z.string() }))
    .mutation(async ({ input, ctx }) => {
      if (!ctx.planningService) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "PlanningService is not configured",
        });
      }

      await ctx.planningService.deleteSession(input.id);
      return { success: true };
    }),

  /**
   * Get artifact content for a planning session.
   */
  getArtifact: publicProcedure
    .input(z.object({ sessionId: z.string(), filename: z.string() }))
    .query(async ({ input, ctx }) => {
      if (!ctx.planningService) {
        throw new TRPCError({
          code: "INTERNAL_SERVER_ERROR",
          message: "PlanningService is not configured",
        });
      }

      try {
        return await ctx.planningService.getArtifact(
          input.sessionId,
          input.filename
        );
      } catch (error) {
        throw new TRPCError({
          code: "NOT_FOUND",
          message:
            error instanceof Error ? error.message : "Artifact not found",
        });
      }
    }),
});
