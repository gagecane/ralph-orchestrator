/**
 * SettingsService Tests
 *
 * Tests the typed, domain-specific access layer on top of SettingsRepository.
 * Covers:
 * - Persona CRUD (get/set/delete/list/has) and default/fallback behavior
 * - Hat CRUD and default/fallback behavior
 * - Event-trigger lookup (findHatsByTrigger)
 * - Raw settings pass-through (getRaw/setRaw/deleteRaw)
 * - Reset-to-default semantics when deleting the active persona/hat
 */

import { test, describe, beforeEach, afterEach } from "node:test";
import assert from "node:assert";
import {
  SettingsService,
  SettingKeys,
  DEFAULT_PERSONA,
  DEFAULT_HAT,
  FALLBACK_PERSONA_DEFINITION,
  FALLBACK_HAT_DEFINITION,
  PersonaDefinition,
  HatDefinition,
} from "./SettingsService";
import { SettingsRepository } from "../repositories/SettingsRepository";
import {
  initializeTestDatabase,
  getTestDatabase,
  closeTestDatabase,
} from "../db/testUtils";

function makeService(): { service: SettingsService; repo: SettingsRepository } {
  const db = getTestDatabase();
  const repo = new SettingsRepository(db);
  return { service: new SettingsService(repo), repo };
}

function persona(overrides: Partial<PersonaDefinition> = {}): PersonaDefinition {
  return {
    name: "Test Persona",
    systemPrompt: "You are a test.",
    description: "A test persona",
    ...overrides,
  };
}

function hat(overrides: Partial<HatDefinition> = {}): HatDefinition {
  return {
    name: "Test Hat",
    triggersOn: ["task.start"],
    publishes: ["task.done"],
    description: "A test hat",
    ...overrides,
  };
}

