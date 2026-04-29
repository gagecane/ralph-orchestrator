/**
 * SettingsRepository Tests
 *
 * Tests for the key-value settings store. Covers:
 * - get/set round-trip with JSON serialization
 * - get returns undefined for missing keys
 * - get falls back to the raw string when the stored value is not valid JSON
 * - set performs upsert (insert then update)
 * - getAll / getAllAsObject / has / delete / deleteAll
 */

import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import { SettingsRepository } from "./SettingsRepository";
import {
  initializeTestDatabase,
  getTestDatabase,
  closeTestDatabase,
} from "../db/testUtils";
import { settings } from "../db/schema";

describe("SettingsRepository", () => {
  let repo: SettingsRepository;

  beforeEach(() => {
    initializeTestDatabase();
    repo = new SettingsRepository(getTestDatabase());
  });

  afterEach(() => {
    closeTestDatabase();
  });

  describe("get", () => {
    it("returns undefined for a missing key", () => {
      assert.equal(repo.get("missing"), undefined);
    });

    it("returns the JSON-parsed value when the key exists", () => {
      repo.set("count", 42);
      assert.equal(repo.get<number>("count"), 42);

      repo.set("config", { a: 1, b: ["x", "y"] });
      assert.deepEqual(
        repo.get<{ a: number; b: string[] }>("config"),
        { a: 1, b: ["x", "y"] },
      );
    });

    it("returns the raw string when the stored value is not valid JSON", () => {
      // Insert a row with non-JSON value directly to simulate a legacy write
      const db = getTestDatabase();
      db.insert(settings)
        .values({
          key: "legacy",
          value: "plain string, not JSON",
          updatedAt: new Date(),
        })
        .run();

      assert.equal(repo.get<string>("legacy"), "plain string, not JSON");
    });
  });

  describe("getWithMetadata", () => {
    it("returns the row with metadata for an existing key", () => {
      // Drizzle's timestamp mode stores seconds-precision epoch in SQLite,
      // so compare at second granularity to avoid ms-level truncation issues.
      const beforeSec = Math.floor(Date.now() / 1000);
      repo.set("k", "v");
      const row = repo.getWithMetadata("k");
      assert.ok(row);
      assert.equal(row!.key, "k");
      // value is stored JSON-serialized
      assert.equal(row!.value, '"v"');
      assert.ok(row!.updatedAt instanceof Date);
      assert.ok(Math.floor(row!.updatedAt.getTime() / 1000) >= beforeSec);
    });

    it("returns undefined for a missing key", () => {
      assert.equal(repo.getWithMetadata("missing"), undefined);
    });
  });

  describe("set", () => {
    it("inserts when the key does not exist", () => {
      const row = repo.set("new-key", { hello: "world" });
      assert.equal(row.key, "new-key");
      assert.deepEqual(repo.get("new-key"), { hello: "world" });
    });

    it("updates when the key already exists", () => {
      repo.set("k", 1);
      const first = repo.getWithMetadata("k");
      assert.ok(first);

      // Ensure a measurable time difference for updatedAt
      repo.set("k", 2);
      const second = repo.getWithMetadata("k");
      assert.ok(second);

      assert.equal(repo.get<number>("k"), 2);
      assert.ok(second!.updatedAt.getTime() >= first!.updatedAt.getTime());
    });

    it("serializes null, booleans, and arrays", () => {
      repo.set("nil", null);
      repo.set("flag", true);
      repo.set("list", [1, 2, 3]);

      assert.equal(repo.get("nil"), null);
      assert.equal(repo.get<boolean>("flag"), true);
      assert.deepEqual(repo.get<number[]>("list"), [1, 2, 3]);
    });
  });

  describe("delete", () => {
    it("returns true when a setting is removed", () => {
      repo.set("k", "v");
      assert.equal(repo.delete("k"), true);
      assert.equal(repo.has("k"), false);
    });

    it("returns false when the key does not exist", () => {
      assert.equal(repo.delete("missing"), false);
    });
  });

  describe("getAll", () => {
    it("returns all rows with raw JSON-encoded values", () => {
      repo.set("a", 1);
      repo.set("b", "two");

      const rows = repo.getAll();
      assert.equal(rows.length, 2);
      const byKey = Object.fromEntries(rows.map((r) => [r.key, r.value]));
      assert.equal(byKey["a"], "1");
      assert.equal(byKey["b"], '"two"');
    });

    it("returns an empty array when nothing has been set", () => {
      assert.deepEqual(repo.getAll(), []);
    });
  });

  describe("getAllAsObject", () => {
    it("returns all settings as a decoded key-value map", () => {
      repo.set("a", 1);
      repo.set("b", { nested: true });

      const obj = repo.getAllAsObject();
      assert.deepEqual(obj, { a: 1, b: { nested: true } });
    });

    it("falls back to raw strings for non-JSON values", () => {
      const db = getTestDatabase();
      db.insert(settings)
        .values({ key: "legacy", value: "raw", updatedAt: new Date() })
        .run();
      repo.set("modern", "value");

      const obj = repo.getAllAsObject();
      assert.equal(obj["legacy"], "raw");
      assert.equal(obj["modern"], "value");
    });
  });

  describe("has", () => {
    it("reflects whether a key exists", () => {
      assert.equal(repo.has("k"), false);
      repo.set("k", 0);
      assert.equal(repo.has("k"), true);
      repo.delete("k");
      assert.equal(repo.has("k"), false);
    });
  });

  describe("deleteAll", () => {
    it("removes every row and returns the count", () => {
      repo.set("a", 1);
      repo.set("b", 2);
      repo.set("c", 3);

      const count = repo.deleteAll();
      assert.equal(count, 3);
      assert.deepEqual(repo.getAll(), []);
    });

    it("returns 0 when there is nothing to delete", () => {
      assert.equal(repo.deleteAll(), 0);
    });
  });
});
