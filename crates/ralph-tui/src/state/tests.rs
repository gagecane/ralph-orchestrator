use super::*;
use ralph_proto::{Event, HatId};
use ratatui::text::Line;

// ========================================================================
// IterationBuffer Tests
// ========================================================================

mod iteration_buffer {
    use super::*;
    use ratatui::text::Line;

    #[test]
    fn new_creates_buffer_with_correct_initial_state() {
        let buffer = IterationBuffer::new(1);
        assert_eq!(buffer.number, 1);
        assert_eq!(buffer.line_count(), 0);
        assert_eq!(buffer.scroll_offset, 0);
    }

    #[test]
    fn append_line_adds_lines_in_order() {
        let mut buffer = IterationBuffer::new(1);
        buffer.append_line(Line::from("first"));
        buffer.append_line(Line::from("second"));
        buffer.append_line(Line::from("third"));

        assert_eq!(buffer.line_count(), 3);
        // Verify order by checking raw content
        let lines = buffer.lines.lock().unwrap();
        assert_eq!(lines[0].spans[0].content, "first");
        assert_eq!(lines[1].spans[0].content, "second");
        assert_eq!(lines[2].spans[0].content, "third");
    }

    #[test]
    fn line_count_returns_correct_count() {
        let mut buffer = IterationBuffer::new(1);
        assert_eq!(buffer.line_count(), 0);

        for i in 0..10 {
            buffer.append_line(Line::from(format!("line {}", i)));
        }
        assert_eq!(buffer.line_count(), 10);
    }

    #[test]
    fn visible_lines_returns_correct_slice_without_scroll() {
        let mut buffer = IterationBuffer::new(1);
        for i in 0..10 {
            buffer.append_line(Line::from(format!("line {}", i)));
        }

        let visible = buffer.visible_lines(5);
        assert_eq!(visible.len(), 5);
        // Should be lines 0-4
        assert_eq!(visible[0].spans[0].content, "line 0");
        assert_eq!(visible[4].spans[0].content, "line 4");
    }

    #[test]
    fn visible_lines_returns_correct_slice_with_scroll() {
        let mut buffer = IterationBuffer::new(1);
        for i in 0..10 {
            buffer.append_line(Line::from(format!("line {}", i)));
        }
        buffer.scroll_offset = 3;

        let visible = buffer.visible_lines(5);
        assert_eq!(visible.len(), 5);
        // Should be lines 3-7
        assert_eq!(visible[0].spans[0].content, "line 3");
        assert_eq!(visible[4].spans[0].content, "line 7");
    }

    #[test]
    fn visible_lines_handles_viewport_larger_than_content() {
        let mut buffer = IterationBuffer::new(1);
        for i in 0..3 {
            buffer.append_line(Line::from(format!("line {}", i)));
        }

        let visible = buffer.visible_lines(10);
        assert_eq!(visible.len(), 3); // Only 3 lines exist
    }

    #[test]
    fn visible_lines_handles_empty_buffer() {
        let buffer = IterationBuffer::new(1);
        let visible = buffer.visible_lines(5);
        assert!(visible.is_empty());
    }

    #[test]
    fn scroll_down_increases_offset() {
        let mut buffer = IterationBuffer::new(1);
        for i in 0..10 {
            buffer.append_line(Line::from(format!("line {}", i)));
        }

        assert_eq!(buffer.scroll_offset, 0);
        buffer.scroll_down(5); // viewport height 5
        assert_eq!(buffer.scroll_offset, 1);
        buffer.scroll_down(5);
        assert_eq!(buffer.scroll_offset, 2);
    }

