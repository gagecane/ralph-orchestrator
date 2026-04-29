/**
 * tRPC Collection Router Tests
 *
 * Tests for the collection.* endpoints that manage hat collections (the
 * visual workflow builder):
 *   - collection.list / .get / .create / .update / .delete
 *   - collection.exportYaml / .importYaml
 *
 * Uses an in-memory SQLite database via the existing connection module so the
 * CollectionRepository + CollectionService stack is exercised end-to-end.
 */

import { test, describe } from "node:test";
import assert from "node:assert/strict";
import { appRouter, createContext, collectionRouter } from "./trpc";
import {
  initializeDatabase,
  getDatabase,
  closeDatabase,
} from "../db/connection";
import type { GraphData } from "../repositories/CollectionRepository";

/**
 * Fresh in-memory DB per test (getDatabase caches — must close between tests).
 */
function freshCtx() {
  closeDatabase();
  initializeDatabase(getDatabase(":memory:"));
  return createContext(getDatabase());
}

const EMPTY_GRAPH: GraphData = {
  nodes: [],
  edges: [],
  viewport: { x: 0, y: 0, zoom: 1 },
};

const SAMPLE_GRAPH: GraphData = {
  nodes: [
    {
      id: "reviewer",
      type: "hatNode",
      position: { x: 100, y: 100 },
      data: {
        key: "reviewer",
        name: "Reviewer",
        description: "Reviews code",
        triggersOn: ["code.change"],
        publishes: ["review.done"],
      },
    },
    {
      id: "builder",
      type: "hatNode",
      position: { x: 400, y: 100 },
      data: {
        key: "builder",
        name: "Builder",
        description: "Builds code",
        triggersOn: ["review.done"],
        publishes: ["build.done"],
      },
    },
  ],
  edges: [
    {
      id: "e1",
      source: "reviewer",
      target: "builder",
      label: "review.done",
    },
  ],
  viewport: { x: 0, y: 0, zoom: 0.8 },
};

describe("collection.list tRPC endpoint", () => {
  test("returns an empty array when no collections exist", async () => {
    // Given: fresh database
    const ctx = freshCtx();

    // When: listing collections
    const caller = appRouter.createCaller(ctx);
    const result = await caller.collection.list();

    // Then: empty array
    assert.ok(Array.isArray(result));
    assert.strictEqual(result.length, 0);
  });

  test("returns metadata for all created collections (no graph data)", async () => {
    // Given: two collections exist
    const ctx = freshCtx();
    await ctx.collectionService.createCollection({
      name: "One",
      description: "first",
      graph: SAMPLE_GRAPH,
    });
    await ctx.collectionService.createCollection({
      name: "Two",
      description: "second",
    });

    // When: listing collections
    const caller = appRouter.createCaller(ctx);
    const result = await caller.collection.list();

    // Then: both returned, and list() rows do NOT include graph data
    assert.strictEqual(result.length, 2);
    const names = result.map((c: any) => c.name).sort();
    assert.deepStrictEqual(names, ["One", "Two"]);
    for (const row of result) {
      assert.ok(!("graphData" in row), "list() must not leak graphData");
      assert.ok(!("graph" in row), "list() must not include hydrated graph");
    }
  });
});

describe("collection.get tRPC endpoint", () => {
  test("returns a collection with full graph data", async () => {
    // Given: a collection with a non-trivial graph
    const ctx = freshCtx();
    const created = ctx.collectionService.createCollection({
      name: "MyFlow",
      description: "A flow",
      graph: SAMPLE_GRAPH,
    });

    // When: fetching by id
    const caller = appRouter.createCaller(ctx);
    const result = await caller.collection.get({ id: created.id });

    // Then: we get back the graph including nodes + edges
    assert.strictEqual(result.id, created.id);
    assert.strictEqual(result.name, "MyFlow");
    assert.ok(result.graph, "get() should hydrate graph data");
    assert.strictEqual(result.graph.nodes.length, 2);
    assert.strictEqual(result.graph.edges.length, 1);
  });

  test("throws NOT_FOUND when the collection does not exist", async () => {
    // Given: no collections
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    // When/Then: NOT_FOUND for missing id
    await assert.rejects(
      () => caller.collection.get({ id: "no-such-id" }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        return true;
      }
    );
  });
});

