/**
 * TaskBridge config/preset resolution.
 *
 * Turns a preset identifier (builtin:*, directory:*, or a collection ID) plus
 * optional ConfigMerger / default config path into the `-c <path>` CLI args
 * passed to ralph run. Extracted from TaskBridge.enqueueTask so the class
 * stays focused on queue orchestration.
 */

import * as fs from "fs";
import * as path from "path";
import type { CollectionService } from "./CollectionService";
import type { ConfigMerger } from "./ConfigMerger";

export interface ResolveConfigArgsInput {
  preset?: string;
  defaultCwd: string;
  defaultConfigPath?: string;
  configMerger?: ConfigMerger;
  collectionService?: CollectionService;
}

/**
 * Resolve a preset (or lack thereof) to the CLI args (`-c <path>`) that ralph
 * run should receive. Returns an empty array when no config can be resolved
 * (matching the legacy behavior).
 */
export function resolveConfigArgs(input: ResolveConfigArgsInput): string[] {
  const { preset, defaultCwd, defaultConfigPath, configMerger, collectionService } = input;
  const args: string[] = [];

  // Preferred path: merge base config with preset hats (or use base as-is for "default")
  if (configMerger && defaultConfigPath) {
    const mergeResult = configMerger.merge(defaultConfigPath, preset ?? "default");
    args.push("-c", mergeResult.tempPath);
    return args;
  }

  // Legacy fallback: resolve preset to config path without merging
  let configResolved = false;

  if (preset) {
    const builtinMatch = preset.match(/^builtin:(.+)$/);
    const directoryMatch = preset.match(/^directory:(.+)$/);

    if (builtinMatch) {
      args.push("-c", preset);
      configResolved = true;
    } else if (directoryMatch) {
      const presetName = directoryMatch[1];
      const presetPath = path.join(defaultCwd, ".ralph", "hats", `${presetName}.yml`);
      args.push("-c", presetPath);
      configResolved = true;
    } else if (collectionService) {
      const yamlContent = collectionService.exportToYaml(preset);
      if (yamlContent) {
        const tempDir = path.join(defaultCwd, ".ralph", "temp");
        if (!fs.existsSync(tempDir)) {
          fs.mkdirSync(tempDir, { recursive: true });
        }
        const tempPath = path.join(tempDir, `collection-${preset}.yml`);
        fs.writeFileSync(tempPath, yamlContent, "utf-8");
        args.push("-c", tempPath);
        configResolved = true;
      }
    }
  }

  if (!configResolved && defaultConfigPath) {
    args.push("-c", defaultConfigPath);
  }

  return args;
}
