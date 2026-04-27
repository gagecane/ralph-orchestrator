/**
 * Hat router - operations for managing hats (operational roles).
 */

import { TRPCError } from "@trpc/server";
import { z } from "zod";
import { router, publicProcedure } from "../context";

export const hatRouter = router({
  /**
   * List all hat definitions from settings
   */
  list: publicProcedure.query(({ ctx }) => {
    const definitions = ctx.settingsService.getHatDefinitions();
    const activeHat = ctx.settingsService.getActiveHat();

    // Convert map to array with active status
    return Object.entries(definitions).map(([key, hat]) => ({
      key,
      ...hat,
      isActive: key === activeHat,
    }));
  }),

  /**
   * Get the currently active hat
   */
  getActive: publicProcedure.query(({ ctx }) => {
    const activeKey = ctx.settingsService.getActiveHat();
    const definition = ctx.settingsService.getActiveHatDefinition();

    return {
      key: activeKey,
      definition: definition ?? null,
    };
  }),

  /**
   * Get a specific hat by key
   */
  get: publicProcedure.input(z.object({ key: z.string() })).query(({ ctx, input }) => {
    const hat = ctx.settingsService.getHat(input.key);
    if (!hat) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Hat '${input.key}' not found`,
      });
    }
    const activeKey = ctx.settingsService.getActiveHat();
    return {
      key: input.key,
      ...hat,
      isActive: input.key === activeKey,
    };
  }),

  /**
   * Set the active hat
   */
  setActive: publicProcedure.input(z.object({ key: z.string() })).mutation(({ ctx, input }) => {
    const hat = ctx.settingsService.getHat(input.key);
    if (!hat) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Hat '${input.key}' not found`,
      });
    }
    ctx.settingsService.setActiveHat(input.key);
    return { success: true, activeHat: input.key };
  }),

  /**
   * Save (create or update) a hat
   */
  save: publicProcedure
    .input(
      z.object({
        key: z.string().min(1),
        name: z.string().min(1),
        description: z.string(),
        triggersOn: z.array(z.string()),
        publishes: z.array(z.string()),
        instructions: z.string().optional(),
      })
    )
    .mutation(({ ctx, input }) => {
      const { key, ...definition } = input;
      ctx.settingsService.setHat(key, definition);
      return { success: true, key };
    }),

  /**
   * Delete a hat
   */
  delete: publicProcedure.input(z.object({ key: z.string() })).mutation(({ ctx, input }) => {
    const deleted = ctx.settingsService.deleteHat(input.key);
    if (!deleted) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Hat '${input.key}' not found`,
      });
    }
    return { success: true };
  }),
});
