/**
 * CollectionRepository Tests
 *
 * Tests for the CRUD operations on hat collections. Covers:
 * - Create with auto-generated id and default empty graph
 * - Create with provided graph data
 * - findAll omits graph data (listing mode)
 * - findById parses graph JSON
 * - Update partial fields (name, description, graph)
 * - Delete and null/false handling for missing ids
 */

import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { CollectionRepository, GraphData } from "./CollectionRepository";
import {
  initializeTestDatabase,
  getTestDatabase,
  closeTestDatabase,
} from "../db/testUtils";

function makeGraph(): GraphData {
  return {
    nodes: [
      {
        id: "planner",
        type: "hatNode",
        position: { x: 10, y: 20 },
        data: {
          key: "planner",
          name: "Planner",
          description: "Plans work",
          triggersOn: ["task.start"],
          publishes: ["plan.done"],
          instructions: "Plan carefully.",
        },
      },
    ],
    edges: [],
    viewport: { x: 1, y: 2, zoom: 1.5 },
  };
}

describe("CollectionRepository", () => {
  let repo: CollectionRepository;

  beforeEach(() => {
    initializeTestDatabase();
    repo = new CollectionRepository(getTestDatabase());
  });

  afterEach(() => {
    closeTestDatabase();
  });

  describe("create", () => {
    it("generates an id and uses an empty default graph when none is provided", () => {
      const created = repo.create({ name: "My Flow" });

      assert.ok(created.id);
      assert.equal(created.name, "My Flow");
      assert.equal(created.description, null);
      assert.deepEqual(created.graph.nodes, []);
      assert.deepEqual(created.graph.edges, []);
      assert.deepEqual(created.graph.viewport, { x: 0, y: 0, zoom: 1 });
      assert.ok(created.createdAt instanceof Date);
      assert.ok(created.updatedAt instanceof Date);
    });

    it("persists the provided graph and description", () => {
      const graph = makeGraph();
      const created = repo.create({
        name: "With Graph",
        description: "A collection with nodes",
        graph,
      });

      const fetched = repo.findById(created.id);
      assert.ok(fetched);
      assert.equal(fetched!.description, "A collection with nodes");
      assert.deepEqual(fetched!.graph, graph);
    });
  });

  describe("findAll", () => {
    it("returns collections without graph data", () => {
      repo.create({ name: "A", graph: makeGraph() });
      repo.create({ name: "B" });

      const rows = repo.findAll();
      assert.equal(rows.length, 2);
      for (const row of rows) {
        // graphData field must not be exposed to the listing consumer
        assert.equal(
          (row as unknown as { graphData?: string }).graphData,
          undefined,
        );
        assert.equal((row as unknown as { graph?: GraphData }).graph, undefined);
      }
    });

    it("returns an empty array when no collections exist", () => {
      assert.deepEqual(repo.findAll(), []);
    });
  });

  describe("findById", () => {
    it("returns null for a missing collection", () => {
      assert.equal(repo.findById("does-not-exist"), null);
    });

    it("parses the stored graph JSON on read", () => {
      const graph = makeGraph();
      const created = repo.create({ name: "X", graph });
      const fetched = repo.findById(created.id);
      assert.ok(fetched);
      assert.deepEqual(fetched!.graph, graph);
    });
  });

  describe("update", () => {
    it("returns null for a missing collection", () => {
      assert.equal(repo.update("missing", { name: "New" }), null);
    });

    it("updates only the fields that are provided", () => {
      const created = repo.create({
        name: "Original",
        description: "orig",
        graph: makeGraph(),
      });
      const originalGraph = created.graph;

      const updated = repo.update(created.id, { name: "Renamed" });
      assert.ok(updated);
      assert.equal(updated!.name, "Renamed");
      assert.equal(updated!.description, "orig");
      assert.deepEqual(updated!.graph, originalGraph);
      // SQLite timestamp columns are second-precision — compare at that
      // granularity to avoid ms-level truncation flakes when `created` is
      // the in-memory pre-round-trip value.
      const createdSec = Math.floor(created.updatedAt.getTime() / 1000);
      const updatedSec = Math.floor(updated!.updatedAt.getTime() / 1000);
      assert.ok(updatedSec >= createdSec);
    });

    it("persists a new graph when graph is provided", () => {
      const created = repo.create({ name: "X", graph: makeGraph() });
      const newGraph: GraphData = {
        nodes: [],
        edges: [
          {
            id: "e1",
            source: "a",
            target: "b",
            label: "done",
          },
        ],
        viewport: { x: 5, y: 5, zoom: 2 },
      };

      const updated = repo.update(created.id, { graph: newGraph });
      assert.ok(updated);
      assert.deepEqual(updated!.graph, newGraph);
    });

    it("allows clearing the description by setting it to null", () => {
      const created = repo.create({ name: "X", description: "keep" });
      // The repository type currently accepts string only via TS, but the
      // runtime-level contract is "if provided, write it". Passing null via
      // cast documents the current behavior.
      const updated = repo.update(created.id, {
        description: null as unknown as string,
      });
      assert.ok(updated);
      assert.equal(updated!.description, null);
    });
  });

  describe("delete", () => {
    it("returns true and removes the collection when it exists", () => {
      const created = repo.create({ name: "Temp" });
      assert.equal(repo.delete(created.id), true);
      assert.equal(repo.findById(created.id), null);
    });

    it("returns false when the collection does not exist", () => {
      assert.equal(repo.delete("nope"), false);
    });
  });
});