    #[test]
    fn scroll_up_decreases_offset() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }
        buffer.scroll_offset = 5;

        buffer.scroll_up();
        assert_eq!(buffer.scroll_offset, 4);
        buffer.scroll_up();
        assert_eq!(buffer.scroll_offset, 3);
    }

    #[test]
    fn scroll_up_does_not_underflow() {
        let mut buffer = IterationBuffer::new(1);
        buffer.append_line(Line::from("line"));
        buffer.scroll_offset = 0;

        buffer.scroll_up();
        assert_eq!(buffer.scroll_offset, 0); // Should stay at 0
    }

    #[test]
    fn scroll_down_does_not_overflow() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }
        // With 10 lines and viewport 5, max scroll is 5 (shows lines 5-9)
        buffer.scroll_offset = 5;

        buffer.scroll_down(5);
        assert_eq!(buffer.scroll_offset, 5); // Should stay at max
    }

    #[test]
    fn scroll_top_resets_to_zero() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }
        buffer.scroll_offset = 5;

        buffer.scroll_top();
        assert_eq!(buffer.scroll_offset, 0);
    }

    #[test]
    fn scroll_bottom_sets_to_max() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }

        buffer.scroll_bottom(5); // viewport height 5
        assert_eq!(buffer.scroll_offset, 5); // max = 10 - 5 = 5
    }

    #[test]
    fn scroll_bottom_handles_small_content() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..3 {
            buffer.append_line(Line::from("line"));
        }

        buffer.scroll_bottom(5); // viewport larger than content
        assert_eq!(buffer.scroll_offset, 0); // Can't scroll
    }

    #[test]
    fn scroll_down_handles_empty_buffer() {
        let mut buffer = IterationBuffer::new(1);
        buffer.scroll_down(5);
        assert_eq!(buffer.scroll_offset, 0);
    }

    // =====================================================================
    // Auto-scroll (following_bottom) Tests
    // =====================================================================

    #[test]
    fn following_bottom_is_true_initially() {
        let buffer = IterationBuffer::new(1);
        assert!(
            buffer.following_bottom,
            "New buffer should start with following_bottom = true"
        );
    }

    #[test]
    fn scroll_up_disables_following_bottom() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }
        buffer.scroll_offset = 5;
        assert!(buffer.following_bottom);

        buffer.scroll_up();

        assert!(
            !buffer.following_bottom,
            "scroll_up should disable following_bottom"
        );
    }

    #[test]
    fn scroll_top_disables_following_bottom() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }
        assert!(buffer.following_bottom);

        buffer.scroll_top();

        assert!(
            !buffer.following_bottom,
            "scroll_top should disable following_bottom"
        );
    }

    #[test]
    fn scroll_bottom_enables_following_bottom() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }
        buffer.following_bottom = false;

        buffer.scroll_bottom(5);

        assert!(
            buffer.following_bottom,
            "scroll_bottom should enable following_bottom"
        );
    }

    #[test]
    fn scroll_down_to_bottom_enables_following_bottom() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }
        buffer.scroll_offset = 4; // One away from max (5 with viewport 5)
        buffer.following_bottom = false;

        buffer.scroll_down(5); // Now at max (5)

        assert!(
            buffer.following_bottom,
            "scroll_down to bottom should enable following_bottom"
        );
    }

    #[test]
    fn scroll_down_not_at_bottom_keeps_following_false() {
        let mut buffer = IterationBuffer::new(1);
        for _ in 0..10 {
            buffer.append_line(Line::from("line"));
        }
        buffer.scroll_offset = 0;
        buffer.following_bottom = false;

        buffer.scroll_down(5); // Now at 1, max is 5

        assert!(
            !buffer.following_bottom,
            "scroll_down not reaching bottom should keep following_bottom false"
        );
    }

    #[test]
    fn autoscroll_scenario_content_grows_past_viewport() {
        // This tests the core bug fix: content growing from small to large
        let mut buffer = IterationBuffer::new(1);

        // Start with small content that fits in viewport
        for _ in 0..5 {
            buffer.append_line(Line::from("line"));
        }

        // Simulate initial state: following_bottom = true, scroll_offset = 0
        let viewport = 20;
        assert!(buffer.following_bottom);
        assert_eq!(buffer.scroll_offset, 0);

        // Simulate auto-scroll logic: if following_bottom, scroll to bottom
        if buffer.following_bottom {
            let max_scroll = buffer.line_count().saturating_sub(viewport);
            buffer.scroll_offset = max_scroll;
        }
        assert_eq!(buffer.scroll_offset, 0); // max_scroll is 0 when content < viewport

        // Content grows past viewport size
        for _ in 0..25 {
            buffer.append_line(Line::from("more content"));
        }
        // Now we have 30 lines, viewport is 20, max_scroll = 10

        // The bug was: scroll_offset = 0, but old logic checked if 0 >= 10-1 (false)
        // With following_bottom flag, we just check the flag:
        if buffer.following_bottom {
            let max_scroll = buffer.line_count().saturating_sub(viewport);
            buffer.scroll_offset = max_scroll;
        }

        // Now scroll_offset should be at the bottom
        assert_eq!(
            buffer.scroll_offset, 10,
            "Auto-scroll should move to bottom when content grows past viewport"
        );
    }
}

// ========================================================================
// TuiState Tests (existing)
// ========================================================================

#[test]
fn iteration_changed_detects_boundary() {
    let mut state = TuiState::new();
    assert!(!state.iteration_changed(), "no change at start");

    // Simulate build.done event (increments iteration)
    let event = Event::new("build.done", "");
    state.update(&event);

    assert_eq!(state.iteration, 1);
    assert_eq!(state.prev_iteration, 0);
    assert!(state.iteration_changed(), "should detect iteration change");
}

#[test]
fn iteration_changed_resets_after_check() {
    let mut state = TuiState::new();
    let event = Event::new("build.done", "");
    state.update(&event);

    assert!(state.iteration_changed());

    // Simulate clearing the flag (app.rs does this by updating prev_iteration)
    state.prev_iteration = state.iteration;
    assert!(!state.iteration_changed(), "flag should reset");
}

#[test]
fn multiple_iterations_tracked() {
    let mut state = TuiState::new();

    for i in 1..=3 {
        let event = Event::new("build.done", "");
        state.update(&event);
        assert_eq!(state.iteration, i);
        assert!(state.iteration_changed());
        state.prev_iteration = state.iteration; // simulate app clearing flag
    }
}

