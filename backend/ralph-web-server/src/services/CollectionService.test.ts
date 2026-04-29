/**
 * CollectionService Tests
 *
 * Tests for the collection business-logic layer sitting on top of
 * CollectionRepository. Covers:
 * - CRUD pass-through to the repository
 * - YAML export (derives triggers/publishes from edges, derives events)
 * - YAML import (creates nodes from hats, derives edges from events)
 * - Round-trip (export → import preserves hats + event topology)
 */

import { test, describe, beforeEach, afterEach } from "node:test";
import assert from "node:assert";
import { parse as yamlParse, stringify as yamlStringify } from "yaml";
import { CollectionService } from "./CollectionService";
import {
  CollectionRepository,
  GraphData,
} from "../repositories/CollectionRepository";
import {
  initializeTestDatabase,
  getTestDatabase,
  closeTestDatabase,
} from "../db/testUtils";

function makeService(): { service: CollectionService; repo: CollectionRepository } {
  const db = getTestDatabase();
  const repo = new CollectionRepository(db);
  return { service: new CollectionService(repo), repo };
}

/**
 * Helper: build a simple two-hat graph (planner -> builder via plan.done).
 */
function twoHatGraph(): GraphData {
  return {
    nodes: [
      {
        id: "planner",
        type: "hatNode",
        position: { x: 0, y: 0 },
        data: {
          key: "planner",
          name: "Planner",
          description: "Plans work",
          triggersOn: ["task.start"],
          publishes: [],
          instructions: "Plan carefully.",
        },
      },
      {
        id: "builder",
        type: "hatNode",
        position: { x: 0, y: 200 },
        data: {
          key: "builder",
          name: "Builder",
          description: "Builds code",
          triggersOn: [],
          publishes: ["build.done"],
        },
      },
    ],
    edges: [
      {
        id: "e1",
        source: "planner",
        target: "builder",
        label: "plan.done",
      },
    ],
    viewport: { x: 0, y: 0, zoom: 1 },
  };
}

