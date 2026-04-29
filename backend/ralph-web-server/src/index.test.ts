/**
 * Top-level Barrel Export Tests
 *
 * Tests for src/index.ts — the public surface of @ralph-web/server.
 * Verifies every advertised symbol is exported and identity-equal to its
 * source module. Broken barrel exports here silently break downstream
 * consumers at runtime, so pinning them with identity checks is worth it.
 */

import { describe, test } from "node:test";
import assert from "node:assert/strict";
import * as pkg from "./index";

import * as dbConnection from "./db/connection";
import * as repositories from "./repositories";
import * as queue from "./queue";
import * as runner from "./runner";
import * as api from "./api";

describe("src/index.ts — database re-exports", () => {
  test("re-exports database connection helpers", () => {
    assert.strictEqual(pkg.getDatabase, dbConnection.getDatabase);
    assert.strictEqual(pkg.initializeDatabase, dbConnection.initializeDatabase);
    assert.strictEqual(pkg.closeDatabase, dbConnection.closeDatabase);
    assert.strictEqual(
      pkg.getSqliteConnection,
      dbConnection.getSqliteConnection
    );
    assert.strictEqual(pkg.schema, dbConnection.schema);
  });
});

describe("src/index.ts — repository re-exports", () => {
  test("re-exports all repository classes", () => {
    assert.strictEqual(pkg.TaskRepository, repositories.TaskRepository);
    assert.strictEqual(pkg.SettingsRepository, repositories.SettingsRepository);
    assert.strictEqual(pkg.TaskLogRepository, repositories.TaskLogRepository);
  });
});

describe("src/index.ts — queue re-exports", () => {
  test("re-exports state helpers", () => {
    assert.strictEqual(pkg.TaskState, queue.TaskState);
    assert.strictEqual(pkg.isTerminalState, queue.isTerminalState);
    assert.strictEqual(pkg.isValidTransition, queue.isValidTransition);
    assert.strictEqual(
      pkg.getAllowedTransitions,
      queue.getAllowedTransitions
    );
  });

  test("re-exports queue services", () => {
    assert.strictEqual(pkg.TaskQueueService, queue.TaskQueueService);
    assert.strictEqual(pkg.EventBus, queue.EventBus);
    assert.strictEqual(pkg.Dispatcher, queue.Dispatcher);
  });
});

describe("src/index.ts — runner re-exports", () => {
  test("re-exports runner state helpers", () => {
    assert.strictEqual(pkg.RunnerState, runner.RunnerState);
    assert.strictEqual(
      pkg.isTerminalRunnerState,
      runner.isTerminalRunnerState
    );
    assert.strictEqual(
      pkg.isValidRunnerTransition,
      runner.isValidRunnerTransition
    );
    assert.strictEqual(
      pkg.getAllowedRunnerTransitions,
      runner.getAllowedRunnerTransitions
    );
  });

  test("re-exports runner classes", () => {
    assert.strictEqual(pkg.LogStream, runner.LogStream);
    assert.strictEqual(pkg.PromptWriter, runner.PromptWriter);
    assert.strictEqual(pkg.RalphRunner, runner.RalphRunner);
  });
});

describe("src/index.ts — API re-exports", () => {
  test("re-exports server builders and tRPC surface", () => {
    assert.strictEqual(pkg.createServer, api.createServer);
    assert.strictEqual(pkg.startServer, api.startServer);
    assert.strictEqual(pkg.appRouter, api.appRouter);
    assert.strictEqual(pkg.taskRouter, api.taskRouter);
    assert.strictEqual(pkg.createContext, api.createContext);
  });
});

describe("src/index.ts — smoke", () => {
  test("all exported functions are callable", () => {
    const fnNames = [
      "getDatabase",
      "initializeDatabase",
      "closeDatabase",
      "getSqliteConnection",
      "isTerminalState",
      "isValidTransition",
      "getAllowedTransitions",
      "isTerminalRunnerState",
      "isValidRunnerTransition",
      "getAllowedRunnerTransitions",
      "createServer",
      "startServer",
      "createContext",
    ] as const;

    for (const name of fnNames) {
      assert.equal(
        typeof (pkg as any)[name],
        "function",
        `pkg.${name} should be a function`
      );
    }
  });

  test("all exported classes are constructors", () => {
    const classNames = [
      "TaskRepository",
      "SettingsRepository",
      "TaskLogRepository",
      "TaskQueueService",
      "EventBus",
      "Dispatcher",
      "LogStream",
      "PromptWriter",
      "RalphRunner",
    ] as const;

    for (const name of classNames) {
      const value = (pkg as any)[name];
      assert.equal(
        typeof value,
        "function",
        `pkg.${name} should be a class/function`
      );
      // Constructors have prototype objects
      assert.ok(value.prototype, `pkg.${name} should have a prototype`);
    }
  });

  test("schema namespace contains drizzle table definitions", () => {
    // schema is the drizzle schema module re-exported as a namespace.
    // We don't pin specific tables (they evolve), just that it's an object
    // with at least one key — anything less means the re-export is broken.
    assert.equal(typeof pkg.schema, "object");
    assert.ok(pkg.schema !== null);
    assert.ok(
      Object.keys(pkg.schema).length > 0,
      "schema should re-export at least one table"
    );
  });
});