#[test]
fn custom_hat_topics_update_pending_hat() {
    // Test that custom hat topics (not hardcoded) update pending_hat correctly
    use std::collections::HashMap;

    // Create a hat map for custom hats
    let mut hat_map = HashMap::new();
    hat_map.insert(
        "review.security".to_string(),
        (
            HatId::new("security_reviewer"),
            "🔒 Security Reviewer".to_string(),
        ),
    );
    hat_map.insert(
        "review.correctness".to_string(),
        (
            HatId::new("correctness_reviewer"),
            "🎯 Correctness Reviewer".to_string(),
        ),
    );

    let mut state = TuiState::with_hat_map(hat_map);

    // Publish review.security event
    let event = Event::new("review.security", "Review PR #123");
    state.update(&event);

    // Should update pending_hat to security reviewer
    assert_eq!(
        state.get_pending_hat_display(),
        "🔒 Security Reviewer",
        "Should display security reviewer hat for review.security topic"
    );

    // Publish review.correctness event
    let event = Event::new("review.correctness", "Check logic");
    state.update(&event);

    // Should update to correctness reviewer
    assert_eq!(
        state.get_pending_hat_display(),
        "🎯 Correctness Reviewer",
        "Should display correctness reviewer hat for review.correctness topic"
    );
}

#[test]
fn unknown_topics_keep_pending_hat_unchanged() {
    // Test that unknown topics don't clear pending_hat
    let mut state = TuiState::new();

    // Set initial hat
    state.pending_hat = Some((HatId::new("planner"), "📋Planner".to_string()));

    // Publish unknown event
    let event = Event::new("unknown.topic", "Some payload");
    state.update(&event);

    // Should keep the planner hat
    assert_eq!(
        state.get_pending_hat_display(),
        "📋Planner",
        "Unknown topics should not clear pending_hat"
    );
}

#[test]
fn task_start_preserves_iterations_across_reset() {
    // Regression test: task.start used to do *self = Self::new() which wiped
    // iteration buffers, causing the header to show "iter 1/0" and losing all
    // previous iteration output.
    let mut state = TuiState::new();

    // Create 3 iterations with content
    state.start_new_iteration();
    state.start_new_iteration();
    state.start_new_iteration();
    assert_eq!(state.total_iterations(), 3);
    assert_eq!(state.current_view, 2); // following latest

    // Navigate back to review history
    state.navigate_prev();
    assert_eq!(state.current_view, 1);
    assert!(!state.following_latest);

    // When task.start fires (e.g., new task planning session)
    let event = Event::new("task.start", "New task");
    state.update(&event);

    // Then iterations are preserved
    assert_eq!(
        state.total_iterations(),
        3,
        "task.start should not wipe iteration buffers"
    );
    assert_eq!(
        state.current_view, 1,
        "task.start should preserve current_view position"
    );
    assert!(
        !state.following_latest,
        "task.start should preserve following_latest state"
    );
}

#[test]
fn loop_terminate_freezes_iteration_timer() {
    // Given a running iteration with elapsed time
    let mut state = TuiState::new();
    let start_event = Event::new("build.task", "");
    state.update(&start_event);

    // Verify timer is running
    assert!(state.iteration_started.is_some());
    let elapsed_before = state.get_iteration_elapsed().unwrap();
    assert!(elapsed_before.as_nanos() > 0);

    // When loop.terminate is received
    let terminate_event = Event::new("loop.terminate", "");
    state.update(&terminate_event);

    // Then the timer is frozen
    assert!(state.loop_completed);
    assert!(state.final_iteration_elapsed.is_some());

    // The elapsed time should be frozen (not increasing)
    let frozen_elapsed = state.get_iteration_elapsed().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let elapsed_after_sleep = state.get_iteration_elapsed().unwrap();

    assert_eq!(
        frozen_elapsed, elapsed_after_sleep,
        "Timer should be frozen after loop.terminate"
    );
}

#[test]
fn loop_terminate_freezes_total_timer() {
    let mut state = TuiState::new();
    state.loop_started = Some(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(90))
            .unwrap(),
    );

    let before = state.get_loop_elapsed().unwrap();
    assert!(before.as_secs() >= 90);

    let terminate_event = Event::new("loop.terminate", "");
    state.update(&terminate_event);

    let frozen = state.get_loop_elapsed().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let after = state.get_loop_elapsed().unwrap();

    assert_eq!(
        frozen, after,
        "Loop elapsed time should be frozen after termination"
    );
}

#[test]
fn build_done_freezes_total_timer() {
    let mut state = TuiState::new();
    state.loop_started = Some(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(42))
            .unwrap(),
    );

    let before = state.get_loop_elapsed().unwrap();
    assert!(before.as_secs() >= 42);

    let done_event = Event::new("build.done", "");
    state.update(&done_event);

    let frozen = state.get_loop_elapsed().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let after = state.get_loop_elapsed().unwrap();

    assert_eq!(
        frozen, after,
        "Loop elapsed time should be frozen after build.done"
    );
}

#[test]
fn build_blocked_freezes_total_timer() {
    let mut state = TuiState::new();
    state.loop_started = Some(
        std::time::Instant::now()
            .checked_sub(std::time::Duration::from_secs(7))
            .unwrap(),
    );

    let before = state.get_loop_elapsed().unwrap();
    assert!(before.as_secs() >= 7);

    let blocked_event = Event::new("build.blocked", "");
    state.update(&blocked_event);

    let frozen = state.get_loop_elapsed().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    let after = state.get_loop_elapsed().unwrap();

    assert_eq!(
        frozen, after,
        "Loop elapsed time should be frozen after build.blocked"
    );
}

// ========================================================================
// TuiState Iteration Management Tests
// ========================================================================

mod tui_state_iterations {
    use super::*;

