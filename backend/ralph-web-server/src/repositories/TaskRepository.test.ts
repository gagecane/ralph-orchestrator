/**
 * TaskRepository Tests
 *
 * Tests for the task CRUD operations. Covers:
 * - create auto-populates createdAt / updatedAt
 * - findById returns undefined when missing
 * - findAll filters by status and archival state
 * - findReady returns open tasks whose blockers are closed or archived
 * - update partial fields and updatedAt refresh
 * - close / archive / unarchive helpers
 * - delete / deleteAll and return values
 */

import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { TaskRepository } from "./TaskRepository";
import {
  initializeTestDatabase,
  getTestDatabase,
  closeTestDatabase,
} from "../db/testUtils";
import type { NewTask } from "../db/schema";

type CreateInput = Omit<NewTask, "createdAt" | "updatedAt">;

function task(overrides: Partial<CreateInput> = {}): CreateInput {
  return {
    id: overrides.id ?? "t-default",
    title: overrides.title ?? "Default",
    status: overrides.status ?? "open",
    priority: overrides.priority ?? 2,
    blockedBy: overrides.blockedBy ?? null,
    archivedAt: overrides.archivedAt ?? null,
    ...overrides,
  };
}

describe("TaskRepository", () => {
  let repo: TaskRepository;

  beforeEach(() => {
    initializeTestDatabase();
    repo = new TaskRepository(getTestDatabase());
  });

  afterEach(() => {
    closeTestDatabase();
  });

  describe("create", () => {
    it("stores the task and populates createdAt / updatedAt", () => {
      // Drizzle's timestamp mode stores seconds-precision, so use
      // second-granularity bounds to avoid ms truncation flakes.
      const beforeSec = Math.floor(Date.now() / 1000);
      const created = repo.create(task({ id: "t1", title: "Hello" }));
      const afterSec = Math.floor(Date.now() / 1000);

      assert.equal(created.id, "t1");
      assert.equal(created.title, "Hello");
      assert.equal(created.status, "open");
      assert.ok(created.createdAt instanceof Date);
      assert.ok(created.updatedAt instanceof Date);
      const createdSec = Math.floor(created.createdAt.getTime() / 1000);
      assert.ok(createdSec >= beforeSec);
      assert.ok(createdSec <= afterSec);
    });
  });

  describe("findById", () => {
    it("returns the task when it exists", () => {
      repo.create(task({ id: "t1" }));
      const found = repo.findById("t1");
      assert.equal(found?.id, "t1");
    });

    it("returns undefined for missing ids", () => {
      assert.equal(repo.findById("nope"), undefined);
    });
  });

  describe("findAll", () => {
    beforeEach(() => {
      repo.create(task({ id: "open-1", status: "open" }));
      repo.create(task({ id: "open-2", status: "open" }));
      repo.create(task({ id: "closed-1", status: "closed" }));
      repo.create(task({ id: "archived-1", status: "open" }));
      repo.archive("archived-1");
    });

    it("returns only non-archived tasks by default", () => {
      const all = repo.findAll();
      const ids = all.map((t) => t.id).sort();
      assert.deepEqual(ids, ["closed-1", "open-1", "open-2"]);
    });

    it("filters by status (excluding archived by default)", () => {
      const openIds = repo.findAll("open").map((t) => t.id).sort();
      assert.deepEqual(openIds, ["open-1", "open-2"]);

      const closedIds = repo.findAll("closed").map((t) => t.id).sort();
      assert.deepEqual(closedIds, ["closed-1"]);
    });

    it("includes archived tasks when includeArchived is true", () => {
      const all = repo.findAll(undefined, true);
      const ids = all.map((t) => t.id).sort();
      assert.deepEqual(ids, ["archived-1", "closed-1", "open-1", "open-2"]);

      const openWithArchived = repo
        .findAll("open", true)
        .map((t) => t.id)
        .sort();
      assert.deepEqual(openWithArchived, ["archived-1", "open-1", "open-2"]);
    });
  });

  describe("findReady", () => {
    it("returns open tasks with no blocker", () => {
      repo.create(task({ id: "a" }));
      repo.create(task({ id: "b" }));
      const ready = repo.findReady().map((t) => t.id).sort();
      assert.deepEqual(ready, ["a", "b"]);
    });

    it("excludes open tasks whose blocker is still open", () => {
      repo.create(task({ id: "blocker" }));
      repo.create(task({ id: "blocked", blockedBy: "blocker" }));

      const ready = repo.findReady().map((t) => t.id);
      assert.deepEqual(ready.sort(), ["blocker"]);
    });

    it("includes tasks whose blocker is closed", () => {
      repo.create(task({ id: "blocker" }));
      repo.create(task({ id: "blocked", blockedBy: "blocker" }));
      repo.close("blocker");

      const ready = repo.findReady().map((t) => t.id).sort();
      assert.deepEqual(ready, ["blocked"]);
    });

    it("includes tasks whose blocker is archived (even if still open)", () => {
      repo.create(task({ id: "blocker" }));
      repo.create(task({ id: "blocked", blockedBy: "blocker" }));
      repo.archive("blocker");

      const ready = repo.findReady().map((t) => t.id).sort();
      assert.deepEqual(ready, ["blocked"]);
    });

    it("never returns closed tasks", () => {
      repo.create(task({ id: "done" }));
      repo.close("done");
      const ready = repo.findReady().map((t) => t.id);
      assert.ok(!ready.includes("done"));
    });
  });

  describe("update", () => {
    it("returns undefined for a missing task", () => {
      assert.equal(repo.update("missing", { title: "x" }), undefined);
    });

    it("updates fields and refreshes updatedAt", async () => {
      const created = repo.create(task({ id: "t1", title: "Original" }));
      // Force a measurable gap so updatedAt can strictly advance.
      await new Promise((resolve) => setTimeout(resolve, 5));

      const updated = repo.update("t1", { title: "Renamed", priority: 1 });
      assert.ok(updated);
      assert.equal(updated!.title, "Renamed");
      assert.equal(updated!.priority, 1);
      // Compare at second granularity since SQLite truncates to seconds.
      const createdAtSec = Math.floor(created.createdAt.getTime() / 1000);
      const updatedCreatedSec = Math.floor(updated!.createdAt.getTime() / 1000);
      assert.equal(updatedCreatedSec, createdAtSec);
      const createdUpdatedSec = Math.floor(created.updatedAt.getTime() / 1000);
      const updatedUpdatedSec = Math.floor(updated!.updatedAt.getTime() / 1000);
      assert.ok(updatedUpdatedSec >= createdUpdatedSec);
    });
  });

  describe("close", () => {
    it("sets the status to closed", () => {
      repo.create(task({ id: "t1" }));
      const updated = repo.close("t1");
      assert.equal(updated?.status, "closed");
    });

    it("returns undefined for a missing task", () => {
      assert.equal(repo.close("missing"), undefined);
    });
  });

  describe("archive / unarchive", () => {
    it("archive sets archivedAt and unarchive clears it", () => {
      repo.create(task({ id: "t1" }));
      const archived = repo.archive("t1");
      assert.ok(archived);
      assert.ok(archived!.archivedAt instanceof Date);

      const unarchived = repo.unarchive("t1");
      assert.ok(unarchived);
      assert.equal(unarchived!.archivedAt, null);
    });

    it("both return undefined for missing tasks", () => {
      assert.equal(repo.archive("missing"), undefined);
      assert.equal(repo.unarchive("missing"), undefined);
    });
  });

  describe("delete", () => {
    it("returns true and removes the task when it exists", () => {
      repo.create(task({ id: "t1" }));
      assert.equal(repo.delete("t1"), true);
      assert.equal(repo.findById("t1"), undefined);
    });

    it("returns false when the task does not exist", () => {
      assert.equal(repo.delete("missing"), false);
    });
  });

  describe("deleteAll", () => {
    it("deletes every task and returns the count", () => {
      repo.create(task({ id: "t1" }));
      repo.create(task({ id: "t2" }));
      repo.create(task({ id: "t3" }));
      const count = repo.deleteAll();
      assert.equal(count, 3);
      assert.deepEqual(repo.findAll(undefined, true), []);
    });

    it("returns 0 when there are no tasks", () => {
      assert.equal(repo.deleteAll(), 0);
    });
  });
});
