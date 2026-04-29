/**
 * Database Connection Module Tests
 *
 * Covers:
 * - getDatabase() singleton behavior (same instance on repeat)
 * - getDatabase() respects explicit path, RALPH_DB_PATH env var, and default HOME fallback
 * - getDatabase() creates missing parent directories (except for :memory:)
 * - getDatabase() applies WAL and foreign_keys pragmas to on-disk databases
 * - getSqliteConnection() returns null before init, raw connection after
 * - closeDatabase() resets module state (next getDatabase creates fresh instance)
 * - closeDatabase() is safe to call when nothing is open
 * - initializeDatabase() creates all required tables (idempotent)
 * - initializeDatabase() adds new columns to a legacy tasks table without errors
 * - initializeDatabase() throws when module sqlite is null even if a drizzle db is passed
 */

import { describe, it, beforeEach, afterEach } from "node:test";
import assert from "node:assert/strict";
import fs from "fs";
import os from "os";
import path from "path";
import Database from "better-sqlite3";
import { drizzle } from "drizzle-orm/better-sqlite3";
import {
  getDatabase,
  closeDatabase,
  getSqliteConnection,
  initializeDatabase,
} from "./connection";
import * as schema from "./schema";

// Capture env / HOME to restore between tests
const ORIGINAL_ENV = { ...process.env };

function withCleanEnv(): void {
  // Reset to a known state each test
  process.env = { ...ORIGINAL_ENV };
  delete process.env.RALPH_DB_PATH;
}

function makeTmpDir(prefix = "ralph-conn-test-"): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