    #[test]
    fn start_new_iteration_creates_first_buffer() {
        // Given TuiState with 0 iterations
        let mut state = TuiState::new();
        assert_eq!(state.total_iterations(), 0);

        // When start_new_iteration() is called
        state.start_new_iteration();

        // Then iterations.len() == 1 and new IterationBuffer exists
        assert_eq!(state.total_iterations(), 1);
        assert_eq!(state.iterations[0].number, 1);
    }

    #[test]
    fn start_new_iteration_creates_subsequent_buffers() {
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();

        assert_eq!(state.total_iterations(), 3);
        assert_eq!(state.iterations[0].number, 1);
        assert_eq!(state.iterations[1].number, 2);
        assert_eq!(state.iterations[2].number, 3);
    }

    #[test]
    fn current_iteration_returns_correct_buffer() {
        // Given TuiState with 3 iterations and current_view = 1
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 1;

        // When current_iteration() is called
        let current = state.current_iteration();

        // Then the buffer at index 1 is returned (iteration number 2)
        assert!(current.is_some());
        assert_eq!(current.unwrap().number, 2);
    }

    #[test]
    fn current_iteration_returns_none_when_empty() {
        let state = TuiState::new();
        assert!(state.current_iteration().is_none());
    }

    #[test]
    fn current_iteration_mut_allows_modification() {
        let mut state = TuiState::new();
        state.start_new_iteration();

        // Add a line via mutable reference
        if let Some(buffer) = state.current_iteration_mut() {
            buffer.append_line(Line::from("test line"));
        }

        // Verify modification persisted
        assert_eq!(state.current_iteration().unwrap().line_count(), 1);
    }

    #[test]
    fn navigate_next_increases_current_view() {
        // Given TuiState with current_view = 1 and 3 iterations
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 1;
        state.following_latest = false;

        // When navigate_next() is called
        state.navigate_next();

        // Then current_view == 2
        assert_eq!(state.current_view, 2);
    }

    #[test]
    fn navigate_prev_decreases_current_view() {
        // Given TuiState with current_view = 2
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 2;

        // When navigate_prev() is called
        state.navigate_prev();

        // Then current_view == 1
        assert_eq!(state.current_view, 1);
    }

    #[test]
    fn navigate_next_does_not_exceed_bounds() {
        // Given TuiState with current_view = 2 and 3 iterations (max index 2)
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 2;

        // When navigate_next() is called
        state.navigate_next();

        // Then current_view stays at 2
        assert_eq!(state.current_view, 2);
    }

    #[test]
    fn navigate_prev_does_not_go_below_zero() {
        // Given TuiState with current_view = 0
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.current_view = 0;

        // When navigate_prev() is called
        state.navigate_prev();

        // Then current_view stays at 0
        assert_eq!(state.current_view, 0);
    }

    #[test]
    fn following_latest_initially_true() {
        // Given new TuiState
        // When created
        let state = TuiState::new();

        // Then following_latest == true
        assert!(state.following_latest);
    }

    #[test]
    fn following_latest_becomes_false_on_back_navigation() {
        // Given TuiState with following_latest = true and current_view = 2
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 2;
        state.following_latest = true;

        // When navigate_prev() is called
        state.navigate_prev();

        // Then following_latest == false
        assert!(!state.following_latest);
    }

    #[test]
    fn following_latest_restored_at_latest() {
        // Given TuiState with following_latest = false
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 1;
        state.following_latest = false;

        // When navigate_next() reaches the last iteration
        state.navigate_next(); // 1 -> 2 (last)

        // Then following_latest == true
        assert!(state.following_latest);
    }

    #[test]
    fn total_iterations_reports_count() {
        // Given TuiState with 3 iterations
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();

        // When total_iterations() is called
        // Then 3 is returned
        assert_eq!(state.total_iterations(), 3);
    }

    #[test]
    fn start_new_iteration_auto_follows_latest() {
        let mut state = TuiState::new();
        state.following_latest = true;
        state.start_new_iteration();
        state.start_new_iteration();

        // When following latest, current_view should track new iterations
        assert_eq!(state.current_view, 1); // Index of second iteration
    }

    // ========================================================================
    // Per-Iteration Scroll Independence Tests (Task 08)
    // ========================================================================

    #[test]
    fn per_iteration_scroll_independence() {
        // Given iteration 1 with scroll_offset 5 and iteration 2 with scroll_offset 0
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();

        // Set different scroll offsets for each iteration
        state.iterations[0].scroll_offset = 5;
        state.iterations[1].scroll_offset = 0;

        // When switching between iterations
        state.current_view = 0;
        assert_eq!(
            state.current_iteration().unwrap().scroll_offset,
            5,
            "iteration 1 should have scroll_offset 5"
        );

        state.navigate_next();
        assert_eq!(
            state.current_iteration().unwrap().scroll_offset,
            0,
            "iteration 2 should have scroll_offset 0"
        );

        // Then each iteration's scroll_offset is preserved
        state.navigate_prev();
        assert_eq!(
            state.current_iteration().unwrap().scroll_offset,
            5,
            "iteration 1 should still have scroll_offset 5 after switching back"
        );
    }

