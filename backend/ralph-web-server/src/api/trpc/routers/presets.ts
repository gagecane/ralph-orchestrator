/**
 * Presets router - operations for listing available presets.
 *
 * Presets come from three sources:
 * 1. Builtin presets (from presets/*.yml at repo root)
 * 2. Directory presets (from .ralph/hats/ or configured path)
 * 3. Database collections (created via Builder tool)
 */

import * as fs from "fs";
import * as path from "path";
import YAML from "yaml";
import { router, publicProcedure } from "../context";

/**
 * Preset type for the presets.list endpoint
 */
export interface Preset {
  id: string;
  name: string;
  source: "builtin" | "directory" | "collection";
  description?: string;
  path?: string;
}

/**
 * Read YAML presets from a directory
 * @param dir - Directory to scan for .yml files
 * @param source - Source type for the presets
 * @param includePath - Whether to include the file path in the preset
 */
export function readPresetsFromDir(
  dir: string,
  source: "builtin" | "directory",
  includePath: boolean
): Preset[] {
  if (!fs.existsSync(dir)) {
    return [];
  }

  return fs
    .readdirSync(dir)
    .filter((f) => f.endsWith(".yml"))
    .map((file) => {
      const name = path.basename(file, ".yml");
      const filePath = path.join(dir, file);
      let description = "";

      try {
        const content = fs.readFileSync(filePath, "utf-8");
        const parsed = YAML.parse(content) as Record<string, unknown>;
        if (parsed && typeof parsed.description === "string") {
          description = parsed.description;
        }
      } catch (err) {
        console.warn(`[Presets] Failed to parse preset file ${file}:`, err);
      }

      return {
        id: `${source}:${name}`,
        name,
        source,
        description,
        ...(includePath && { path: filePath }),
      };
    });
}

// Path to builtin presets - shared directory at repo root (6 levels up from this file)
const BUILTIN_PRESETS_DIR = path.resolve(__dirname, "../../../../../../presets");

export function getBuiltinPresets(): Preset[] {
  return readPresetsFromDir(BUILTIN_PRESETS_DIR, "builtin", false);
}

export function getDirectoryPresets(): Preset[] {
  const hatsDir = path.resolve(process.cwd(), ".ralph/hats");
  return readPresetsFromDir(hatsDir, "directory", true);
}

export const presetsRouter = router({
  /**
   * List all presets from all sources: builtin, directory, and collections
   */
  list: publicProcedure.query(({ ctx }) => {
    const builtinPresets = getBuiltinPresets();
    const directoryPresets = getDirectoryPresets();

    // Get collections from database and convert to presets
    const collections = ctx.collectionService.listCollections();
    const collectionPresets: Preset[] = collections.map((c) => ({
      id: c.id,
      name: c.name,
      source: "collection" as const,
      description: c.description ?? undefined,
    }));

    // Return in order: builtin, directory, collection
    return [...builtinPresets, ...directoryPresets, ...collectionPresets];
  }),
});