describe("db/connection", () => {
  let tmpDirs: string[] = [];

  beforeEach(() => {
    withCleanEnv();
    closeDatabase();
    tmpDirs = [];
  });

  afterEach(() => {
    closeDatabase();
    process.env = { ...ORIGINAL_ENV };
    for (const dir of tmpDirs) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        // best-effort cleanup
      }
    }
  });

  describe("getDatabase", () => {
    it("returns the same drizzle instance on repeated calls (singleton)", () => {
      const first = getDatabase(":memory:");
      const second = getDatabase(":memory:");
      // Module-level singleton: second call ignores path argument and returns the first db
      assert.strictEqual(first, second);
    });

    it("opens an explicit database path", () => {
      const dir = makeTmpDir();
      tmpDirs.push(dir);
      const dbPath = path.join(dir, "explicit.db");

      getDatabase(dbPath);

      assert.ok(fs.existsSync(dbPath), "database file should exist on disk");
    });

    it("uses RALPH_DB_PATH when no path argument is provided", () => {
      const dir = makeTmpDir();
      tmpDirs.push(dir);
      const envPath = path.join(dir, "from-env.db");
      process.env.RALPH_DB_PATH = envPath;

      getDatabase();

      assert.ok(fs.existsSync(envPath), "RALPH_DB_PATH target should exist");
    });

    it("falls back to HOME-based default when no path and no env is set", () => {
      const dir = makeTmpDir();
      tmpDirs.push(dir);
      process.env.HOME = dir;
      delete process.env.USERPROFILE;

      getDatabase();

      const expected = path.join(dir, ".ralph", "web", "ralph.db");
      assert.ok(fs.existsSync(expected), `default path should exist at ${expected}`);
    });

    it("creates missing parent directories for the database file", () => {
      const dir = makeTmpDir();
      tmpDirs.push(dir);
      const nested = path.join(dir, "one", "two", "three", "nested.db");

      assert.ok(!fs.existsSync(path.dirname(nested)), "parent should not exist yet");

      getDatabase(nested);

      assert.ok(fs.existsSync(nested), "database file should be created");
      assert.ok(
        fs.statSync(path.dirname(nested)).isDirectory(),
        "parent directory should be created",
      );
    });

    it("does not create a directory for :memory: databases", () => {
      // Should not throw or try to mkdir for the special :memory: path
      const db = getDatabase(":memory:");
      assert.ok(db, "in-memory database should be returned");
      // Sanity: no stray ":memory:" directory in cwd
      assert.ok(!fs.existsSync(path.join(process.cwd(), ":memory:")));
    });

    it("applies WAL journal mode and foreign_keys pragmas on file-backed databases", () => {
      const dir = makeTmpDir();
      tmpDirs.push(dir);
      const dbPath = path.join(dir, "pragmas.db");

      getDatabase(dbPath);

      const sqlite = getSqliteConnection();
      assert.ok(sqlite, "raw sqlite connection should be available after init");

      const journalMode = sqlite!.pragma("journal_mode", { simple: true });
      assert.equal(journalMode, "wal");

      const foreignKeys = sqlite!.pragma("foreign_keys", { simple: true });
      // Pragma returns 1 when enabled
      assert.equal(foreignKeys, 1);
    });
  });

  describe("getSqliteConnection", () => {
    it("returns null before getDatabase() is called", () => {
      assert.equal(getSqliteConnection(), null);
    });

    it("returns the raw sqlite handle after getDatabase() is called", () => {
      getDatabase(":memory:");
      const sqlite = getSqliteConnection();
      assert.ok(sqlite, "connection should be a non-null object");
      // Sanity-check it's a working sqlite connection
      const row = sqlite!.prepare("SELECT 1 AS one").get() as { one: number };
      assert.equal(row.one, 1);
    });
  });

  describe("closeDatabase", () => {
    it("is a no-op when no database is open", () => {
      // Should not throw
      closeDatabase();
      assert.equal(getSqliteConnection(), null);
    });

    it("resets module state so getDatabase() produces a fresh instance", () => {
      const first = getDatabase(":memory:");
      closeDatabase();
      assert.equal(getSqliteConnection(), null, "connection should be null after close");

      const second = getDatabase(":memory:");
      assert.notStrictEqual(first, second, "second getDatabase after close should return a new instance");
    });

    it("actually closes the underlying sqlite handle", () => {
      getDatabase(":memory:");
      const sqlite = getSqliteConnection();
      assert.ok(sqlite);

      closeDatabase();

      // After close, attempting to use the old handle should throw
      assert.throws(() => {
        sqlite!.prepare("SELECT 1").get();
      });
    });
  });

  describe("initializeDatabase", () => {
    function tableExists(name: string): boolean {
      const sqlite = getSqliteConnection();
      assert.ok(sqlite, "sqlite connection must be initialized");
      const row = sqlite!
        .prepare(
          "SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .get(name);
      return row !== undefined;
    }

    function columnNames(table: string): string[] {
      const sqlite = getSqliteConnection();
      assert.ok(sqlite);
      const rows = sqlite!.prepare(`PRAGMA table_info(${table})`).all() as Array<{ name: string }>;
      return rows.map((r) => r.name);
    }

    it("creates all required tables", () => {
      getDatabase(":memory:");
      initializeDatabase();

      assert.ok(tableExists("tasks"));
      assert.ok(tableExists("queued_tasks"));
      assert.ok(tableExists("task_logs"));
      assert.ok(tableExists("settings"));
      assert.ok(tableExists("collections"));
    });

    it("creates the idx_task_logs_task_id index", () => {
      getDatabase(":memory:");
      initializeDatabase();

      const sqlite = getSqliteConnection();
      const row = sqlite!
        .prepare(
          "SELECT name FROM sqlite_master WHERE type = 'index' AND name = ?",
        )
        .get("idx_task_logs_task_id");
      assert.ok(row, "index idx_task_logs_task_id should exist");
    });

    it("is idempotent (safe to call multiple times)", () => {
      getDatabase(":memory:");
      initializeDatabase();
      // Should not throw on a second call
      initializeDatabase();

      const columns = columnNames("tasks");
      // Duplicates would indicate broken idempotency; each column must appear exactly once
      const unique = new Set(columns);
      assert.equal(unique.size, columns.length, "column names should be unique");
    });

    it("adds new execution-tracking columns to a legacy tasks table", () => {
      getDatabase(":memory:");
      const sqlite = getSqliteConnection();
      assert.ok(sqlite);

      // Pre-create a legacy tasks table with only the original columns
      sqlite!.exec(`
        CREATE TABLE tasks (
          id TEXT PRIMARY KEY,
          title TEXT NOT NULL,
          status TEXT NOT NULL DEFAULT 'open',
          priority INTEGER NOT NULL DEFAULT 2,
          blocked_by TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL
        )
      `);

      // Sanity: legacy table has no new columns
      const beforeColumns = columnNames("tasks");
      assert.ok(!beforeColumns.includes("execution_summary"));
      assert.ok(!beforeColumns.includes("preset"));
      assert.ok(!beforeColumns.includes("loop_id"));

      initializeDatabase();

      const afterColumns = columnNames("tasks");
      // Every column that addColumnIfNotExists attempts to add must now be present
      for (const col of [
        "queued_task_id",
        "started_at",
        "completed_at",
        "error_message",
        "execution_summary",
        "exit_code",
        "duration_ms",
        "archived_at",
        "merge_loop_prompt",
        "preset",
        "current_iteration",
        "max_iterations",
        "loop_id",
      ]) {
        assert.ok(
          afterColumns.includes(col),
          `tasks table should contain column ${col} after initializeDatabase`,
        );
      }
    });

    it("throws when called with an external drizzle db but no module connection", () => {
      // Build a drizzle instance without going through getDatabase() so the
      // module-level sqlite stays null. initializeDatabase should then refuse.
      const external = new Database(":memory:");
      try {
        const externalDb = drizzle(external, { schema });
        closeDatabase(); // ensure module sqlite is null

        assert.throws(
          () => initializeDatabase(externalDb),
          /Database not initialized/,
        );
      } finally {
        external.close();
      }
    });
  });
});
