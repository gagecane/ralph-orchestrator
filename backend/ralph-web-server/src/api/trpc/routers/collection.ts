/**
 * Collection router - operations for managing hat collections (visual workflow builder).
 */

import { TRPCError } from "@trpc/server";
import { z } from "zod";
import { router, publicProcedure } from "../context";

/**
 * Zod schema for graph node position
 */
const nodePositionSchema = z.object({
  x: z.number(),
  y: z.number(),
});

/**
 * Zod schema for hat node data
 */
const hatNodeDataSchema = z.object({
  key: z.string(),
  name: z.string(),
  description: z.string(),
  triggersOn: z.array(z.string()),
  publishes: z.array(z.string()),
  instructions: z.string().optional(),
});

/**
 * Zod schema for graph node
 */
const graphNodeSchema = z.object({
  id: z.string(),
  type: z.string(),
  position: nodePositionSchema,
  data: hatNodeDataSchema,
});

/**
 * Zod schema for graph edge
 */
const graphEdgeSchema = z.object({
  id: z.string(),
  source: z.string(),
  target: z.string(),
  sourceHandle: z.string().optional(),
  targetHandle: z.string().optional(),
  label: z.string().optional(),
});

/**
 * Zod schema for viewport
 */
const viewportSchema = z.object({
  x: z.number(),
  y: z.number(),
  zoom: z.number(),
});

/**
 * Zod schema for complete graph data
 */
const graphDataSchema = z.object({
  nodes: z.array(graphNodeSchema),
  edges: z.array(graphEdgeSchema),
  viewport: viewportSchema,
});

export const collectionRouter = router({
  /**
   * List all collections (metadata only, no graph data)
   */
  list: publicProcedure.query(({ ctx }) => {
    return ctx.collectionService.listCollections();
  }),

  /**
   * Get a single collection with full graph data
   */
  get: publicProcedure.input(z.object({ id: z.string() })).query(({ ctx, input }) => {
    const collection = ctx.collectionService.getCollection(input.id);
    if (!collection) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Collection with id '${input.id}' not found`,
      });
    }
    return collection;
  }),

  /**
   * Create a new collection
   */
  create: publicProcedure
    .input(
      z.object({
        name: z.string().min(1),
        description: z.string().optional(),
        graph: graphDataSchema.optional(),
      })
    )
    .mutation(({ ctx, input }) => {
      return ctx.collectionService.createCollection(input);
    }),

  /**
   * Update an existing collection
   */
  update: publicProcedure
    .input(
      z.object({
        id: z.string(),
        name: z.string().min(1).optional(),
        description: z.string().optional(),
        graph: graphDataSchema.optional(),
      })
    )
    .mutation(({ ctx, input }) => {
      const { id, ...updates } = input;
      const collection = ctx.collectionService.updateCollection(id, updates);
      if (!collection) {
        throw new TRPCError({
          code: "NOT_FOUND",
          message: `Collection with id '${id}' not found`,
        });
      }
      return collection;
    }),

  /**
   * Delete a collection
   */
  delete: publicProcedure.input(z.object({ id: z.string() })).mutation(({ ctx, input }) => {
    const deleted = ctx.collectionService.deleteCollection(input.id);
    if (!deleted) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Collection with id '${input.id}' not found`,
      });
    }
    return { success: true };
  }),

  /**
   * Export a collection to Ralph YAML preset format
   */
  exportYaml: publicProcedure.input(z.object({ id: z.string() })).query(({ ctx, input }) => {
    const yaml = ctx.collectionService.exportToYaml(input.id);
    if (!yaml) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: `Collection with id '${input.id}' not found`,
      });
    }
    return { yaml };
  }),

  /**
   * Import a YAML preset as a new collection
   */
  importYaml: publicProcedure
    .input(
      z.object({
        yaml: z.string(),
        name: z.string().min(1),
        description: z.string().optional(),
      })
    )
    .mutation(({ ctx, input }) => {
      try {
        return ctx.collectionService.importFromYaml(input.yaml, input.name, input.description);
      } catch (error) {
        throw new TRPCError({
          code: "BAD_REQUEST",
          message: `Failed to import YAML: ${error instanceof Error ? error.message : "Unknown error"}`,
        });
      }
    }),
});
