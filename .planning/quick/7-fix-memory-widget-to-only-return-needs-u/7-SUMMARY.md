---
phase: quick-7
plan: 01
subsystem: memory-widget
tags: [performance, idle-cpu, memory]
key-files:
  modified:
    - src/memory_graph.rs
    - src/widgets/memory.rs
decisions:
  - "Still push samples when unchanged to maintain graph timeline accuracy, but return false to skip redraw"
metrics:
  duration: "26s"
  completed: "2026-03-12T21:31:27Z"
  tasks_completed: 1
  tasks_total: 1
---

# Quick Task 7: Fix Memory Widget Unnecessary Wakeups Summary

Change detection in maybe_sample() to skip redraws when memory usage unchanged, plus needs_faster_refresh() returns false since the widget uses its own sample timer.

## What Changed

### Task 1: Add change detection and fix needs_faster_refresh

**Commit:** 4167ef6

1. **`src/memory_graph.rs` - `maybe_sample()`**: Added comparison of new sample against `self.samples.back()`. The sample is always pushed (to maintain graph timeline), but the method now returns `false` when the value is unchanged, preventing unnecessary redraws.

2. **`src/widgets/memory.rs` - `needs_faster_refresh()`**: Changed from `true` to `false`. The memory widget has its own internal timer (`sample_interval_ms`) managed inside `maybe_sample()`, so it does not need the main loop to force 1-second wakeups.

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo build --release` succeeds
- `needs_faster_refresh()` returns `false`
- `maybe_sample()` compares new sample against previous before deciding return value

## Self-Check: PASSED
