---
phase: quick-8
plan: 01
subsystem: rendering
tags: [performance, blink, battery]
dependency_graph:
  requires: []
  provides: [conditional-blink-redraw]
  affects: [main-loop, widget-trait]
tech_stack:
  added: []
  patterns: [trait-default-method, aggregation-delegation]
key_files:
  created: []
  modified:
    - src/widgets/mod.rs
    - src/widgets/battery.rs
    - src/layer_manager.rs
    - src/main.rs
decisions:
  - "Used trait default method pattern matching existing needs_faster_refresh() convention"
  - "Blink timer keeps running unconditionally to ensure immediate activation on Low transition"
metrics:
  duration: "1 minute"
  completed: "2026-03-12"
---

# Quick Task 8: Only Redraw on Blink Toggle When Battery Low

Conditional blink redraw gated by needs_blink() trait method, eliminating 2 unnecessary full renders per second at normal battery levels.

## What Changed

### Task 1: Add needs_blink() to Widget trait and BatteryWidget
- **Commit:** 42a72a3
- Added `needs_blink()` default method (returns `false`) to `Widget` trait in `src/widgets/mod.rs`
- Overrode in `BatteryWidget` to return `true` when `self.last_state == Some(BatteryState::Low)`
- Uses already-tracked `last_state` field -- no extra sysfs reads

### Task 2: Add LayerManager aggregation and gate blink redraw in main loop
- **Commit:** c128754
- Added `needs_blink()` aggregation method to `LayerManager` following same pattern as `needs_faster_refresh()`
- Changed blink toggle block in `src/main.rs` to only set `needs_redraw = true` when `layer_mgr.needs_blink()` returns true
- Blink timer still toggles every 500ms unconditionally so transition to Low activates blinking within 500ms

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo build --release` succeeds (warnings are pre-existing, unrelated)
- `cargo fmt --check` passes
- Grep confirms all needs_blink() implementations in correct locations

## Self-Check: PASSED

All modified files exist and both commits verified.