    #[test]
    fn scroll_within_iteration_does_not_affect_others() {
        // Given multiple iterations with different scroll offsets
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();

        // Add content to each iteration
        for i in 0..3 {
            for j in 0..20 {
                state.iterations[i].append_line(Line::from(format!(
                    "iter {} line {}",
                    i + 1,
                    j
                )));
            }
        }

        // Set initial scroll offsets
        state.iterations[0].scroll_offset = 3;
        state.iterations[1].scroll_offset = 7;
        state.iterations[2].scroll_offset = 10;

        // When scrolling in iteration 2
        state.current_view = 1;
        state.current_iteration_mut().unwrap().scroll_down(10);

        // Then only iteration 2's scroll changed
        assert_eq!(
            state.iterations[0].scroll_offset, 3,
            "iteration 1 unchanged"
        );
        assert_eq!(
            state.iterations[1].scroll_offset, 8,
            "iteration 2 scrolled down"
        );
        assert_eq!(
            state.iterations[2].scroll_offset, 10,
            "iteration 3 unchanged"
        );
    }

    // ========================================================================
    // New Iteration Alert Tests (Task 07)
    // ========================================================================

    #[test]
    fn new_iteration_alert_set_when_not_following() {
        // Given following_latest = false and new iteration arrives
        let mut state = TuiState::new();
        state.start_new_iteration(); // Iteration 1
        state.start_new_iteration(); // Iteration 2
        state.navigate_prev(); // Go back to iteration 1, following_latest = false

        // When start_new_iteration() is called
        state.start_new_iteration(); // Iteration 3

        // Then new_iteration_alert is set to the new iteration number
        assert_eq!(state.new_iteration_alert, Some(3));
    }

    #[test]
    fn new_iteration_alert_not_set_when_following() {
        // Given following_latest = true
        let mut state = TuiState::new();
        state.following_latest = true;
        state.start_new_iteration();

        // When start_new_iteration() is called
        state.start_new_iteration();

        // Then new_iteration_alert remains None
        assert_eq!(state.new_iteration_alert, None);
    }

    #[test]
    fn alert_cleared_when_following_restored() {
        // Given new_iteration_alert = Some(5)
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 0;
        state.following_latest = false;
        state.new_iteration_alert = Some(3);

        // When navigation restores following_latest = true
        state.navigate_next(); // 0 -> 1
        state.navigate_next(); // 1 -> 2 (last, restores following)

        // Then new_iteration_alert is cleared to None
        assert_eq!(state.new_iteration_alert, None);
    }

    #[test]
    fn alert_not_cleared_on_partial_navigation() {
        // Given new_iteration_alert = Some(3) and not at last iteration
        let mut state = TuiState::new();
        state.start_new_iteration();
        state.start_new_iteration();
        state.start_new_iteration();
        state.current_view = 0;
        state.following_latest = false;
        state.new_iteration_alert = Some(3);

        // When navigate_next() but not reaching last
        state.navigate_next(); // 0 -> 1

        // Then alert is still set (not at latest yet)
        assert_eq!(state.new_iteration_alert, Some(3));
        assert!(!state.following_latest);
    }

    #[test]
    fn alert_updates_for_multiple_new_iterations() {
        // Given not following and multiple new iterations arrive
        let mut state = TuiState::new();
        state.start_new_iteration(); // 1
        state.start_new_iteration(); // 2
        state.navigate_prev(); // Go back, stop following

        state.start_new_iteration(); // 3 arrives
        assert_eq!(state.new_iteration_alert, Some(3));

        // When another iteration arrives
        state.start_new_iteration(); // 4 arrives

        // Then alert should show the newest
        assert_eq!(state.new_iteration_alert, Some(4));
    }
}

// ========================================================================
// SearchState Tests (Task 09)
// ========================================================================

mod search_state {
    use super::*;

    #[test]
    fn search_finds_matches_in_lines() {
        // Given current iteration with "error" in 3 lines
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("First error occurred"));
        buffer.append_line(Line::from("Normal line"));
        buffer.append_line(Line::from("Another error here"));
        buffer.append_line(Line::from("Final error message"));

        // When search("error") is called
        state.search("error");

