/**
 * LoopDetail Component Tests
 *
 * Tests for the expandable loop details panel covering:
 * - Collapsed vs expanded state
 * - Git vs non-git workspace labeling
 * - Status-dependent conditional fields (merge PID, merge commit, failure reason, process ID)
 * - Loop ID shortening and primary marker
 * - Age/relative time rendering
 * - Path truncation
 */

import { describe, it, expect, beforeEach, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { LoopDetail, type LoopDetailData } from "./LoopDetail";

function baseLoop(overrides: Partial<LoopDetailData> = {}): LoopDetailData {
  return {
    id: "loop-abcdef123456789",
    status: "running",
    location: "/tmp/workspace",
    prompt: "Add header to index.html",
    ...overrides,
  };
}

describe("LoopDetail", () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  describe("collapsed state", () => {
    it("renders collapsed by default", () => {
      render(<LoopDetail loop={baseLoop()} />);

      // Header is visible
      expect(screen.getByText("Loop Details")).toBeInTheDocument();

      // Expanded content (Loop ID label) should NOT be visible
      expect(screen.queryByText("Loop ID")).not.toBeInTheDocument();
    });

    it("uses aria-expanded=false when collapsed", () => {
      render(<LoopDetail loop={baseLoop()} />);

      const toggle = screen.getByRole("button");
      expect(toggle).toHaveAttribute("aria-expanded", "false");
    });

    it("renders status badge in the header", () => {
      render(<LoopDetail loop={baseLoop({ status: "running" })} />);

      // LoopBadge renders the status label inside the header
      expect(screen.getByText("running")).toBeInTheDocument();
    });
  });

  describe("expansion toggle", () => {
    it("expands when header is clicked", () => {
      render(<LoopDetail loop={baseLoop()} />);

      const toggle = screen.getByRole("button");
      fireEvent.click(toggle);

      expect(screen.getByText("Loop ID")).toBeInTheDocument();
      expect(toggle).toHaveAttribute("aria-expanded", "true");
    });

    it("collapses on second click", () => {
      render(<LoopDetail loop={baseLoop()} />);
      const toggle = screen.getByRole("button");

      fireEvent.click(toggle);
      expect(screen.getByText("Loop ID")).toBeInTheDocument();

      fireEvent.click(toggle);
      expect(screen.queryByText("Loop ID")).not.toBeInTheDocument();
    });

    it("respects defaultExpanded=true", () => {
      render(<LoopDetail loop={baseLoop()} defaultExpanded />);

      expect(screen.getByText("Loop ID")).toBeInTheDocument();
      expect(screen.getByRole("button")).toHaveAttribute("aria-expanded", "true");
    });
  });

  describe("loop ID display", () => {
    it("shortens long IDs to 12 chars when expanded", () => {
      render(
        <LoopDetail loop={baseLoop({ id: "loop-abcdef123456789" })} defaultExpanded />,
      );

      // shortId slices first 12 chars: "loop-abcdef1"
      expect(screen.getByText("loop-abcdef1")).toBeInTheDocument();
    });

    it("keeps full ID in the title attribute for hover", () => {
      render(
        <LoopDetail loop={baseLoop({ id: "loop-abcdef123456789" })} defaultExpanded />,
      );

      const idSpan = screen.getByText("loop-abcdef1");
      expect(idSpan).toHaveAttribute("title", "loop-abcdef123456789");
    });

    it("marks primary loops with (primary) suffix", () => {
      render(<LoopDetail loop={baseLoop({ isPrimary: true })} defaultExpanded />);

      expect(screen.getByText(/\(primary\)/)).toBeInTheDocument();
    });

    it("does not mark non-primary loops", () => {
      render(<LoopDetail loop={baseLoop({ isPrimary: false })} defaultExpanded />);

      expect(screen.queryByText(/\(primary\)/)).not.toBeInTheDocument();
    });
  });

  describe("git vs non-git workspace", () => {
    it("shows 'Worktree' label when repoRoot is set", () => {
      render(
        <LoopDetail
          loop={baseLoop({ repoRoot: "/home/user/repo" })}
          defaultExpanded
        />,
      );

      expect(screen.getByText("Worktree")).toBeInTheDocument();
      expect(screen.queryByText("Workspace")).not.toBeInTheDocument();
    });

    it("shows 'Workspace' label when repoRoot is null", () => {
      render(
        <LoopDetail loop={baseLoop({ repoRoot: null })} defaultExpanded />,
      );

      expect(screen.getByText("Workspace")).toBeInTheDocument();
      expect(screen.queryByText("Worktree")).not.toBeInTheDocument();
    });

    it("shows 'Workspace' label when repoRoot is undefined", () => {
      render(<LoopDetail loop={baseLoop()} defaultExpanded />);

      expect(screen.getByText("Workspace")).toBeInTheDocument();
    });

    it("prefers workspaceRoot over location for path value", () => {
      render(
        <LoopDetail
          loop={baseLoop({
            workspaceRoot: "/preferred/path",
            location: "/other/path",
          })}
          defaultExpanded
        />,
      );

      expect(screen.getByText("/preferred/path")).toBeInTheDocument();
    });

    it("falls back to cwd if neither workspaceRoot nor location are set", () => {
      render(
        <LoopDetail
          loop={{
            id: "loop-1",
            status: "running",
            location: "",
            prompt: "p",
            cwd: "/cwd/fallback",
          }}
          defaultExpanded
        />,
      );

      expect(screen.getByText("/cwd/fallback")).toBeInTheDocument();
    });

    it("falls back to '(unknown)' if no path fields are set", () => {
      render(
        <LoopDetail
          loop={{ id: "loop-1", status: "running", location: "", prompt: "p" }}
          defaultExpanded
        />,
      );

      expect(screen.getByText("(unknown)")).toBeInTheDocument();
    });

    it("truncates long paths with ellipsis prefix", () => {
      const longPath =
        "/very/long/path/with/lots/of/segments/that/exceed/sixty/characters/total";
      render(
        <LoopDetail
          loop={baseLoop({ workspaceRoot: longPath })}
          defaultExpanded
        />,
      );

      const pathEl = screen.getByTitle(longPath);
      // Displayed value starts with "..." due to truncation
      expect(pathEl.textContent).toMatch(/^\.\.\./);
    });
  });

  describe("prompt display", () => {
    it("renders the prompt text", () => {
      render(
        <LoopDetail
          loop={baseLoop({ prompt: "Fix the bug in parser" })}
          defaultExpanded
        />,
      );

      expect(screen.getByText("Fix the bug in parser")).toBeInTheDocument();
    });

    it("renders '(no prompt)' placeholder when prompt is empty", () => {
      render(<LoopDetail loop={baseLoop({ prompt: "" })} defaultExpanded />);

      expect(screen.getByText("(no prompt)")).toBeInTheDocument();
    });
  });

  describe("status-dependent fields", () => {
    it("shows merge PID only when status is 'merging'", () => {
      const { rerender } = render(
        <LoopDetail
          loop={baseLoop({ status: "merging", mergePid: 12345 })}
          defaultExpanded
        />,
      );

      expect(screen.getByText("Merge PID")).toBeInTheDocument();
      expect(screen.getByText("12345")).toBeInTheDocument();

      // Different status — should hide
      rerender(
        <LoopDetail
          loop={baseLoop({ status: "running", mergePid: 12345 })}
          defaultExpanded
        />,
      );

      expect(screen.queryByText("Merge PID")).not.toBeInTheDocument();
    });

    it("does not show merge PID when status is merging but mergePid missing", () => {
      render(
        <LoopDetail loop={baseLoop({ status: "merging" })} defaultExpanded />,
      );

      expect(screen.queryByText("Merge PID")).not.toBeInTheDocument();
    });

    it("shows merge commit only when status is 'merged'", () => {
      render(
        <LoopDetail
          loop={baseLoop({
            status: "merged",
            mergeCommit: "abc12345678901234567890",
          })}
          defaultExpanded
        />,
      );

      expect(screen.getByText("Merge Commit")).toBeInTheDocument();
      // Shortens to 8 chars
      expect(screen.getByText("abc12345")).toBeInTheDocument();
    });

    it("shows failure reason only when status is 'needs-review'", () => {
      render(
        <LoopDetail
          loop={baseLoop({
            status: "needs-review",
            failureReason: "Merge conflict in README",
          })}
          defaultExpanded
        />,
      );

      expect(screen.getByText("Failure Reason")).toBeInTheDocument();
      expect(screen.getByText("Merge conflict in README")).toBeInTheDocument();
    });

    it("does not show failure reason when status is not needs-review", () => {
      render(
        <LoopDetail
          loop={baseLoop({
            status: "running",
            failureReason: "should not show",
          })}
          defaultExpanded
        />,
      );

      expect(screen.queryByText("Failure Reason")).not.toBeInTheDocument();
    });

    it("shows process ID only when status is 'running' and pid is set", () => {
      render(
        <LoopDetail
          loop={baseLoop({ status: "running", pid: 99999 })}
          defaultExpanded
        />,
      );

      expect(screen.getByText("Process ID")).toBeInTheDocument();
      expect(screen.getByText("99999")).toBeInTheDocument();
    });

    it("does not show process ID when running but pid missing", () => {
      render(
        <LoopDetail loop={baseLoop({ status: "running" })} defaultExpanded />,
      );

      expect(screen.queryByText("Process ID")).not.toBeInTheDocument();
    });

    it("does not show process ID when pid present but status is not running", () => {
      render(
        <LoopDetail
          loop={baseLoop({ status: "merged", pid: 99999 })}
          defaultExpanded
        />,
      );

      expect(screen.queryByText("Process ID")).not.toBeInTheDocument();
    });
  });

  describe("age display", () => {
    it("renders age from startedAt", () => {
      const fiveMinAgo = new Date(Date.now() - 5 * 60 * 1000).toISOString();
      render(
        <LoopDetail
          loop={baseLoop({ startedAt: fiveMinAgo })}
          defaultExpanded
        />,
      );

      // Should be "5m ago" (allowing for small timing drift)
      expect(screen.getByText(/\d+m ago/)).toBeInTheDocument();
    });

    it("prefers startedAt over queuedAt", () => {
      const tenMinAgo = new Date(Date.now() - 10 * 60 * 1000).toISOString();
      const oneMinAgo = new Date(Date.now() - 60 * 1000).toISOString();

      render(
        <LoopDetail
          loop={baseLoop({ startedAt: oneMinAgo, queuedAt: tenMinAgo })}
          defaultExpanded
        />,
      );

      // Should show the 1-minute age, not 10-minute
      expect(screen.getByText(/1m ago/)).toBeInTheDocument();
      expect(screen.queryByText(/10m ago/)).not.toBeInTheDocument();
    });

    it("falls back to queuedAt when startedAt is absent", () => {
      const threeHoursAgo = new Date(Date.now() - 3 * 60 * 60 * 1000).toISOString();
      render(
        <LoopDetail
          loop={baseLoop({ queuedAt: threeHoursAgo })}
          defaultExpanded
        />,
      );

      expect(screen.getByText(/\dh ago/)).toBeInTheDocument();
    });

    it("renders seconds ago format for very recent times", () => {
      const thirtySecAgo = new Date(Date.now() - 30 * 1000).toISOString();
      render(
        <LoopDetail
          loop={baseLoop({ startedAt: thirtySecAgo })}
          defaultExpanded
        />,
      );

      expect(screen.getByText(/\d+s ago/)).toBeInTheDocument();
    });

    it("does not render age section when no timestamps provided", () => {
      render(<LoopDetail loop={baseLoop()} defaultExpanded />);

      expect(screen.queryByText(/\d+[smhd] ago/)).not.toBeInTheDocument();
    });
  });

  describe("className prop", () => {
    it("applies additional className to outer container", () => {
      const { container } = render(
        <LoopDetail loop={baseLoop()} className="extra-class" />,
      );

      expect(container.querySelector(".extra-class")).toBeInTheDocument();
    });
  });
});
