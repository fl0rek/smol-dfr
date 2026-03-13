# Quick Task 12: Fix main loop spinning

## Root Cause
Widget eventfds from both layers registered with epoll, but only active layer drained in `poll()`. Inactive layer's workspace/volume reader threads kept signaling their eventfds → epoll.wait() returned immediately every iteration → 100% CPU spin.

## Changes
- **`src/layer_manager.rs`**: `poll()` now drains ALL layers' widget fds, only reporting changes from active layer
- **`src/widgets/time.rs`**: Time widget now self-manages redraws via `update()` with change detection (cached `last_formatted` string)
- **`src/main.rs`**: Removed unconditional time-based redraw (was forcing full render every second). Kept timeout shortening for faster-refresh widgets. Removed debug eprintln statements.

## Commit
- `37eb64a`: fix(quick-12): drain inactive layer fds to stop main loop spinning