        // Then matches.len() >= 3
        assert!(
            state.search_state.matches.len() >= 3,
            "expected at least 3 matches, got {}",
            state.search_state.matches.len()
        );
        assert_eq!(state.search_state.query, Some("error".to_string()));
    }

    #[test]
    fn search_is_case_insensitive() {
        // Given current iteration with "Error" and "error"
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("Error in uppercase"));
        buffer.append_line(Line::from("error in lowercase"));
        buffer.append_line(Line::from("ERROR all caps"));

        // When search("error") is called
        state.search("error");

        // Then all 3 are found
        assert_eq!(
            state.search_state.matches.len(),
            3,
            "expected 3 case-insensitive matches"
        );
    }

    #[test]
    fn next_match_cycles_forward() {
        // Given 3 matches and current_match = 2 (last)
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("match one"));
        buffer.append_line(Line::from("match two"));
        buffer.append_line(Line::from("match three"));
        state.search("match");
        state.search_state.current_match = 2;

        // When next_match() is called
        state.next_match();

        // Then current_match becomes 0 (cycles back)
        assert_eq!(state.search_state.current_match, 0);
    }

    #[test]
    fn prev_match_cycles_backward() {
        // Given 3 matches and current_match = 0 (first)
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("match one"));
        buffer.append_line(Line::from("match two"));
        buffer.append_line(Line::from("match three"));
        state.search("match");
        state.search_state.current_match = 0;

        // When prev_match() is called
        state.prev_match();

        // Then current_match becomes 2 (cycles back)
        assert_eq!(state.search_state.current_match, 2);
    }

    #[test]
    fn search_jumps_to_match_line() {
        // Given match at line 50
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        for i in 0..60 {
            if i == 50 {
                buffer.append_line(Line::from("target match here"));
            } else {
                buffer.append_line(Line::from(format!("line {}", i)));
            }
        }

        // When search finds match at line 50
        state.search("target");

        // Then scroll_offset is updated so line 50 is visible
        let buffer = state.current_iteration().unwrap();
        // With viewport of ~20, scroll should position line 50 in view
        assert!(
            buffer.scroll_offset <= 50,
            "scroll_offset {} should position line 50 in view",
            buffer.scroll_offset
        );
    }

    #[test]
    fn clear_search_resets_state() {
        // Given active search
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("search term here"));
        state.search("term");
        assert!(state.search_state.query.is_some());

        // When clear_search() is called
        state.clear_search();

        // Then query = None, matches cleared, search_mode = false
        assert!(state.search_state.query.is_none());
        assert!(state.search_state.matches.is_empty());
        assert!(!state.search_state.search_mode);
    }

    #[test]
    fn search_with_no_matches_sets_empty() {
        // Given iteration with no matching content
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("hello world"));

        // When searching for non-existent term
        state.search("xyz");

        // Then matches is empty but query is set
        assert_eq!(state.search_state.query, Some("xyz".to_string()));
        assert!(state.search_state.matches.is_empty());
        assert_eq!(state.search_state.current_match, 0);
    }

    #[test]
    fn search_on_empty_iteration_handles_gracefully() {
        // Given empty iteration
        let mut state = TuiState::new();
        state.start_new_iteration();

        // When searching
        state.search("anything");

        // Then no panic, empty matches
        assert!(state.search_state.matches.is_empty());
    }

    #[test]
    fn next_match_with_no_matches_does_nothing() {
        // Given no active search or empty matches
        let mut state = TuiState::new();
        state.start_new_iteration();

        // When next_match is called
        state.next_match();

        // Then no panic, current_match stays 0
        assert_eq!(state.search_state.current_match, 0);
    }

    #[test]
    fn multiple_matches_on_same_line() {
        // Given line with multiple occurrences
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        buffer.append_line(Line::from("error error error"));

        // When searching
        state.search("error");

        // Then finds all 3 matches
        assert_eq!(
            state.search_state.matches.len(),
            3,
            "should find 3 matches on same line"
        );
    }

    #[test]
    fn next_match_updates_scroll_to_show_match() {
        // Given many lines with matches spread out
        let mut state = TuiState::new();
        state.start_new_iteration();
        let buffer = state.current_iteration_mut().unwrap();
        for i in 0..100 {
            if i % 30 == 0 {
                buffer.append_line(Line::from("findme"));
            } else {
                buffer.append_line(Line::from(format!("line {}", i)));
            }
        }
        state.search("findme");

        // Navigate to second match (at line 30)
        state.next_match();

        // Then scroll should position line 30 in view
        let buffer = state.current_iteration().unwrap();
        // Match at line 30, scroll should be adjusted
        assert!(buffer.scroll_offset <= 30, "scroll should show line 30");
    }

    #[test]
    fn latest_iteration_lines_handle_returns_newest_iteration() {
        // Given a user viewing iteration 1 while iteration 3 is executing
        let mut state = TuiState::new();
        state.start_new_iteration(); // iteration 1
        state.start_new_iteration(); // iteration 2
        state.start_new_iteration(); // iteration 3

        // User navigates back to iteration 1
        state.current_view = 0;
        state.following_latest = false;

        // When getting line handles
        let current_handle = state.current_iteration_lines_handle();
        let latest_handle = state.latest_iteration_lines_handle();

        // Then current_iteration_lines_handle returns iteration 1's buffer
        assert!(current_handle.is_some());
        // And latest_iteration_lines_handle returns iteration 3's buffer
        assert!(latest_handle.is_some());

        // Write to latest and verify it doesn't affect current view
        {
            let latest = latest_handle.unwrap();
            latest
                .lock()
                .unwrap()
                .push(Line::from("output from iteration 3"));
        }

        // Current view (iteration 1) should be empty
        let current = state.current_iteration().unwrap();
        assert_eq!(
            current.lines.lock().unwrap().len(),
            0,
            "iteration 1 should have no lines"
        );

        // Latest (iteration 3) should have the output
        let latest_buffer = state.iterations.last().unwrap();
        assert_eq!(
            latest_buffer.lines.lock().unwrap().len(),
            1,
            "iteration 3 should have the output"
        );
    }

    #[test]
    fn output_goes_to_correct_iteration_when_user_reviewing_history() {
        // This reproduces the bug: user is on page 3 of 6, but active agent writes to page 3
        let mut state = TuiState::new();

        // Create 6 iterations
        for _ in 0..6 {
            state.start_new_iteration();
        }

        // User navigates to iteration 3 (index 2)
        state.current_view = 2;
        state.following_latest = false;

        // New iteration starts (iteration 7)
        state.start_new_iteration();

        // Get handle for writing output - MUST use latest, not current
        let lines_handle = state.latest_iteration_lines_handle();

        // Write output
        {
            let handle = lines_handle.unwrap();
            handle
                .lock()
                .unwrap()
                .push(Line::from("iteration 7 output"));
        }

        // Verify: iteration 3 (what user is viewing) should be unaffected
        let iteration_3 = &state.iterations[2];
        assert_eq!(
            iteration_3.lines.lock().unwrap().len(),
            0,
            "iteration 3 (being viewed) should have no output"
        );

        // Verify: iteration 7 (latest) should have the output
        let iteration_7 = state.iterations.last().unwrap();
        assert_eq!(
            iteration_7.lines.lock().unwrap().len(),
            1,
            "iteration 7 (latest) should have the output"
        );
    }
}