describe("collection.create tRPC endpoint", () => {
  test("creates a collection with an empty graph when none provided", async () => {
    // Given: fresh database
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    // When: creating without a graph
    const result = await caller.collection.create({ name: "Empty" });

    // Then: collection exists with defaults (service/repo chooses shape;
    //       we just require name + id + a graph object)
    assert.ok(result.id);
    assert.strictEqual(result.name, "Empty");
    assert.ok(result.graph);
    assert.ok(Array.isArray(result.graph.nodes));
    assert.ok(Array.isArray(result.graph.edges));
  });

  test("creates a collection with the provided graph", async () => {
    // Given: fresh database
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    // When: creating with a populated graph
    const result = await caller.collection.create({
      name: "Populated",
      description: "has nodes",
      graph: SAMPLE_GRAPH,
    });

    // Then: graph round-trips through the repository
    assert.strictEqual(result.graph.nodes.length, 2);
    assert.strictEqual(result.graph.edges.length, 1);
    assert.strictEqual(result.graph.nodes[0]!.data.name, "Reviewer");

    // And: the list endpoint sees it
    const list = await caller.collection.list();
    assert.strictEqual(list.length, 1);
  });

  test("rejects empty name via zod", async () => {
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    await assert.rejects(
      () => caller.collection.create({ name: "" }),
      (err: any) => {
        assert.strictEqual(err.code, "BAD_REQUEST");
        return true;
      }
    );
  });
});

describe("collection.update tRPC endpoint", () => {
  test("updates the name of an existing collection", async () => {
    // Given: a collection exists
    const ctx = freshCtx();
    const created = ctx.collectionService.createCollection({
      name: "Original",
      graph: EMPTY_GRAPH,
    });

    // When: updating the name
    const caller = appRouter.createCaller(ctx);
    const updated = await caller.collection.update({
      id: created.id,
      name: "Renamed",
    });

    // Then: update is persisted and reflected in get()
    assert.strictEqual(updated.name, "Renamed");
    const fetched = await caller.collection.get({ id: created.id });
    assert.strictEqual(fetched.name, "Renamed");
  });

  test("updates the graph of an existing collection", async () => {
    // Given: a collection with an empty graph
    const ctx = freshCtx();
    const created = ctx.collectionService.createCollection({
      name: "Flow",
      graph: EMPTY_GRAPH,
    });

    // When: updating graph only
    const caller = appRouter.createCaller(ctx);
    const updated = await caller.collection.update({
      id: created.id,
      graph: SAMPLE_GRAPH,
    });

    // Then: the graph is replaced
    assert.strictEqual(updated.graph.nodes.length, 2);
    assert.strictEqual(updated.graph.edges.length, 1);
  });

  test("throws NOT_FOUND for an unknown id", async () => {
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    await assert.rejects(
      () => caller.collection.update({ id: "ghost", name: "new" }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        return true;
      }
    );
  });
});

describe("collection.delete tRPC endpoint", () => {
  test("deletes an existing collection", async () => {
    // Given: a collection exists
    const ctx = freshCtx();
    const created = ctx.collectionService.createCollection({
      name: "Trash me",
      graph: EMPTY_GRAPH,
    });

    // When: deleting via tRPC
    const caller = appRouter.createCaller(ctx);
    const result = await caller.collection.delete({ id: created.id });

    // Then: success + the collection is gone
    assert.deepStrictEqual(result, { success: true });
    const list = await caller.collection.list();
    assert.strictEqual(list.length, 0);
  });

  test("throws NOT_FOUND when deleting a missing collection", async () => {
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    await assert.rejects(
      () => caller.collection.delete({ id: "no-such" }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        return true;
      }
    );
  });
});

