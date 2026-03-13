---
phase: quick-12
description: "Fix main loop spinning: drain inactive layer fds and remove unconditional time redraw"
date: 2026-03-13
---

# Quick Task 12: Fix main loop spinning

## Root Cause

Widget eventfds from BOTH layers are registered with epoll, but `LayerManager::poll()` only drains the ACTIVE layer's fds. When the inactive layer's workspace/volume widget reader threads signal their eventfds (on every niri/PA event), those fds remain readable, causing `epoll.wait()` to return immediately — spinning the main loop at 100% CPU.

Secondary: The time-based redraw (`main.rs:267-269`) unconditionally sets `needs_redraw = true` every second even when content is unchanged, triggering a full iced render pipeline needlessly.

## Plan 01: Fix fd drainage and time redraw

### Task 1: Drain ALL layer widget fds in poll()

**Files:** `src/layer_manager.rs`
**Action:** Add `drain_all()` method that calls `poll()` on all layers' widgets but only returns change status for the active layer. Or simpler: change `poll()` to drain all layers.
**Verify:** `cargo build --release`
**Done:** Inactive layer fds are drained, epoll.wait() blocks properly.

### Task 2: Make time widget report changes via update()

**Files:** `src/widgets/time.rs`, `src/main.rs`
**Action:**
- In `time.rs`: add `last_rendered` field tracking the last formatted string; `update()` re-formats and returns true only when output changed.
- In `main.rs`: remove the unconditional `needs_redraw = true` from the time-based refresh check (lines 261-270). The time widget's `update()` now handles this.
- Keep the `needs_faster_refresh()` mechanism for timeout calculation only (so the loop wakes up at the right frequency), but don't force a redraw.
**Verify:** `cargo build --release`
**Done:** No unconditional redraws from time check.

### Task 3: Remove debug eprintln statements

**Files:** `src/main.rs`
**Action:** Remove `eprintln!("Redraw: {:?}", ...)` and `eprintln!("loop: {:?}", timeout)` debug lines.
**Verify:** `cargo build --release`
**Done:** No debug spam in production.
