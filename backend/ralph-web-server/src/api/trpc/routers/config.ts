/**
 * Config router - operations for reading/writing ralph.yml configuration.
 *
 * Security considerations:
 * - Config path is hardcoded to prevent path traversal
 * - YAML parsing is safe (no code execution)
 * - Input validation ensures valid YAML before writing
 */

import { TRPCError } from "@trpc/server";
import { z } from "zod";
import * as fs from "fs";
import * as path from "path";
import YAML from "yaml";
import { router, publicProcedure } from "../context";

// Path to configs directory relative to this file (6 levels up to repo root)
const REPO_ROOT = path.resolve(__dirname, "../../../../../..");
const CONFIG_PATH = path.join(REPO_ROOT, "ralph.yml");

export const configRouter = router({
  /**
   * Get the current ralph.yml configuration
   * Returns both raw YAML string and parsed object
   */
  get: publicProcedure.query(() => {
    const configPath = CONFIG_PATH;

    if (!fs.existsSync(configPath)) {
      throw new TRPCError({
        code: "NOT_FOUND",
        message: "Configuration file not found at ralph.yml",
      });
    }

    const raw = fs.readFileSync(configPath, "utf-8");
    let parsed: Record<string, unknown> = {};

    try {
      parsed = YAML.parse(raw) as Record<string, unknown>;
    } catch (err) {
      console.warn("[Config] Failed to parse ralph.yml:", err);
    }

    return { raw, parsed };
  }),

  /**
   * Update the ralph.yml configuration
   * Validates YAML before writing to prevent corruption
   */
  update: publicProcedure
    .input(
      z.object({
        content: z.string(),
      })
    )
    .mutation(({ input }) => {
      const configPath = CONFIG_PATH;

      // Validate YAML syntax before writing
      try {
        YAML.parse(input.content);
      } catch (error) {
        throw new TRPCError({
          code: "BAD_REQUEST",
          message: `Invalid YAML syntax: ${error instanceof Error ? error.message : "Unknown error"}`,
        });
      }

      // Ensure config directory exists
      const configDir = path.dirname(configPath);
      if (!fs.existsSync(configDir)) {
        fs.mkdirSync(configDir, { recursive: true });
      }

      fs.writeFileSync(configPath, input.content, "utf-8");

      // Return the updated config
      const parsed = YAML.parse(input.content) as Record<string, unknown>;
      return { success: true, parsed };
    }),
});
