/**
 * HatManager Tests
 *
 * Tests for the YAML-based hat preset manager. Covers:
 * - Listing presets (.yml / .yaml)
 * - Loading and parsing preset files
 * - Snake_case → camelCase transformation
 * - MCP server config parsing
 * - Caching behavior
 * - Error paths (missing file, invalid YAML, schema violations)
 */

import { test, describe, beforeEach, afterEach } from "node:test";
import assert from "node:assert";
import * as path from "path";
import * as fs from "fs";
import * as os from "os";
import {
  HatManager,
  PresetLoadError,
  PresetValidationError,
} from "./HatManager";

function writePreset(dir: string, filename: string, body: string): void {
  fs.writeFileSync(path.join(dir, filename), body, "utf-8");
}

describe("HatManager", () => {
  let tempDir: string;
  let presetsDir: string;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "hat-manager-test-"));
    presetsDir = path.join(tempDir, "presets");
    fs.mkdirSync(presetsDir, { recursive: true });
  });

  afterEach(() => {
    fs.rmSync(tempDir, { recursive: true, force: true });
  });

  describe("constructor and getPresetsDir", () => {
    test("resolves presets directory to absolute path", () => {
      const manager = new HatManager(presetsDir);
      assert.ok(
        path.isAbsolute(manager.getPresetsDir()),
        "Presets directory should be resolved to absolute path",
      );
      assert.strictEqual(manager.getPresetsDir(), path.resolve(presetsDir));
    });

    test("resolves a relative path to absolute", () => {
      const manager = new HatManager("./some/relative/path");
      assert.ok(path.isAbsolute(manager.getPresetsDir()));
    });
  });

  describe("listPresets", () => {
    test("returns empty array when presets directory does not exist", () => {
      const manager = new HatManager(path.join(tempDir, "missing"));
      assert.deepStrictEqual(manager.listPresets(), []);
    });

    test("returns empty array when directory exists but is empty", () => {
      const manager = new HatManager(presetsDir);
      assert.deepStrictEqual(manager.listPresets(), []);
    });

    test("lists .yml files without extension", () => {
      writePreset(presetsDir, "planner.yml", "name: P\ndescription: D\n");
      writePreset(presetsDir, "builder.yml", "name: B\ndescription: D\n");

      const manager = new HatManager(presetsDir);
      const presets = manager.listPresets().sort();
      assert.deepStrictEqual(presets, ["builder", "planner"]);
    });

    test("lists .yaml files without extension", () => {
      writePreset(presetsDir, "reviewer.yaml", "name: R\ndescription: D\n");

      const manager = new HatManager(presetsDir);
      assert.deepStrictEqual(manager.listPresets(), ["reviewer"]);
    });

    test("handles a mix of .yml and .yaml files", () => {
      writePreset(presetsDir, "a.yml", "name: A\ndescription: D\n");
      writePreset(presetsDir, "b.yaml", "name: B\ndescription: D\n");

      const manager = new HatManager(presetsDir);
      const presets = manager.listPresets().sort();
      assert.deepStrictEqual(presets, ["a", "b"]);
    });

    test("ignores non-YAML files", () => {
      writePreset(presetsDir, "planner.yml", "name: P\ndescription: D\n");
      writePreset(presetsDir, "README.md", "# readme");
      writePreset(presetsDir, "notes.txt", "notes");
      writePreset(presetsDir, "config.json", '{"a": 1}');

      const manager = new HatManager(presetsDir);
      assert.deepStrictEqual(manager.listPresets(), ["planner"]);
    });
  });

  describe("exists", () => {
    test("returns false for a non-existent preset", () => {
      const manager = new HatManager(presetsDir);
      assert.strictEqual(manager.exists("nope"), false);
    });

    test("returns true for a .yml preset", () => {
      writePreset(presetsDir, "found.yml", "name: F\ndescription: D\n");
      const manager = new HatManager(presetsDir);
      assert.strictEqual(manager.exists("found"), true);
    });

    test("returns true for a .yaml preset", () => {
      writePreset(presetsDir, "found.yaml", "name: F\ndescription: D\n");
      const manager = new HatManager(presetsDir);
      assert.strictEqual(manager.exists("found"), true);
    });
  });

  describe("load", () => {
    test("loads a minimal preset with default empty triggers/publishes", () => {
      writePreset(
        presetsDir,
        "minimal.yml",
        "name: Minimal Hat\ndescription: Does nothing\n",
      );

      const manager = new HatManager(presetsDir);
      const preset = manager.load("minimal");

      assert.strictEqual(preset.filename, "minimal");
      assert.strictEqual(preset.name, "Minimal Hat");
      assert.strictEqual(preset.description, "Does nothing");
      assert.deepStrictEqual(preset.triggersOn, []);
      assert.deepStrictEqual(preset.publishes, []);
      assert.strictEqual(preset.defaultPublishes, undefined);
      assert.strictEqual(preset.mcpServers, undefined);
    });

    test("maps snake_case YAML fields to camelCase TypeScript fields", () => {
      const yaml = [
        "name: Planner",
        "description: Plans things",
        "triggers:",
        "  - task.start",
        "  - plan.retry",
        "publishes:",
        "  - plan.done",
        "default_publishes: plan.done",
        "instructions: |",
        "  Do a thing.",
        "",
      ].join("\n");
      writePreset(presetsDir, "planner.yml", yaml);

      const manager = new HatManager(presetsDir);
      const preset = manager.load("planner");

      assert.strictEqual(preset.name, "Planner");
      assert.deepStrictEqual(preset.triggersOn, ["task.start", "plan.retry"]);
      assert.deepStrictEqual(preset.publishes, ["plan.done"]);
      assert.strictEqual(preset.defaultPublishes, "plan.done");
      assert.match(preset.instructions ?? "", /Do a thing/);
    });

    test("parses mcp_servers with command, args, and env", () => {
      const yaml = [
        "name: MCP Hat",
        "description: Uses MCP",
        "mcp_servers:",
        "  filesystem:",
        "    command: npx",
        "    args:",
        "      - -y",
        "      - '@modelcontextprotocol/server-filesystem'",
        "      - /tmp",
        "    env:",
        "      FOO: bar",
        "      BAZ: qux",
        "",
      ].join("\n");
      writePreset(presetsDir, "mcp.yml", yaml);

      const manager = new HatManager(presetsDir);
      const preset = manager.load("mcp");

      assert.ok(preset.mcpServers, "mcpServers should be set");
      assert.ok(preset.mcpServers.filesystem);
      assert.strictEqual(preset.mcpServers.filesystem.command, "npx");
      assert.deepStrictEqual(preset.mcpServers.filesystem.args, [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "/tmp",
      ]);
      assert.deepStrictEqual(preset.mcpServers.filesystem.env, {
        FOO: "bar",
        BAZ: "qux",
      });
    });

    test("prefers .yml over .yaml when both exist", () => {
      writePreset(presetsDir, "dup.yml", "name: FromYml\ndescription: yml\n");
      writePreset(presetsDir, "dup.yaml", "name: FromYaml\ndescription: yaml\n");

      const manager = new HatManager(presetsDir);
      const preset = manager.load("dup");

      assert.strictEqual(preset.name, "FromYml");
    });

    test("falls back to .yaml when only .yaml exists", () => {
      writePreset(presetsDir, "only.yaml", "name: OnlyYaml\ndescription: D\n");

      const manager = new HatManager(presetsDir);
      const preset = manager.load("only");

      assert.strictEqual(preset.name, "OnlyYaml");
    });

    test("caches loaded presets across calls", () => {
      writePreset(presetsDir, "cached.yml", "name: V1\ndescription: D\n");

      const manager = new HatManager(presetsDir);
      const first = manager.load("cached");
      assert.strictEqual(first.name, "V1");

      // Modify file on disk; cached load should still return V1.
      writePreset(presetsDir, "cached.yml", "name: V2\ndescription: D\n");
      const cached = manager.load("cached");
      assert.strictEqual(cached.name, "V1", "Should return cached value");

      // useCache: false bypasses cache and reads updated file.
      const fresh = manager.load("cached", { useCache: false });
      assert.strictEqual(fresh.name, "V2", "Should bypass cache when disabled");
    });

    test("clearCache forces a reload on next call", () => {
      writePreset(presetsDir, "cached.yml", "name: V1\ndescription: D\n");

      const manager = new HatManager(presetsDir);
      manager.load("cached");

      writePreset(presetsDir, "cached.yml", "name: V2\ndescription: D\n");
      manager.clearCache();

      const reloaded = manager.load("cached");
      assert.strictEqual(reloaded.name, "V2");
    });

    test("throws PresetLoadError when the preset file does not exist", () => {
      const manager = new HatManager(presetsDir);
      assert.throws(
        () => manager.load("missing"),
        (err: unknown) => {
          assert.ok(err instanceof PresetLoadError, "Should be PresetLoadError");
          assert.strictEqual((err as PresetLoadError).filename, "missing");
          return true;
        },
      );
    });

    test("throws PresetLoadError when YAML is syntactically invalid", () => {
      writePreset(presetsDir, "broken.yml", "name: oops\n  :::invalid yaml:::");

      const manager = new HatManager(presetsDir);
      assert.throws(
        () => manager.load("broken"),
        (err: unknown) => err instanceof PresetLoadError,
      );
    });

    test("throws PresetValidationError when required fields are missing", () => {
      // Missing `name` and `description` — both required.
      writePreset(presetsDir, "bad.yml", "triggers: []\npublishes: []\n");

      const manager = new HatManager(presetsDir);
      assert.throws(
        () => manager.load("bad"),
        (err: unknown) => {
          assert.ok(
            err instanceof PresetValidationError,
            "Should be PresetValidationError",
          );
          assert.strictEqual((err as PresetValidationError).filename, "bad");
          assert.ok(
            (err as PresetValidationError).issues.length > 0,
            "Should include zod issues",
          );
          return true;
        },
      );
    });

    test("throws PresetValidationError when triggers has wrong type", () => {
      writePreset(
        presetsDir,
        "typeerr.yml",
        "name: T\ndescription: D\ntriggers: not-an-array\n",
      );

      const manager = new HatManager(presetsDir);
      assert.throws(
        () => manager.load("typeerr"),
        (err: unknown) => err instanceof PresetValidationError,
      );
    });
  });

  describe("loadAll", () => {
    test("returns empty array when no presets exist", () => {
      const manager = new HatManager(presetsDir);
      assert.deepStrictEqual(manager.loadAll(), []);
    });

    test("loads every preset in the directory", () => {
      writePreset(presetsDir, "a.yml", "name: A\ndescription: d\n");
      writePreset(presetsDir, "b.yml", "name: B\ndescription: d\n");
      writePreset(presetsDir, "c.yaml", "name: C\ndescription: d\n");

      const manager = new HatManager(presetsDir);
      const presets = manager.loadAll();

      assert.strictEqual(presets.length, 3);
      const names = presets.map((p) => p.name).sort();
      assert.deepStrictEqual(names, ["A", "B", "C"]);
    });

    test("propagates a validation error from any single bad preset", () => {
      writePreset(presetsDir, "good.yml", "name: G\ndescription: d\n");
      writePreset(presetsDir, "bad.yml", "triggers: []\n"); // missing name + description

      const manager = new HatManager(presetsDir);
      assert.throws(() => manager.loadAll());
    });
  });
});