// ========================================================================
// Guidance Tests
// ========================================================================

mod guidance {
    use super::*;

    #[test]
    fn start_guidance_sets_mode_and_clears_input() {
        let mut state = TuiState::new();
        state.guidance_input = "leftover".to_string();
        state.start_guidance(GuidanceMode::Next);
        assert_eq!(state.guidance_mode, Some(GuidanceMode::Next));
        assert!(state.guidance_input.is_empty());
    }

    #[test]
    fn start_guidance_now_mode() {
        let mut state = TuiState::new();
        state.start_guidance(GuidanceMode::Now);
        assert_eq!(state.guidance_mode, Some(GuidanceMode::Now));
    }

    #[test]
    fn cancel_guidance_clears_state() {
        let mut state = TuiState::new();
        state.start_guidance(GuidanceMode::Next);
        state.guidance_input = "some text".to_string();
        state.cancel_guidance();
        assert!(state.guidance_mode.is_none());
        assert!(state.guidance_input.is_empty());
    }

    #[test]
    fn send_guidance_next_pushes_to_queue() {
        let mut state = TuiState::new();
        state.start_guidance(GuidanceMode::Next);
        state.guidance_input = "check auth.rs".to_string();
        assert!(state.send_guidance());
        assert!(state.guidance_mode.is_none());
        assert!(state.guidance_input.is_empty());

        let queue = state.guidance_next_queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0], "check auth.rs");
    }

    #[test]
    fn send_guidance_empty_input_cancels() {
        let mut state = TuiState::new();
        state.start_guidance(GuidanceMode::Next);
        state.guidance_input = "   ".to_string();
        assert!(!state.send_guidance());
        let queue = state.guidance_next_queue.lock().unwrap();
        assert!(queue.is_empty());
    }

    #[test]
    fn send_guidance_sets_flash() {
        let mut state = TuiState::new();
        state.start_guidance(GuidanceMode::Next);
        state.guidance_input = "test".to_string();
        state.send_guidance();
        assert!(state.guidance_flash.is_some());
        assert_eq!(
            state.active_guidance_flash(),
            Some((GuidanceMode::Next, GuidanceResult::Queued))
        );
    }

    #[test]
    fn send_guidance_now_writes_to_events_file() {
        let dir = tempfile::tempdir().unwrap();
        let events_path = dir.path().join("events.jsonl");
        let urgent_steer_path = dir.path().join("urgent-steer.json");

        let mut state = TuiState::new();
        state.events_path = Some(events_path.clone());
        state.urgent_steer_path = Some(urgent_steer_path.clone());
        state.start_guidance(GuidanceMode::Now);
        state.guidance_input = "fix the bug now".to_string();
        assert!(state.send_guidance());

        let content = std::fs::read_to_string(&events_path).unwrap();
        let event: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(event["topic"], "human.guidance");
        assert_eq!(event["payload"], "fix the bug now");
        assert!(event["ts"].is_string());

        let steer = ralph_core::UrgentSteerStore::new(urgent_steer_path)
            .load()
            .unwrap()
            .expect("urgent steer");
        assert_eq!(steer.messages, vec!["fix the bug now"]);
    }

    #[test]
    fn send_guidance_now_without_events_path_fails() {
        let mut state = TuiState::new();
        state.events_path = None;
        state.start_guidance(GuidanceMode::Now);
        state.guidance_input = "test".to_string();
        assert!(!state.send_guidance());
    }

    #[test]
    fn is_guidance_active_reflects_mode() {
        let mut state = TuiState::new();
        assert!(!state.is_guidance_active());
        state.start_guidance(GuidanceMode::Next);
        assert!(state.is_guidance_active());
        state.cancel_guidance();
        assert!(!state.is_guidance_active());
    }

    #[test]
    fn multiple_guidance_messages_queue_correctly() {
        let mut state = TuiState::new();

        state.start_guidance(GuidanceMode::Next);
        state.guidance_input = "first".to_string();
        state.send_guidance();

        state.start_guidance(GuidanceMode::Next);
        state.guidance_input = "second".to_string();
        state.send_guidance();

        let queue = state.guidance_next_queue.lock().unwrap();
        assert_eq!(queue.len(), 2);
        assert_eq!(queue[0], "first");
        assert_eq!(queue[1], "second");
    }

    #[test]
    fn task_start_preserves_guidance_queue() {
        let mut state = TuiState::new();
        state.start_new_iteration();

        // Queue some guidance
        state.start_guidance(GuidanceMode::Next);
        state.guidance_input = "remember this".to_string();
        state.send_guidance();

        // Simulate task.start reset
        let event = Event::new("task.start", "New task");
        state.update(&event);

        // Queue should be preserved (same Arc)
        let queue = state.guidance_next_queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0], "remember this");
    }
}

