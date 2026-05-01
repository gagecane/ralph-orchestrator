/**
 * Unit tests for PromptWriter
 */

import { test } from "node:test";
import assert from "node:assert";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { PromptWriter, PromptContent } from "./PromptWriter";
import type {
  HatDefinition,
  PersonaDefinition,
  SettingsService,
} from "../services/SettingsService";

/**
 * Creates a fresh temp directory for a test and returns its path.
 * Uses a unique name to avoid collisions between concurrent tests.
 */
function makeTempDir(label: string): string {
  const dir = path.join(os.tmpdir(), `promptwriter-test-${label}-${Date.now()}-${Math.random().toString(36).slice(2)}`);
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

/**
 * Minimal SettingsService stub for testing context injection.
 * Only implements the two methods PromptWriter consumes.
 */
function makeSettingsStub(
  persona: PersonaDefinition | undefined,
  hat: HatDefinition | undefined
): SettingsService {
  return {
    getCurrentPersonaDefinition: () => persona,
    getActiveHatDefinition: () => hat,
  } as unknown as SettingsService;
}

// ============================================================================
// Constructor / options
// ============================================================================

test("PromptWriter uses os.tmpdir() by default", () => {
  const writer = new PromptWriter({ autoCleanup: false });
  const filePath = writer.writeText("hello");

  try {
    assert.ok(
      filePath.startsWith(os.tmpdir()),
      `Expected file path to start with os.tmpdir() (${os.tmpdir()}), got ${filePath}`
    );
  } finally {
    writer.cleanupAll();
  }
});

test("PromptWriter uses custom tempDir when provided", () => {
  const tempDir = makeTempDir("custom-dir");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const filePath = writer.writeText("hello");
    assert.ok(
      filePath.startsWith(tempDir),
      `Expected file path to start with ${tempDir}, got ${filePath}`
    );
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter uses default file prefix 'ralph-prompt-'", () => {
  const tempDir = makeTempDir("default-prefix");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const filePath = writer.writeText("hello");
    assert.ok(
      path.basename(filePath).startsWith("ralph-prompt-"),
      `Expected filename to start with 'ralph-prompt-', got ${path.basename(filePath)}`
    );
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter uses custom filePrefix when provided", () => {
  const tempDir = makeTempDir("custom-prefix");
  const writer = new PromptWriter({
    tempDir,
    filePrefix: "my-prefix-",
    autoCleanup: false,
  });

  try {
    const filePath = writer.writeText("hello");
    assert.ok(
      path.basename(filePath).startsWith("my-prefix-"),
      `Expected filename to start with 'my-prefix-', got ${path.basename(filePath)}`
    );
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter creates temp directory if it does not exist", () => {
  const tempDir = path.join(
    os.tmpdir(),
    `promptwriter-nonexistent-${Date.now()}-${Math.random().toString(36).slice(2)}`
  );
  assert.strictEqual(fs.existsSync(tempDir), false, "Precondition: tempDir should not exist");

  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    writer.writeText("hello");
    assert.ok(fs.existsSync(tempDir), "Expected writer to create the temp directory");
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

// ============================================================================
// writeText
// ============================================================================

test("PromptWriter.writeText writes content to a file and tracks it", () => {
  const tempDir = makeTempDir("writeText-basic");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const content = "Hello, prompt!";
    const filePath = writer.writeText(content);

    assert.ok(fs.existsSync(filePath), "File should exist on disk");
    assert.strictEqual(fs.readFileSync(filePath, "utf-8"), content);
    assert.strictEqual(writer.isOwnedFile(filePath), true, "File should be tracked");
    assert.strictEqual(writer.getActiveCount(), 1);
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writeText generates unique file paths for concurrent calls", () => {
  const tempDir = makeTempDir("writeText-unique");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const paths = new Set<string>();
    for (let i = 0; i < 10; i++) {
      paths.add(writer.writeText(`content-${i}`));
    }
    assert.strictEqual(paths.size, 10, "Each writeText call should produce a unique path");
    assert.strictEqual(writer.getActiveCount(), 10);
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writeText prepends persona context when SettingsService is configured", () => {
  const tempDir = makeTempDir("writeText-persona");
  const persona: PersonaDefinition = {
    name: "Helper",
    systemPrompt: "Be helpful and concise.",
  };
  const writer = new PromptWriter({
    tempDir,
    autoCleanup: false,
    settingsService: makeSettingsStub(persona, undefined),
  });

  try {
    const filePath = writer.writeText("Do the thing.");
    const content = fs.readFileSync(filePath, "utf-8");

    assert.ok(
      content.includes("<persona>\nBe helpful and concise.\n</persona>"),
      `Expected persona block in content: ${content}`
    );
    assert.ok(content.endsWith("Do the thing."), "Task text should appear after context");
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writeText prepends hat instructions when available", () => {
  const tempDir = makeTempDir("writeText-hat-instructions");
  const hat: HatDefinition = {
    name: "planner",
    triggersOn: [],
    publishes: [],
    description: "plans work",
    instructions: "Think step-by-step.",
  };
  const writer = new PromptWriter({
    tempDir,
    autoCleanup: false,
    settingsService: makeSettingsStub(undefined, hat),
  });

  try {
    const filePath = writer.writeText("Do the thing.");
    const content = fs.readFileSync(filePath, "utf-8");

    assert.ok(
      content.includes('<hat name="planner">\nThink step-by-step.\n</hat>'),
      `Expected hat instructions block: ${content}`
    );
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writeText falls back to hat description when no instructions are set", () => {
  const tempDir = makeTempDir("writeText-hat-description");
  const hat: HatDefinition = {
    name: "builder",
    triggersOn: [],
    publishes: [],
    description: "Builds code.",
    // No instructions
  };
  const writer = new PromptWriter({
    tempDir,
    autoCleanup: false,
    settingsService: makeSettingsStub(undefined, hat),
  });

  try {
    const filePath = writer.writeText("Do the thing.");
    const content = fs.readFileSync(filePath, "utf-8");

    assert.ok(
      content.includes('<hat name="builder">\nBuilds code.\n</hat>'),
      `Expected hat description fallback: ${content}`
    );
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writeText omits context prefix when SettingsService returns no data", () => {
  const tempDir = makeTempDir("writeText-empty-context");
  const writer = new PromptWriter({
    tempDir,
    autoCleanup: false,
    settingsService: makeSettingsStub(undefined, undefined),
  });

  try {
    const filePath = writer.writeText("Do the thing.");
    const content = fs.readFileSync(filePath, "utf-8");

    assert.strictEqual(
      content,
      "Do the thing.",
      "No context prefix should be added when persona and hat are undefined"
    );
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writeText omits persona prefix when persona has no systemPrompt", () => {
  const tempDir = makeTempDir("writeText-empty-persona");
  const persona: PersonaDefinition = {
    name: "Empty",
    systemPrompt: "", // Empty string -> falsy
  };
  const writer = new PromptWriter({
    tempDir,
    autoCleanup: false,
    settingsService: makeSettingsStub(persona, undefined),
  });

  try {
    const filePath = writer.writeText("Do the thing.");
    const content = fs.readFileSync(filePath, "utf-8");

    assert.strictEqual(content, "Do the thing.");
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writeText combines persona and hat context", () => {
  const tempDir = makeTempDir("writeText-persona-and-hat");
  const persona: PersonaDefinition = {
    name: "Helper",
    systemPrompt: "Be helpful.",
  };
  const hat: HatDefinition = {
    name: "planner",
    triggersOn: [],
    publishes: [],
    description: "plans",
    instructions: "Plan first.",
  };
  const writer = new PromptWriter({
    tempDir,
    autoCleanup: false,
    settingsService: makeSettingsStub(persona, hat),
  });

  try {
    const filePath = writer.writeText("Task body");
    const content = fs.readFileSync(filePath, "utf-8");

    const personaIdx = content.indexOf("<persona>");
    const hatIdx = content.indexOf('<hat name="planner">');
    const taskIdx = content.indexOf("Task body");

    assert.ok(personaIdx !== -1, "Persona block missing");
    assert.ok(hatIdx !== -1, "Hat block missing");
    assert.ok(taskIdx !== -1, "Task text missing");
    assert.ok(personaIdx < hatIdx, "Persona should appear before hat");
    assert.ok(hatIdx < taskIdx, "Hat should appear before task");
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

// ============================================================================
// writePrompt
// ============================================================================

test("PromptWriter.writePrompt writes task-only prompt", () => {
  const tempDir = makeTempDir("writePrompt-task-only");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const prompt: PromptContent = { task: "Just do it." };
    const filePath = writer.writePrompt(prompt);
    const content = fs.readFileSync(filePath, "utf-8");

    assert.strictEqual(content, "Just do it.");
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writePrompt includes system, context, task and metadata in order", () => {
  const tempDir = makeTempDir("writePrompt-all-fields");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const prompt: PromptContent = {
      task: "Main task.",
      context: "Background info.",
      system: "System instructions.",
      metadata: { key: "value", count: 42 },
    };
    const filePath = writer.writePrompt(prompt);
    const content = fs.readFileSync(filePath, "utf-8");

    const systemIdx = content.indexOf("<system>");
    const contextIdx = content.indexOf("<context>");
    const taskIdx = content.indexOf("Main task.");
    const metadataIdx = content.indexOf("<!-- metadata:");

    assert.ok(systemIdx !== -1, "System block missing");
    assert.ok(contextIdx !== -1, "Context block missing");
    assert.ok(taskIdx !== -1, "Task text missing");
    assert.ok(metadataIdx !== -1, "Metadata comment missing");

    assert.ok(systemIdx < contextIdx, "System should come before context");
    assert.ok(contextIdx < taskIdx, "Context should come before task");
    assert.ok(taskIdx < metadataIdx, "Task should come before metadata");

    assert.ok(content.includes("System instructions."));
    assert.ok(content.includes("Background info."));
    assert.ok(content.includes('"key":"value"'));
    assert.ok(content.includes('"count":42'));
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writePrompt omits metadata comment when metadata is empty", () => {
  const tempDir = makeTempDir("writePrompt-empty-metadata");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const filePath = writer.writePrompt({ task: "Task", metadata: {} });
    const content = fs.readFileSync(filePath, "utf-8");

    assert.ok(!content.includes("<!-- metadata:"), "Empty metadata should not produce a comment");
    assert.strictEqual(content, "Task");
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writePrompt prepends persona/hat context once (no double injection)", () => {
  const tempDir = makeTempDir("writePrompt-no-double-injection");
  const persona: PersonaDefinition = {
    name: "P",
    systemPrompt: "PERSONA_MARKER",
  };
  const writer = new PromptWriter({
    tempDir,
    autoCleanup: false,
    settingsService: makeSettingsStub(persona, undefined),
  });

  try {
    const filePath = writer.writePrompt({ task: "Task body" });
    const content = fs.readFileSync(filePath, "utf-8");

    const occurrences = content.split("PERSONA_MARKER").length - 1;
    assert.strictEqual(
      occurrences,
      1,
      `Persona marker should appear exactly once, found ${occurrences} times`
    );
    assert.ok(content.includes("<persona>"));
    assert.ok(content.includes("Task body"));
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.writePrompt tracks created file for cleanup", () => {
  const tempDir = makeTempDir("writePrompt-tracking");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const filePath = writer.writePrompt({ task: "Task" });
    assert.strictEqual(writer.isOwnedFile(filePath), true);
    assert.strictEqual(writer.getActiveCount(), 1);
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

// ============================================================================
// read
// ============================================================================

test("PromptWriter.read returns the content previously written", () => {
  const tempDir = makeTempDir("read-basic");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const content = "round-trip content";
    const filePath = writer.writeText(content);

    assert.strictEqual(writer.read(filePath), content);
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.read throws when file does not exist", () => {
  const writer = new PromptWriter({ autoCleanup: false });
  const fakePath = path.join(os.tmpdir(), `nonexistent-${Date.now()}.txt`);

  assert.throws(() => writer.read(fakePath), /ENOENT|no such file/i);
});

// ============================================================================
// delete
// ============================================================================

test("PromptWriter.delete removes the file and untracks it", () => {
  const tempDir = makeTempDir("delete-basic");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const filePath = writer.writeText("to be deleted");
    assert.ok(fs.existsSync(filePath));

    const result = writer.delete(filePath);

    assert.strictEqual(result, true);
    assert.strictEqual(fs.existsSync(filePath), false, "File should be removed from disk");
    assert.strictEqual(writer.isOwnedFile(filePath), false, "File should no longer be tracked");
    assert.strictEqual(writer.getActiveCount(), 0);
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.delete returns false for unknown file paths", () => {
  const writer = new PromptWriter({ autoCleanup: false });
  const fakePath = path.join(os.tmpdir(), `never-created-${Date.now()}.txt`);

  const result = writer.delete(fakePath);

  assert.strictEqual(result, false);
});

test("PromptWriter.delete returns true and untracks even when file is already gone", () => {
  const tempDir = makeTempDir("delete-preremoved");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const filePath = writer.writeText("content");
    // Remove the file out-of-band
    fs.unlinkSync(filePath);
    assert.strictEqual(fs.existsSync(filePath), false);

    const result = writer.delete(filePath);

    assert.strictEqual(result, true, "delete should still succeed and clean up tracking state");
    assert.strictEqual(writer.isOwnedFile(filePath), false);
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

// ============================================================================
// cleanupAll
// ============================================================================

test("PromptWriter.cleanupAll removes all tracked files and returns the count", () => {
  const tempDir = makeTempDir("cleanupAll-basic");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const f1 = writer.writeText("a");
    const f2 = writer.writeText("b");
    const f3 = writer.writePrompt({ task: "c" });

    assert.strictEqual(writer.getActiveCount(), 3);

    const cleaned = writer.cleanupAll();

    assert.strictEqual(cleaned, 3);
    assert.strictEqual(fs.existsSync(f1), false);
    assert.strictEqual(fs.existsSync(f2), false);
    assert.strictEqual(fs.existsSync(f3), false);
    assert.strictEqual(writer.getActiveCount(), 0);
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.cleanupAll tolerates files that were already removed", () => {
  const tempDir = makeTempDir("cleanupAll-preremoved");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const f1 = writer.writeText("a");
    const f2 = writer.writeText("b");

    // Remove one file out-of-band so cleanup sees a mix
    fs.unlinkSync(f1);

    const cleaned = writer.cleanupAll();

    // Only f2 actually existed at cleanup time
    assert.strictEqual(cleaned, 1);
    assert.strictEqual(fs.existsSync(f2), false);
    assert.strictEqual(writer.getActiveCount(), 0, "Tracking set should be cleared even on partial failure");
  } finally {
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.cleanupAll returns 0 when nothing has been written", () => {
  const writer = new PromptWriter({ autoCleanup: false });

  assert.strictEqual(writer.cleanupAll(), 0);
  assert.strictEqual(writer.getActiveCount(), 0);
});

// ============================================================================
// Accessors
// ============================================================================

test("PromptWriter.getCreatedFiles returns a snapshot array of tracked paths", () => {
  const tempDir = makeTempDir("getCreatedFiles");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const f1 = writer.writeText("a");
    const f2 = writer.writeText("b");

    const created = writer.getCreatedFiles();
    assert.strictEqual(created.length, 2);
    assert.ok(created.includes(f1));
    assert.ok(created.includes(f2));

    // Returned array should be a snapshot, not a live reference
    created.push("fake-path");
    assert.strictEqual(writer.getActiveCount(), 2, "Mutating the snapshot should not affect internal state");
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.getActiveCount tracks writes and deletes", () => {
  const tempDir = makeTempDir("getActiveCount");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    assert.strictEqual(writer.getActiveCount(), 0);
    const f1 = writer.writeText("a");
    assert.strictEqual(writer.getActiveCount(), 1);
    writer.writeText("b");
    assert.strictEqual(writer.getActiveCount(), 2);
    writer.delete(f1);
    assert.strictEqual(writer.getActiveCount(), 1);
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

test("PromptWriter.isOwnedFile only returns true for paths it created", () => {
  const tempDir = makeTempDir("isOwnedFile");
  const writer = new PromptWriter({ tempDir, autoCleanup: false });

  try {
    const owned = writer.writeText("mine");
    const foreignPath = path.join(tempDir, "foreign.txt");
    fs.writeFileSync(foreignPath, "not mine");

    assert.strictEqual(writer.isOwnedFile(owned), true);
    assert.strictEqual(writer.isOwnedFile(foreignPath), false);
    assert.strictEqual(writer.isOwnedFile("/definitely/does/not/exist"), false);
  } finally {
    writer.cleanupAll();
    fs.rmSync(tempDir, { recursive: true, force: true });
  }
});

// ============================================================================
// Auto-cleanup handler
// ============================================================================

test("PromptWriter registers process 'exit' listener when autoCleanup is true (default)", () => {
  const before = process.listenerCount("exit");
  const writer = new PromptWriter();
  const after = process.listenerCount("exit");

  try {
    assert.ok(
      after > before,
      `Expected 'exit' listener count to increase (before=${before}, after=${after})`
    );
  } finally {
    writer.cleanupAll();
  }
});

test("PromptWriter does NOT register process listeners when autoCleanup is false", () => {
  const beforeExit = process.listenerCount("exit");
  const beforeSigint = process.listenerCount("SIGINT");
  const beforeSigterm = process.listenerCount("SIGTERM");

  const writer = new PromptWriter({ autoCleanup: false });

  try {
    assert.strictEqual(process.listenerCount("exit"), beforeExit);
    assert.strictEqual(process.listenerCount("SIGINT"), beforeSigint);
    assert.strictEqual(process.listenerCount("SIGTERM"), beforeSigterm);
  } finally {
    writer.cleanupAll();
  }
});
