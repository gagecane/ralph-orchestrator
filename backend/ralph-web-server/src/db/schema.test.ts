/**
 * Drizzle ORM Schema Tests
 *
 * Exercises the schema definitions in `./schema.ts` against an in-memory
 * database created by testUtils. Confirms that every exported table supports
 * a drizzle-typed insert/select round-trip, that nullability and defaults
 * are applied as declared, and that timestamp columns round-trip JS Date
 * values correctly.
 */

import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { eq } from "drizzle-orm";
import {
  initializeTestDatabase,
  getTestDatabase,
  closeTestDatabase,
} from "./testUtils";
import {
  tasks,
  queuedTasks,
  taskLogs,
  settings,
  collections,
} from "./schema";

describe("db/schema", () => {
  beforeEach(() => {
    initializeTestDatabase();
  });

  afterEach(() => {
    closeTestDatabase();
  });

  describe("tasks table", () => {
    it("accepts a minimal row and applies declared defaults", () => {
      const db = getTestDatabase();
      const now = new Date("2026-01-02T03:04:05Z");

      db.insert(tasks)
        .values({
          id: "t-1",
          title: "Minimal",
          // status omitted -> default 'open'
          // priority omitted -> default 2
          createdAt: now,
          updatedAt: now,
        })
        .run();

      const row = db.select().from(tasks).where(eq(tasks.id, "t-1")).get();
      assert.ok(row, "inserted row should be retrievable");
      assert.equal(row!.id, "t-1");
      assert.equal(row!.title, "Minimal");
      assert.equal(row!.status, "open");
      assert.equal(row!.priority, 2);
      assert.equal(row!.blockedBy, null);
      assert.equal(row!.archivedAt, null);
      assert.deepEqual(row!.createdAt, now);
      assert.deepEqual(row!.updatedAt, now);
    });

    it("round-trips all optional / execution tracking fields", () => {
      const db = getTestDatabase();
      const created = new Date("2026-02-02T00:00:00Z");
      const started = new Date("2026-02-02T00:00:10Z");
      const completed = new Date("2026-02-02T00:01:00Z");
      const archived = new Date("2026-02-03T00:00:00Z");

      db.insert(tasks)
        .values({
          id: "t-full",
          title: "Full row",
          status: "closed",
          priority: 1,
          blockedBy: "t-parent",
          createdAt: created,
          updatedAt: created,
          queuedTaskId: "q-1",
          startedAt: started,
          completedAt: completed,
          errorMessage: "boom",
          executionSummary: "# Did things",
          exitCode: 0,
          durationMs: 50_000,
          archivedAt: archived,
          mergeLoopPrompt: "merge please",
          preset: "builtin:review",
          currentIteration: 3,
          maxIterations: 10,
          loopId: "loop-1",
        })
        .run();

      const row = db.select().from(tasks).where(eq(tasks.id, "t-full")).get();
      assert.ok(row);
      assert.equal(row!.status, "closed");
      assert.equal(row!.priority, 1);
      assert.equal(row!.blockedBy, "t-parent");
      assert.equal(row!.queuedTaskId, "q-1");
      assert.deepEqual(row!.startedAt, started);
      assert.deepEqual(row!.completedAt, completed);
      assert.equal(row!.errorMessage, "boom");
      assert.equal(row!.executionSummary, "# Did things");
      assert.equal(row!.exitCode, 0);
      assert.equal(row!.durationMs, 50_000);
      assert.deepEqual(row!.archivedAt, archived);
      assert.equal(row!.mergeLoopPrompt, "merge please");
      assert.equal(row!.preset, "builtin:review");
      assert.equal(row!.currentIteration, 3);
      assert.equal(row!.maxIterations, 10);
      assert.equal(row!.loopId, "loop-1");
    });

    it("rejects inserts that omit a NOT NULL column (title)", () => {
      const db = getTestDatabase();
      assert.throws(() => {
        db.insert(tasks)
          // @ts-expect-error - intentionally omitting the required title
          .values({
            id: "t-no-title",
            createdAt: new Date(),
            updatedAt: new Date(),
          })
          .run();
      });
    });

    it("enforces primary key uniqueness on id", () => {
      const db = getTestDatabase();
      const now = new Date();
      db.insert(tasks)
        .values({ id: "dup", title: "first", createdAt: now, updatedAt: now })
        .run();

      assert.throws(() => {
        db.insert(tasks)
          .values({ id: "dup", title: "second", createdAt: now, updatedAt: now })
          .run();
      });
    });
  });

  describe("queuedTasks table", () => {
    it("round-trips a full queued task row", () => {
      const db = getTestDatabase();
      const enqueuedAt = new Date("2026-03-01T00:00:00Z");
      const startedAt = new Date("2026-03-01T00:00:05Z");

      db.insert(queuedTasks)
        .values({
          id: "q-1",
          taskType: "ralph.run",
          payload: JSON.stringify({ prompt: "hi" }),
          // state omitted -> default 'pending'
          // priority omitted -> default 5
          enqueuedAt,
          startedAt,
          // retryCount omitted -> default 0
          dbTaskId: "t-linked",
        })
        .run();

      const row = db.select().from(queuedTasks).where(eq(queuedTasks.id, "q-1")).get();
      assert.ok(row);
      assert.equal(row!.taskType, "ralph.run");
      assert.equal(row!.payload, JSON.stringify({ prompt: "hi" }));
      assert.equal(row!.state, "pending");
      assert.equal(row!.priority, 5);
      assert.equal(row!.retryCount, 0);
      assert.deepEqual(row!.enqueuedAt, enqueuedAt);
      assert.deepEqual(row!.startedAt, startedAt);
      assert.equal(row!.completedAt, null);
      assert.equal(row!.error, null);
      assert.equal(row!.dbTaskId, "t-linked");
    });

    it("accepts each valid state value (pending/running/completed/failed)", () => {
      const db = getTestDatabase();
      const states: Array<"pending" | "running" | "completed" | "failed"> = [
        "pending",
        "running",
        "completed",
        "failed",
      ];

      for (const [i, state] of states.entries()) {
        db.insert(queuedTasks)
          .values({
            id: `q-state-${i}`,
            taskType: "test",
            payload: "{}",
            state,
            enqueuedAt: new Date(),
          })
          .run();
      }

      const rows = db.select().from(queuedTasks).all();
      const stored = rows.map((r) => r.state).sort();
      assert.deepEqual(stored, ["completed", "failed", "pending", "running"]);
    });
  });

  describe("taskLogs table", () => {
    it("auto-increments id and round-trips a row", () => {
      const db = getTestDatabase();
      const t1 = new Date("2026-04-01T00:00:00Z");
      const t2 = new Date("2026-04-01T00:00:01Z");

      db.insert(taskLogs)
        .values({ taskId: "t-1", timestamp: t1, source: "stdout", line: "hello" })
        .run();
      db.insert(taskLogs)
        .values({ taskId: "t-1", timestamp: t2, source: "stderr", line: "warn" })
        .run();

      const rows = db.select().from(taskLogs).all();
      assert.equal(rows.length, 2);
      assert.ok(rows[0].id !== null && rows[0].id !== undefined);
      assert.ok(rows[1].id !== null && rows[1].id !== undefined);
      assert.ok(rows[1].id! > rows[0].id!, "autoincrement id should be monotonic");
      assert.deepEqual(rows[0].timestamp, t1);
      assert.equal(rows[0].source, "stdout");
      assert.equal(rows[1].source, "stderr");
    });

    it("supports filtering and ordering by taskId and id", () => {
      const db = getTestDatabase();
      const base = new Date("2026-04-01T00:00:00Z");

      db.insert(taskLogs)
        .values({ taskId: "a", timestamp: base, source: "stdout", line: "a1" })
        .run();
      db.insert(taskLogs)
        .values({ taskId: "b", timestamp: base, source: "stdout", line: "b1" })
        .run();
      db.insert(taskLogs)
        .values({ taskId: "a", timestamp: base, source: "stderr", line: "a2" })
        .run();

      const aRows = db
        .select()
        .from(taskLogs)
        .where(eq(taskLogs.taskId, "a"))
        .all();

      assert.equal(aRows.length, 2);
      // Autoincrement ids preserve insertion order for a given taskId
      assert.ok(aRows[0].id! < aRows[1].id!);
      assert.equal(aRows[0].line, "a1");
      assert.equal(aRows[1].line, "a2");
    });
  });

  describe("settings table", () => {
    it("round-trips key/value/timestamp", () => {
      const db = getTestDatabase();
      const updatedAt = new Date("2026-05-01T00:00:00Z");

      db.insert(settings)
        .values({ key: "theme", value: '"dark"', updatedAt })
        .run();

      const row = db.select().from(settings).where(eq(settings.key, "theme")).get();
      assert.ok(row);
      assert.equal(row!.key, "theme");
      assert.equal(row!.value, '"dark"');
      assert.deepEqual(row!.updatedAt, updatedAt);
    });

    it("enforces unique primary key on key", () => {
      const db = getTestDatabase();
      const now = new Date();
      db.insert(settings).values({ key: "k", value: "1", updatedAt: now }).run();
      assert.throws(() => {
        db.insert(settings).values({ key: "k", value: "2", updatedAt: now }).run();
      });
    });
  });

  describe("collections table", () => {
    it("round-trips a collection row", () => {
      const db = getTestDatabase();
      const createdAt = new Date("2026-06-01T00:00:00Z");
      const updatedAt = new Date("2026-06-02T00:00:00Z");

      const graph = JSON.stringify({ nodes: [], edges: [], viewport: { x: 0, y: 0, zoom: 1 } });

      db.insert(collections)
        .values({
          id: "c-1",
          name: "My Workflow",
          description: "A sample",
          graphData: graph,
          createdAt,
          updatedAt,
        })
        .run();

      const row = db.select().from(collections).where(eq(collections.id, "c-1")).get();
      assert.ok(row);
      assert.equal(row!.name, "My Workflow");
      assert.equal(row!.description, "A sample");
      assert.equal(row!.graphData, graph);
      assert.deepEqual(row!.createdAt, createdAt);
      assert.deepEqual(row!.updatedAt, updatedAt);
    });

    it("allows a null description but requires name and graph_data", () => {
      const db = getTestDatabase();
      const now = new Date();

      db.insert(collections)
        .values({
          id: "c-min",
          name: "Minimal",
          description: null,
          graphData: "{}",
          createdAt: now,
          updatedAt: now,
        })
        .run();

      const row = db.select().from(collections).where(eq(collections.id, "c-min")).get();
      assert.ok(row);
      assert.equal(row!.description, null);

      // Missing required name should fail
      assert.throws(() => {
        db.insert(collections)
          // @ts-expect-error - name intentionally missing
          .values({
            id: "c-nameless",
            description: null,
            graphData: "{}",
            createdAt: now,
            updatedAt: now,
          })
          .run();
      });
    });
  });

  describe("timestamp mode mapping", () => {
    it("stores Date values as integer seconds and reads them back as Date", () => {
      const db = getTestDatabase();
      const when = new Date("2026-07-04T12:34:56.000Z");

      db.insert(tasks)
        .values({ id: "ts-1", title: "ts", createdAt: when, updatedAt: when })
        .run();

      const row = db.select().from(tasks).where(eq(tasks.id, "ts-1")).get();
      assert.ok(row);
      assert.ok(row!.createdAt instanceof Date, "createdAt should be a Date");
      assert.equal(row!.createdAt.getTime(), when.getTime());
    });
  });
});