// ========================================================================
// Wave View Per-Iteration Tests
// ========================================================================

mod wave_view {
    use super::*;

    /// Simulates wave lifecycle: start a wave on the given state,
    /// then complete it, storing the data on the correct iteration.
    fn simulate_wave(state: &mut TuiState, worker_count: u32) {
        let iter_idx = state.iterations.len().saturating_sub(1);
        state.wave_active = Some(WaveInfo::new("TestHat".to_string(), worker_count));
        state.wave_active_iteration_idx = Some(iter_idx);
        // Add some content to worker buffers
        if let Some(ref wave) = state.wave_active {
            for (i, buf) in wave.worker_buffers.iter().enumerate() {
                let handle = buf.lines_handle();
                if let Ok(mut lines) = handle.lock() {
                    lines.push(Line::from(format!("Worker {} output", i + 1)));
                }
            }
        }
        // Complete the wave — move to iteration buffer
        let wave_iter_idx = state.wave_active_iteration_idx.take();
        if let Some(wave) = state.wave_active.take() {
            let target = wave_iter_idx.unwrap_or(0);
            if let Some(buf) = state.iterations.get_mut(target) {
                buf.wave_info = Some(wave);
            }
        }
    }

    #[test]
    fn wave_view_shows_correct_wave_for_historical_iteration() {
        let mut state = TuiState::new();

        // Iteration 1: wave with 5 workers
        state.start_new_iteration();
        simulate_wave(&mut state, 5);

        // Iteration 2: wave with 3 workers
        state.start_new_iteration();
        simulate_wave(&mut state, 3);

        // Navigate to iteration 1
        state.navigate_prev();
        assert_eq!(state.current_view, 0);

        // Press 'w' — should show iteration 1's wave (5 workers)
        state.enter_wave_view();
        assert!(state.wave_view_active);

        let wave = state.wave_info_for_wave_view().unwrap();
        assert_eq!(
            wave.total, 5,
            "Should show 5 workers from iteration 1, not 3"
        );
        assert_eq!(wave.worker_buffers.len(), 5);
    }

    #[test]
    fn wave_view_shows_active_wave_on_current_iteration() {
        let mut state = TuiState::new();

        // Iteration 1: completed wave with 5 workers
        state.start_new_iteration();
        simulate_wave(&mut state, 5);

        // Iteration 2: active wave with 3 workers (not completed)
        state.start_new_iteration();
        state.wave_active = Some(WaveInfo::new("ActiveHat".to_string(), 3));
        state.wave_active_iteration_idx = Some(1);

        // Viewing iteration 2 (latest) — should see active wave
        assert_eq!(state.current_view, 1);
        state.enter_wave_view();
        assert!(state.wave_view_active);

        let wave = state.wave_info_for_wave_view().unwrap();
        assert_eq!(wave.total, 3, "Should show active wave's 3 workers");
    }

    #[test]
    fn wave_view_ignores_active_wave_when_viewing_historical() {
        let mut state = TuiState::new();

        // Iteration 1: completed wave with 5 workers
        state.start_new_iteration();
        simulate_wave(&mut state, 5);

        // Iteration 2: active wave with 3 workers (not completed)
        state.start_new_iteration();
        state.wave_active = Some(WaveInfo::new("ActiveHat".to_string(), 3));
        state.wave_active_iteration_idx = Some(1);

        // Navigate back to iteration 1
        state.navigate_prev();
        assert_eq!(state.current_view, 0);

        // Press 'w' — should show iteration 1's completed wave, NOT the active wave
        state.enter_wave_view();
        assert!(state.wave_view_active);

        let wave = state.wave_info_for_wave_view().unwrap();
        assert_eq!(
            wave.total, 5,
            "Must show historical iteration's 5 workers, not active wave's 3"
        );
    }

    #[test]
    fn wave_view_no_op_on_iteration_without_wave() {
        let mut state = TuiState::new();

        // Iteration 1: has a wave
        state.start_new_iteration();
        simulate_wave(&mut state, 3);

        // Iteration 2: no wave
        state.start_new_iteration();

        // Viewing iteration 2 — pressing 'w' should be a no-op
        state.enter_wave_view();
        assert!(!state.wave_view_active);
    }

    #[test]
    fn wave_worker_navigation_uses_correct_wave() {
        let mut state = TuiState::new();

        // Iteration 1: wave with 5 workers
        state.start_new_iteration();
        simulate_wave(&mut state, 5);

        // Iteration 2: wave with 2 workers
        state.start_new_iteration();
        simulate_wave(&mut state, 2);

        // Navigate to iteration 1 and enter wave view
        state.navigate_prev();
        state.enter_wave_view();

        // Cycle through workers — should wrap at 5, not 2
        for i in 0..5 {
            assert_eq!(state.wave_view_index, i);
            state.wave_view_next();
        }
        // After 5 nexts, should wrap back to 0
        assert_eq!(state.wave_view_index, 0);
    }
}
