/**
 * TaskBridge helpers.
 *
 * Pure, side-effect-free helpers extracted from TaskBridge.ts to keep the
 * service class focused on orchestration.
 */

import { execSync } from "child_process";
import type { RunnerResultPayload } from "./TaskBridge.types";

/**
 * Get the git repository root path from a given directory.
 * Falls back to the provided directory if not in a git repo.
 */
export function getGitRepoRoot(cwd: string): string {
  try {
    return execSync("git rev-parse --show-toplevel", { cwd, encoding: "utf-8" }).trim();
  } catch {
    return cwd;
  }
}

/**
 * Extract a meaningful summary from task output.
 * Looks for the last substantive message that describes what was accomplished.
 */
export function extractSummaryFromOutput(result: RunnerResultPayload): string | null {
  const output = result.combined || result.stdout || "";
  if (!output) return null;

  const lines = output.split("\n").filter((line) => line.trim());

  // Look for summary-like content in the last 30 lines
  const lastLines = lines.slice(-30);

  // Try to find meaningful summary lines (not just status/progress)
  const summaryPatterns = [
    /^#+\s*(summary|completed|done|result)/i,
    /completed.*successfully/i,
    /task.*complete/i,
    /all.*pass/i,
    /commit.*:/i,
  ];

  // Collect meaningful lines
  const meaningfulLines: string[] = [];
  let inSummarySection = false;

  for (const line of lastLines) {
    // Check if we're entering a summary section
    if (summaryPatterns.some((p) => p.test(line))) {
      inSummarySection = true;
    }

    // Skip noise lines
    if (line.startsWith(">") || line.includes("───") || line.match(/^\s*$/)) {
      continue;
    }

    if (inSummarySection || meaningfulLines.length > 0) {
      meaningfulLines.push(line);
    }
  }

  // If we found summary content, return it
  if (meaningfulLines.length > 0) {
    return meaningfulLines.slice(0, 15).join("\n"); // Cap at 15 lines
  }

  // Fallback: return last few non-empty lines
  return lastLines.slice(-5).join("\n") || null;
}
