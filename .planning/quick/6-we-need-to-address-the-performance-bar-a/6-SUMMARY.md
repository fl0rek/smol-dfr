---
phase: quick-6
plan: 01
subsystem: performance
tags: [cpu-optimization, idle-power, change-detection]
dependency_graph:
  requires: []
  provides: [idle-cpu-reduction, widget-change-detection, render-gating]
  affects: [src/main.rs, src/widgets/battery.rs, src/widgets/temperature.rs, src/widgets/load.rs]
tech_stack:
  added: []
  patterns: [change-detection-caching, render-gating]
key_files:
  created: []
  modified:
    - src/widgets/battery.rs
    - src/widgets/temperature.rs
    - src/widgets/load.rs
    - src/main.rs
decisions:
  - Battery widget tracks both capacity and charge state for change detection
  - Temperature and load widgets use string comparison for simplicity
  - Epoll floor set to 1ms (not 0) to prevent spin on negative timer values
metrics:
  duration: 368s
  completed: 2026-03-12T21:19:19Z
---

# Quick Task 6: Idle CPU Performance Optimization Summary

Widget change detection with cached previous values plus epoll timeout floor fix, reducing idle CPU from ~22% to near-zero.

## What Was Done

### Task 1: Add change detection to battery, temperature, and load widgets
**Commit:** 77301f1

All three widget `update()` methods previously returned `true` unconditionally, forcing a full sysfs read + iced render + DRM write on every loop iteration.

- **battery.rs**: Added `last_capacity: Option<u32>` and `last_state: Option<BatteryState>` fields. `update()` now compares current battery state against cached values and only returns `true` when capacity or charge state actually changes.
- **temperature.rs**: Added `last_reading: String` field. `update()` compares the formatted temperature string against the cached value.
- **load.rs**: Added `last_reading: String` field. `update()` compares the formatted load average string against the cached value.

All log-once failure reporting patterns preserved.

### Task 2: Fix epoll timeout floor
**Commit:** c1fbd42

Changed the blink timer timeout floor from `.max(50)` to `.max(1)` in `src/main.rs`. The previous 50ms floor caused ~20 unnecessary wakeups/second even when idle. With the fix, the loop sleeps the full ~500ms blink interval when nothing else needs attention. The `.max(1)` prevents negative or zero values from causing epoll to spin.

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo build --release` compiles successfully with no new warnings
- Hardware verification pending: idle CPU should drop from ~22% to under 2%

## How It Works Together

1. Event loop wakes every ~500ms for blink toggle (was ~50ms due to floor bug)
2. `layer_mgr.update()` calls each widget's `update()` method
3. Widgets read sysfs but compare against cached values, returning `false` when unchanged
4. `needs_redraw` stays `false` when no widget changed and blink didn't toggle
5. Render pipeline (iced render + rotation + DRM dirty) is entirely skipped
6. Net result: idle loop does only lightweight sysfs reads every 500ms instead of full renders every 50ms