describe("SettingsService", () => {
  beforeEach(() => {
    initializeTestDatabase();
  });

  afterEach(() => {
    closeTestDatabase();
  });

  // ============================================================
  // Persona Methods
  // ============================================================

  describe("Persona methods", () => {
    test("getCurrentPersona returns DEFAULT_PERSONA when unset", () => {
      const { service } = makeService();
      assert.strictEqual(service.getCurrentPersona(), DEFAULT_PERSONA);
    });

    test("setCurrentPersona persists and getCurrentPersona reads it back", () => {
      const { service } = makeService();
      service.setCurrentPersona("friendly");
      assert.strictEqual(service.getCurrentPersona(), "friendly");
    });

    test("setCurrentPersona writes to the PERSONA_CURRENT key", () => {
      const { service, repo } = makeService();
      service.setCurrentPersona("formal");
      assert.strictEqual(
        repo.get<string>(SettingKeys.PERSONA_CURRENT),
        "formal",
      );
    });

    test("getPersonaDefinitions returns {} when none defined", () => {
      const { service } = makeService();
      assert.deepStrictEqual(service.getPersonaDefinitions(), {});
    });

    test("setPersona + getPersona round-trip", () => {
      const { service } = makeService();
      const p = persona({ name: "Pirate", systemPrompt: "Arrr" });
      service.setPersona("pirate", p);
      assert.deepStrictEqual(service.getPersona("pirate"), p);
    });

    test("setPersona preserves existing personas when adding a new one", () => {
      const { service } = makeService();
      service.setPersona("a", persona({ name: "A" }));
      service.setPersona("b", persona({ name: "B" }));

      const defs = service.getPersonaDefinitions();
      assert.strictEqual(Object.keys(defs).length, 2);
      assert.strictEqual(defs["a"].name, "A");
      assert.strictEqual(defs["b"].name, "B");
    });

    test("setPersona overwrites an existing persona with the same key", () => {
      const { service } = makeService();
      service.setPersona("p", persona({ name: "Old" }));
      service.setPersona("p", persona({ name: "New" }));

      assert.strictEqual(service.getPersona("p")?.name, "New");
      assert.strictEqual(service.listPersonas().length, 1);
    });

    test("getPersona returns undefined for unknown name", () => {
      const { service } = makeService();
      assert.strictEqual(service.getPersona("missing"), undefined);
    });

    test("getCurrentPersonaDefinition returns FALLBACK for unseeded default persona", () => {
      const { service } = makeService();
      // No current persona set (defaults to DEFAULT_PERSONA) and no definitions stored
      const def = service.getCurrentPersonaDefinition();
      assert.deepStrictEqual(def, FALLBACK_PERSONA_DEFINITION);
    });

    test("getCurrentPersonaDefinition returns the stored default when seeded", () => {
      const { service } = makeService();
      const stored = persona({ name: "Seeded Default", systemPrompt: "seeded" });
      service.setPersona(DEFAULT_PERSONA, stored);
      assert.deepStrictEqual(service.getCurrentPersonaDefinition(), stored);
    });

    test("getCurrentPersonaDefinition returns undefined for unknown non-default persona", () => {
      const { service } = makeService();
      service.setCurrentPersona("nonexistent");
      assert.strictEqual(service.getCurrentPersonaDefinition(), undefined);
    });

    test("getCurrentPersonaDefinition returns the active (non-default) persona's definition", () => {
      const { service } = makeService();
      const p = persona({ name: "Active" });
      service.setPersona("active", p);
      service.setCurrentPersona("active");
      assert.deepStrictEqual(service.getCurrentPersonaDefinition(), p);
    });

    test("deletePersona removes the persona and returns true", () => {
      const { service } = makeService();
      service.setPersona("gone", persona());
      assert.strictEqual(service.deletePersona("gone"), true);
      assert.strictEqual(service.hasPersona("gone"), false);
    });

    test("deletePersona returns false when persona does not exist", () => {
      const { service } = makeService();
      assert.strictEqual(service.deletePersona("nope"), false);
    });

    test("deletePersona resets current persona to DEFAULT when the active one is deleted", () => {
      const { service } = makeService();
      service.setPersona("active", persona());
      service.setCurrentPersona("active");

      service.deletePersona("active");

      assert.strictEqual(service.getCurrentPersona(), DEFAULT_PERSONA);
    });

    test("deletePersona leaves current persona alone when deleting a different one", () => {
      const { service } = makeService();
      service.setPersona("keep", persona());
      service.setPersona("drop", persona());
      service.setCurrentPersona("keep");

      service.deletePersona("drop");

      assert.strictEqual(service.getCurrentPersona(), "keep");
    });

    test("listPersonas returns all defined persona names", () => {
      const { service } = makeService();
      service.setPersona("a", persona());
      service.setPersona("b", persona());
      service.setPersona("c", persona());

      const names = service.listPersonas().sort();
      assert.deepStrictEqual(names, ["a", "b", "c"]);
    });

    test("listPersonas returns [] when none defined", () => {
      const { service } = makeService();
      assert.deepStrictEqual(service.listPersonas(), []);
    });

    test("hasPersona returns true for existing and false for missing", () => {
      const { service } = makeService();
      service.setPersona("here", persona());

      assert.strictEqual(service.hasPersona("here"), true);
      assert.strictEqual(service.hasPersona("gone"), false);
    });
  });

  // ============================================================
  // Hat Methods
  // ============================================================

  describe("Hat methods", () => {
    test("getActiveHat returns DEFAULT_HAT when unset", () => {
      const { service } = makeService();
      assert.strictEqual(service.getActiveHat(), DEFAULT_HAT);
    });

    test("setActiveHat persists and getActiveHat reads it back", () => {
      const { service } = makeService();
      service.setActiveHat("builder");
      assert.strictEqual(service.getActiveHat(), "builder");
    });

    test("setActiveHat writes to the HAT_ACTIVE key", () => {
      const { service, repo } = makeService();
      service.setActiveHat("planner");
      assert.strictEqual(
        repo.get<string>(SettingKeys.HAT_ACTIVE),
        "planner",
      );
    });

    test("getHatDefinitions returns {} when none defined", () => {
      const { service } = makeService();
      assert.deepStrictEqual(service.getHatDefinitions(), {});
    });

    test("setHat + getHat round-trip", () => {
      const { service } = makeService();
      const h = hat({ name: "Planner", triggersOn: ["plan.start"] });
      service.setHat("planner", h);
      assert.deepStrictEqual(service.getHat("planner"), h);
    });

    test("setHat preserves existing hats when adding a new one", () => {
      const { service } = makeService();
      service.setHat("a", hat({ name: "A" }));
      service.setHat("b", hat({ name: "B" }));

      const defs = service.getHatDefinitions();
      assert.strictEqual(Object.keys(defs).length, 2);
    });

    test("setHat overwrites existing hat with the same key", () => {
      const { service } = makeService();
      service.setHat("h", hat({ name: "Old" }));
      service.setHat("h", hat({ name: "New" }));

      assert.strictEqual(service.getHat("h")?.name, "New");
      assert.strictEqual(service.listHats().length, 1);
    });

    test("getHat returns undefined for unknown hat", () => {
      const { service } = makeService();
      assert.strictEqual(service.getHat("missing"), undefined);
    });

    test("getActiveHatDefinition returns FALLBACK for unseeded default hat", () => {
      const { service } = makeService();
      const def = service.getActiveHatDefinition();
      assert.deepStrictEqual(def, FALLBACK_HAT_DEFINITION);
    });

    test("getActiveHatDefinition returns the stored default when seeded", () => {
      const { service } = makeService();
      const stored = hat({ name: "Seeded Ralph" });
      service.setHat(DEFAULT_HAT, stored);
      assert.deepStrictEqual(service.getActiveHatDefinition(), stored);
    });

    test("getActiveHatDefinition returns undefined for unknown non-default hat", () => {
      const { service } = makeService();
      service.setActiveHat("nonexistent");
      assert.strictEqual(service.getActiveHatDefinition(), undefined);
    });

    test("getActiveHatDefinition returns the active (non-default) hat's definition", () => {
      const { service } = makeService();
      const h = hat({ name: "Active" });
      service.setHat("active", h);
      service.setActiveHat("active");
      assert.deepStrictEqual(service.getActiveHatDefinition(), h);
    });

    test("deleteHat removes the hat and returns true", () => {
      const { service } = makeService();
      service.setHat("gone", hat());
      assert.strictEqual(service.deleteHat("gone"), true);
      assert.strictEqual(service.hasHat("gone"), false);
    });

    test("deleteHat returns false when hat does not exist", () => {
      const { service } = makeService();
      assert.strictEqual(service.deleteHat("nope"), false);
    });

    test("deleteHat resets active hat to DEFAULT when the active one is deleted", () => {
      const { service } = makeService();
      service.setHat("active", hat());
      service.setActiveHat("active");

      service.deleteHat("active");

      assert.strictEqual(service.getActiveHat(), DEFAULT_HAT);
    });

    test("deleteHat leaves active hat alone when deleting a different one", () => {
      const { service } = makeService();
      service.setHat("keep", hat());
      service.setHat("drop", hat());
      service.setActiveHat("keep");

      service.deleteHat("drop");

      assert.strictEqual(service.getActiveHat(), "keep");
    });

    test("listHats returns all defined hat names", () => {
      const { service } = makeService();
      service.setHat("a", hat());
      service.setHat("b", hat());

      const names = service.listHats().sort();
      assert.deepStrictEqual(names, ["a", "b"]);
    });

    test("listHats returns [] when none defined", () => {
      const { service } = makeService();
      assert.deepStrictEqual(service.listHats(), []);
    });

    test("hasHat returns true for existing and false for missing", () => {
      const { service } = makeService();
      service.setHat("here", hat());

      assert.strictEqual(service.hasHat("here"), true);
      assert.strictEqual(service.hasHat("gone"), false);
    });
  });

  // ============================================================
  // findHatsByTrigger
  // ============================================================

  describe("findHatsByTrigger", () => {
    test("returns [] when no hats defined", () => {
      const { service } = makeService();
      assert.deepStrictEqual(service.findHatsByTrigger("task.start"), []);
    });

    test("returns [] when no hat triggers on the event", () => {
      const { service } = makeService();
      service.setHat("a", hat({ triggersOn: ["other.event"] }));
      assert.deepStrictEqual(service.findHatsByTrigger("task.start"), []);
    });

    test("returns a single hat that triggers on the event", () => {
      const { service } = makeService();
      service.setHat("planner", hat({ triggersOn: ["task.start"] }));
      service.setHat("builder", hat({ triggersOn: ["plan.done"] }));

      assert.deepStrictEqual(
        service.findHatsByTrigger("task.start"),
        ["planner"],
      );
    });

    test("returns all hats that trigger on the event", () => {
      const { service } = makeService();
      service.setHat("a", hat({ triggersOn: ["task.start", "task.done"] }));
      service.setHat("b", hat({ triggersOn: ["task.start"] }));
      service.setHat("c", hat({ triggersOn: ["other.event"] }));

      const hats = service.findHatsByTrigger("task.start").sort();
      assert.deepStrictEqual(hats, ["a", "b"]);
    });
  });

  // ============================================================
  // Raw access
  // ============================================================

  describe("Raw settings access", () => {
    test("setRaw + getRaw round-trip for primitive values", () => {
      const { service } = makeService();
      service.setRaw("custom.number", 42);
      service.setRaw("custom.string", "hello");
      service.setRaw("custom.bool", true);

      assert.strictEqual(service.getRaw<number>("custom.number"), 42);
      assert.strictEqual(service.getRaw<string>("custom.string"), "hello");
      assert.strictEqual(service.getRaw<boolean>("custom.bool"), true);
    });

    test("setRaw + getRaw round-trip for complex values", () => {
      const { service } = makeService();
      const payload = { a: 1, b: ["x", "y"], c: { nested: true } };
      service.setRaw("custom.obj", payload);

      assert.deepStrictEqual(service.getRaw("custom.obj"), payload);
    });

    test("getRaw returns undefined for missing key", () => {
      const { service } = makeService();
      assert.strictEqual(service.getRaw("missing"), undefined);
    });

    test("deleteRaw returns true when key exists, false when missing", () => {
      const { service } = makeService();
      service.setRaw("kill.me", "doomed");

      assert.strictEqual(service.deleteRaw("kill.me"), true);
      assert.strictEqual(service.getRaw("kill.me"), undefined);
      assert.strictEqual(service.deleteRaw("kill.me"), false);
    });
  });

  // ============================================================
  // Persistence across service instances
  // ============================================================

  describe("Persistence across service instances", () => {
    test("values set by one service instance are visible to another sharing the same repo", () => {
      const db = getTestDatabase();
      const repo = new SettingsRepository(db);
      const a = new SettingsService(repo);
      const b = new SettingsService(repo);

      a.setPersona("shared", persona({ name: "Shared" }));
      a.setCurrentPersona("shared");

      assert.strictEqual(b.getCurrentPersona(), "shared");
      assert.strictEqual(b.getPersona("shared")?.name, "Shared");
    });
  });
});
