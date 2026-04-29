/**
 * TaskLogRepository Tests
 *
 * Tests for persistent task log storage. Covers:
 * - append returns the inserted rowid and stores the fields correctly
 * - append accepts both Date and numeric timestamps
 * - listByTaskId returns rows ordered by id ascending
 * - listByTaskId filters by taskId and applies afterId / limit options
 * - deleteAll removes every log and returns the deleted count
 */

import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { TaskLogRepository } from "./TaskLogRepository";
import {
  initializeTestDatabase,
  getTestDatabase,
  closeTestDatabase,
} from "../db/testUtils";
import type { LogEntry } from "../runner/LogStream";

function entry(
  line: string,
  source: "stdout" | "stderr" = "stdout",
  timestamp: Date = new Date(),
): LogEntry {
  return { line, source, timestamp };
}

describe("TaskLogRepository", () => {
  let repo: TaskLogRepository;

  beforeEach(() => {
    initializeTestDatabase();
    repo = new TaskLogRepository(getTestDatabase());
  });

  afterEach(() => {
    closeTestDatabase();
  });

  describe("append", () => {
    it("returns an increasing rowid and persists the log fields", () => {
      const ts = new Date("2026-04-29T00:00:00.000Z");
      const id1 = repo.append("task-1", entry("hello", "stdout", ts));
      const id2 = repo.append("task-1", entry("world", "stderr", ts));

      assert.ok(id1 > 0);
      assert.ok(id2 > id1);

      const rows = repo.listByTaskId("task-1");
      assert.equal(rows.length, 2);
      assert.equal(rows[0].line, "hello");
      assert.equal(rows[0].source, "stdout");
      assert.ok(rows[0].timestamp instanceof Date);
      assert.equal(rows[0].timestamp.getTime(), ts.getTime());
      assert.equal(rows[1].line, "world");
      assert.equal(rows[1].source, "stderr");
    });

    it("accepts a numeric timestamp and coerces it to a Date", () => {
      const ms = Date.UTC(2026, 0, 1, 0, 0, 0);
      // The repo's public signature types timestamp as Date, but the
      // implementation explicitly handles numeric values as well. Cast to
      // document that runtime contract.
      repo.append(
        "task-numeric",
        { line: "x", source: "stdout", timestamp: ms as unknown as Date },
      );

      const rows = repo.listByTaskId("task-numeric");
      assert.equal(rows.length, 1);
      assert.ok(rows[0].timestamp instanceof Date);
      assert.equal(rows[0].timestamp.getTime(), ms);
    });
  });

  describe("listByTaskId", () => {
    it("returns logs only for the matching task, in id ascending order", () => {
      const idA1 = repo.append("a", entry("a-1"));
      const idB1 = repo.append("b", entry("b-1"));
      const idA2 = repo.append("a", entry("a-2"));

      const rowsA = repo.listByTaskId("a");
      assert.equal(rowsA.length, 2);
      assert.equal(rowsA[0].line, "a-1");
      assert.equal(rowsA[1].line, "a-2");
      assert.ok(rowsA[0].id < rowsA[1].id);
      assert.equal(rowsA[0].id, idA1);
      assert.equal(rowsA[1].id, idA2);

      const rowsB = repo.listByTaskId("b");
      assert.equal(rowsB.length, 1);
      assert.equal(rowsB[0].id, idB1);
    });

    it("returns an empty array when the task has no logs", () => {
      assert.deepEqual(repo.listByTaskId("nothing"), []);
    });

    it("filters with afterId (strictly greater)", () => {
      const id1 = repo.append("t", entry("l1"));
      repo.append("t", entry("l2"));
      repo.append("t", entry("l3"));

      const rows = repo.listByTaskId("t", { afterId: id1 });
      assert.equal(rows.length, 2);
      assert.equal(rows[0].line, "l2");
      assert.equal(rows[1].line, "l3");
      for (const r of rows) {
        assert.ok(r.id > id1);
      }
    });

    it("applies limit", () => {
      for (let i = 0; i < 5; i++) {
        repo.append("t", entry(`line-${i}`));
      }

      const rows = repo.listByTaskId("t", { limit: 2 });
      assert.equal(rows.length, 2);
      assert.equal(rows[0].line, "line-0");
      assert.equal(rows[1].line, "line-1");
    });

    it("combines afterId and limit", () => {
      const ids: number[] = [];
      for (let i = 0; i < 5; i++) {
        ids.push(repo.append("t", entry(`line-${i}`)));
      }

      const rows = repo.listByTaskId("t", { afterId: ids[1], limit: 2 });
      assert.equal(rows.length, 2);
      assert.equal(rows[0].line, "line-2");
      assert.equal(rows[1].line, "line-3");
    });
  });

  describe("deleteAll", () => {
    it("removes every row across tasks and returns the count", () => {
      repo.append("a", entry("x"));
      repo.append("b", entry("y"));
      repo.append("a", entry("z"));

      const deleted = repo.deleteAll();
      assert.equal(deleted, 3);
      assert.deepEqual(repo.listByTaskId("a"), []);
      assert.deepEqual(repo.listByTaskId("b"), []);
    });

    it("returns 0 when there is nothing to delete", () => {
      assert.equal(repo.deleteAll(), 0);
    });
  });
});
