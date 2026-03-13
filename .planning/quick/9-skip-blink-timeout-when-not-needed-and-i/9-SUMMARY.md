---
phase: quick-9
plan: 01
subsystem: main-loop
tags: [performance, idle, blink]
key-files:
  modified: [src/main.rs]
decisions:
  - "Keep now_i outside guard since battery_time_until block needs it"
metrics:
  duration: 37s
  completed: 2026-03-13
---

# Quick Task 9: Skip Blink Timeout When Not Needed

Conditional blink guard eliminates 500ms wakeup cycle when nothing needs blinking.

## What Changed

Wrapped both the blink toggle and the timeout reduction in an `if layer_mgr.needs_blink()` guard. Previously, the main loop always clamped epoll timeout to 500ms for blink cycling, even when no widget was blinking. Now, when `needs_blink()` returns false, the loop can sleep up to `TIMEOUT_MS` (10s) or the next minute boundary, significantly reducing idle CPU wakeups.

## Tasks Completed

| # | Task | Commit | Files |
|---|------|--------|-------|
| 1 | Wrap blink toggle and timeout in needs_blink() guard | dce261e | src/main.rs |

## Deviations from Plan

None - plan executed exactly as written.

## Verification

- `cargo build --release` compiles without errors
- Blink block fully wrapped in `if layer_mgr.needs_blink()`
- `now_i` remains available for battery_time_until block below

## Self-Check: PASSED