describe("CollectionService", () => {
  beforeEach(() => {
    initializeTestDatabase();
  });

  afterEach(() => {
    closeTestDatabase();
  });

  describe("CRUD operations", () => {
    test("createCollection persists and returns the new collection with graph", () => {
      const { service } = makeService();

      const created = service.createCollection({
        name: "My Collection",
        description: "test",
      });

      assert.ok(created.id, "Should assign an id");
      assert.strictEqual(created.name, "My Collection");
      assert.strictEqual(created.description, "test");
      assert.ok(created.graph, "Should have graph");
      assert.deepStrictEqual(created.graph.nodes, []);
      assert.deepStrictEqual(created.graph.edges, []);
    });

    test("getCollection returns the persisted collection", () => {
      const { service } = makeService();
      const created = service.createCollection({ name: "A" });

      const fetched = service.getCollection(created.id);
      assert.ok(fetched);
      assert.strictEqual(fetched.id, created.id);
      assert.strictEqual(fetched.name, "A");
    });

    test("getCollection returns null for unknown id", () => {
      const { service } = makeService();
      assert.strictEqual(service.getCollection("nope"), null);
    });

    test("listCollections returns metadata without graphData", () => {
      const { service } = makeService();
      service.createCollection({ name: "One" });
      service.createCollection({ name: "Two" });

      const all = service.listCollections();
      assert.strictEqual(all.length, 2);
      const names = all.map((c) => c.name).sort();
      assert.deepStrictEqual(names, ["One", "Two"]);
      // graphData should not be present on list items
      for (const c of all) {
        assert.ok(!("graphData" in c), "listCollections should omit graphData");
      }
    });

    test("updateCollection updates name, description, and graph", () => {
      const { service } = makeService();
      const created = service.createCollection({
        name: "Old",
        description: "old-desc",
      });

      const updated = service.updateCollection(created.id, {
        name: "New",
        description: "new-desc",
        graph: twoHatGraph(),
      });

      assert.ok(updated);
      assert.strictEqual(updated.name, "New");
      assert.strictEqual(updated.description, "new-desc");
      assert.strictEqual(updated.graph.nodes.length, 2);
      assert.strictEqual(updated.graph.edges.length, 1);
    });

    test("updateCollection returns null for unknown id", () => {
      const { service } = makeService();
      assert.strictEqual(
        service.updateCollection("nope", { name: "x" }),
        null,
      );
    });

    test("deleteCollection removes the collection and returns true", () => {
      const { service } = makeService();
      const created = service.createCollection({ name: "Gone" });

      assert.strictEqual(service.deleteCollection(created.id), true);
      assert.strictEqual(service.getCollection(created.id), null);
    });

    test("deleteCollection returns false for unknown id", () => {
      const { service } = makeService();
      assert.strictEqual(service.deleteCollection("nope"), false);
    });
  });

  describe("exportToYaml", () => {
    test("returns null for unknown collection id", () => {
      const { service } = makeService();
      assert.strictEqual(service.exportToYaml("nope"), null);
    });

    test("emits a YAML header comment with the collection name", () => {
      const { service } = makeService();
      const created = service.createCollection({
        name: "Header Test",
        description: "desc here",
        graph: twoHatGraph(),
      });

      const yaml = service.exportToYaml(created.id);
      assert.ok(yaml);
      // Header has three comment lines: name, description, generated-at
      const lines = yaml.split("\n");
      assert.ok(lines[0].startsWith("# Header Test"));
      assert.ok(lines[1].startsWith("# desc here"));
      assert.ok(lines[2].startsWith("# Generated at:"));
    });

    test("falls back to a default header description when none provided", () => {
      const { service } = makeService();
      const created = service.createCollection({
        name: "No Desc",
        graph: twoHatGraph(),
      });

      const yaml = service.exportToYaml(created.id);
      assert.ok(yaml);
      assert.match(yaml, /Generated by Ralph Hat Collection Builder/);
    });

    test("derives triggers and publishes from edges", () => {
      const { service } = makeService();
      const created = service.createCollection({
        name: "Edge Derivation",
        graph: twoHatGraph(),
      });

      const yamlText = service.exportToYaml(created.id);
      assert.ok(yamlText);
      const parsed = yamlParse(yamlText) as any;

      // planner publishes plan.done (added by edge)
      assert.ok(parsed.hats.planner);
      assert.ok(
        parsed.hats.planner.publishes.includes("plan.done"),
        "planner should publish plan.done (derived from edge)",
      );

      // builder triggers on plan.done (added by edge)
      assert.ok(parsed.hats.builder);
      assert.ok(
        parsed.hats.builder.triggers.includes("plan.done"),
        "builder should trigger on plan.done (derived from edge)",
      );

      // builder's existing publish is preserved
      assert.ok(parsed.hats.builder.publishes.includes("build.done"));

      // events section should contain plan.done
      assert.ok(parsed.events);
      assert.ok(parsed.events["plan.done"]);
      assert.match(parsed.events["plan.done"].description, /plan.done/);
    });

    test("synthesizes an event name when an edge has no label", () => {
      const { service } = makeService();
      const graph: GraphData = twoHatGraph();
      graph.edges[0].label = undefined;

      const created = service.createCollection({
        name: "Unlabeled Edge",
        graph,
      });

      const yamlText = service.exportToYaml(created.id);
      assert.ok(yamlText);
      const parsed = yamlParse(yamlText) as any;

      const synthesized = "planner_to_builder";
      assert.ok(
        parsed.hats.planner.publishes.includes(synthesized),
        `planner should publish ${synthesized}`,
      );
      assert.ok(
        parsed.hats.builder.triggers.includes(synthesized),
        `builder should trigger on ${synthesized}`,
      );
    });

    test("sets default_publishes to the first publish event", () => {
      const { service } = makeService();
      const created = service.createCollection({
        name: "Defaults",
        graph: twoHatGraph(),
      });

      const yamlText = service.exportToYaml(created.id);
      const parsed = yamlParse(yamlText!) as any;

      // planner's first publish (derived) is plan.done
      assert.strictEqual(parsed.hats.planner.default_publishes, "plan.done");
      // builder publishes only build.done
      assert.strictEqual(parsed.hats.builder.default_publishes, "build.done");
    });

    test("omits default_publishes and instructions when not applicable", () => {
      const { service } = makeService();
      const graph: GraphData = {
        nodes: [
          {
            id: "lone",
            type: "hatNode",
            position: { x: 0, y: 0 },
            data: {
              key: "lone",
              name: "Lone",
              description: "No publishes, no instructions",
              triggersOn: [],
              publishes: [],
            },
          },
        ],
        edges: [],
        viewport: { x: 0, y: 0, zoom: 1 },
      };
      const created = service.createCollection({ name: "Minimal", graph });

      const yamlText = service.exportToYaml(created.id);
      const parsed = yamlParse(yamlText!) as any;

      const lone = parsed.hats.lone;
      assert.ok(lone, "hat should be emitted");
      assert.strictEqual(
        lone.default_publishes,
        undefined,
        "default_publishes should be omitted with no publishes",
      );
      assert.strictEqual(
        lone.instructions,
        undefined,
        "instructions should be omitted when absent",
      );
    });

    test("omits events section when there are no edges", () => {
      const { service } = makeService();
      const graph: GraphData = {
        nodes: [
          {
            id: "solo",
            type: "hatNode",
            position: { x: 0, y: 0 },
            data: {
              key: "solo",
              name: "Solo",
              description: "No edges",
              triggersOn: [],
              publishes: [],
            },
          },
        ],
        edges: [],
        viewport: { x: 0, y: 0, zoom: 1 },
      };
      const created = service.createCollection({ name: "NoEdges", graph });

      const yamlText = service.exportToYaml(created.id);
      const parsed = yamlParse(yamlText!) as any;

      assert.strictEqual(
        parsed.events,
        undefined,
        "events section should be omitted when no edges exist",
      );
    });

    test("preserves hat instructions when present", () => {
      const { service } = makeService();
      const graph = twoHatGraph();
      const created = service.createCollection({ name: "Inst", graph });

      const yamlText = service.exportToYaml(created.id);
      const parsed = yamlParse(yamlText!) as any;

      assert.strictEqual(parsed.hats.planner.instructions, "Plan carefully.");
    });
  });

  describe("importFromYaml", () => {
    test("creates nodes from each hat in the preset", () => {
      const { service } = makeService();

      const yaml = yamlStringify({
        hats: {
          alpha: {
            name: "Alpha",
            description: "First hat",
            triggers: ["task.start"],
            publishes: ["alpha.done"],
            instructions: "Do alpha things.",
          },
          beta: {
            name: "Beta",
            description: "Second hat",
            triggers: ["alpha.done"],
            publishes: ["beta.done"],
          },
        },
      });

      const created = service.importFromYaml(yaml, "Imported", "From YAML");

      assert.strictEqual(created.name, "Imported");
      assert.strictEqual(created.description, "From YAML");
      assert.strictEqual(created.graph.nodes.length, 2);

      const alpha = created.graph.nodes.find((n) => n.data.key === "alpha");
      const beta = created.graph.nodes.find((n) => n.data.key === "beta");
      assert.ok(alpha && beta);
      assert.strictEqual(alpha.data.name, "Alpha");
      assert.strictEqual(alpha.data.instructions, "Do alpha things.");
      assert.deepStrictEqual(alpha.data.publishes, ["alpha.done"]);
      assert.deepStrictEqual(beta.data.triggersOn, ["alpha.done"]);
    });

    test("creates edges for publisher -> subscriber relationships", () => {
      const { service } = makeService();
      const yaml = yamlStringify({
        hats: {
          publisher: {
            name: "P",
            description: "Publisher",
            publishes: ["event.x"],
          },
          subscriber: {
            name: "S",
            description: "Subscriber",
            triggers: ["event.x"],
          },
        },
      });

      const created = service.importFromYaml(yaml, "Events");

      assert.strictEqual(created.graph.edges.length, 1);
      const edge = created.graph.edges[0];
      assert.strictEqual(edge.source, "publisher");
      assert.strictEqual(edge.target, "subscriber");
      assert.strictEqual(edge.label, "event.x");
    });

    test("does not create self-loops when a hat both publishes and triggers on the same event", () => {
      const { service } = makeService();
      const yaml = yamlStringify({
        hats: {
          solo: {
            name: "Solo",
            description: "Self-cycler",
            publishes: ["ping"],
            triggers: ["ping"],
          },
        },
      });

      const created = service.importFromYaml(yaml, "SelfLoopTest");

      assert.strictEqual(created.graph.nodes.length, 1);
      assert.strictEqual(
        created.graph.edges.length,
        0,
        "should not add an edge where source === target",
      );
    });

    test("handles hats with no triggers or publishes without error", () => {
      const { service } = makeService();
      const yaml = yamlStringify({
        hats: {
          empty: {
            name: "Empty",
            description: "Nothing to do",
          },
        },
      });

      const created = service.importFromYaml(yaml, "Empty");

      assert.strictEqual(created.graph.nodes.length, 1);
      assert.deepStrictEqual(created.graph.nodes[0].data.triggersOn, []);
      assert.deepStrictEqual(created.graph.nodes[0].data.publishes, []);
      assert.strictEqual(created.graph.edges.length, 0);
    });

    test("returns an empty graph when YAML has no hats", () => {
      const { service } = makeService();
      const yaml = yamlStringify({ event_loop: { max_iterations: 10 } });

      const created = service.importFromYaml(yaml, "NoHats");

      assert.strictEqual(created.graph.nodes.length, 0);
      assert.strictEqual(created.graph.edges.length, 0);
    });
  });

  describe("export → import round-trip", () => {
    test("preserves hat keys, triggers, and publishes across export/import", () => {
      const { service } = makeService();
      const original = service.createCollection({
        name: "Round Trip",
        graph: twoHatGraph(),
      });

      const yaml = service.exportToYaml(original.id);
      assert.ok(yaml);

      const reimported = service.importFromYaml(yaml, "Round Trip (imported)");

      // Same set of hat keys
      const origKeys = original.graph.nodes.map((n) => n.data.key).sort();
      const newKeys = reimported.graph.nodes.map((n) => n.data.key).sort();
      assert.deepStrictEqual(newKeys, origKeys);

      // plan.done edge is preserved (planner -> builder)
      const hasPlanDoneEdge = reimported.graph.edges.some(
        (e) => e.source === "planner" && e.target === "builder" && e.label === "plan.done",
      );
      assert.ok(hasPlanDoneEdge, "plan.done edge should survive round-trip");
    });
  });
});