describe("collection.exportYaml tRPC endpoint", () => {
  test("exports a collection to Ralph YAML with hats section", async () => {
    // Given: a collection with two hats and one edge
    const ctx = freshCtx();
    const created = ctx.collectionService.createCollection({
      name: "Export Me",
      description: "A test flow",
      graph: SAMPLE_GRAPH,
    });

    // When: exporting via tRPC
    const caller = appRouter.createCaller(ctx);
    const result = await caller.collection.exportYaml({ id: created.id });

    // Then: we get back YAML text that mentions both hat keys and the event
    assert.ok(typeof result.yaml === "string");
    assert.ok(result.yaml.includes("reviewer"), "yaml should mention reviewer key");
    assert.ok(result.yaml.includes("builder"), "yaml should mention builder key");
    assert.ok(
      result.yaml.includes("review.done"),
      "yaml should include the edge event label"
    );
    assert.ok(
      result.yaml.includes("hats:"),
      "yaml should have a hats section"
    );
  });

  test("throws NOT_FOUND when exporting a missing collection", async () => {
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    await assert.rejects(
      () => caller.collection.exportYaml({ id: "missing" }),
      (err: any) => {
        assert.strictEqual(err.code, "NOT_FOUND");
        return true;
      }
    );
  });
});

describe("collection.importYaml tRPC endpoint", () => {
  const validYaml = `
hats:
  reviewer:
    name: Reviewer
    description: Reviews code
    triggers:
      - code.change
    publishes:
      - review.done
  builder:
    name: Builder
    description: Builds code
    triggers:
      - review.done
    publishes:
      - build.done
`;

  test("imports a valid YAML preset as a new collection", async () => {
    // Given: a fresh database
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    // When: importing a YAML preset
    const result = await caller.collection.importYaml({
      yaml: validYaml,
      name: "Imported Flow",
      description: "from yaml",
    });

    // Then: collection is created with both hats as nodes
    assert.strictEqual(result.name, "Imported Flow");
    assert.strictEqual(result.graph.nodes.length, 2);

    // And: an edge exists connecting reviewer -> builder via review.done
    const edge = result.graph.edges.find(
      (e) => e.source === "reviewer" && e.target === "builder"
    );
    assert.ok(edge, "expected a reviewer->builder edge");
    assert.strictEqual(edge?.label, "review.done");

    // And: it shows up in list()
    const list = await caller.collection.list();
    assert.strictEqual(list.length, 1);
  });

  test("wraps parse errors as BAD_REQUEST", async () => {
    const ctx = freshCtx();
    const caller = appRouter.createCaller(ctx);

    // Input that blows up the YAML parser (unterminated flow mapping)
    const brokenYaml = "hats: { \n foo: [ }";

    await assert.rejects(
      () =>
        caller.collection.importYaml({
          yaml: brokenYaml,
          name: "Broken",
        }),
      (err: any) => {
        assert.strictEqual(err.code, "BAD_REQUEST");
        assert.ok(
          /Failed to import YAML/.test(err.message),
          `expected failure message, got: ${err.message}`
        );
        return true;
      }
    );
  });
});

describe("collectionRouter (standalone) createCaller", () => {
  // Sanity: the exported collectionRouter can be called directly (used by
  // the index.ts re-export path and allows targeted router testing).
  test("round-trip via collectionRouter directly", async () => {
    const ctx = freshCtx();
    const caller = collectionRouter.createCaller(ctx);

    const created = await caller.create({
      name: "Direct",
      graph: EMPTY_GRAPH,
    });
    const fetched = await caller.get({ id: created.id });
    assert.strictEqual(fetched.name, "Direct");
  });
});
