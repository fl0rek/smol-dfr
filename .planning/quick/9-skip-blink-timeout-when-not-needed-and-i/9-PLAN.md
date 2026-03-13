---
phase: quick-9
plan: 01
type: execute
wave: 1
depends_on: []
files_modified: [src/main.rs]
autonomous: true
requirements: [QUICK-9]

must_haves:
  truths:
    - "When needs_blink() is false, epoll timeout is NOT reduced to 500ms"
    - "When needs_blink() is true, blink toggle and 500ms timeout work as before"
  artifacts:
    - path: "src/main.rs"
      provides: "Conditional blink block"
      contains: "if layer_mgr.needs_blink()"
  key_links:
    - from: "src/main.rs blink block"
      to: "layer_mgr.needs_blink()"
      via: "conditional guard wrapping both toggle and timeout"
      pattern: "if layer_mgr\\.needs_blink\\(\\)"
---

<objective>
Skip blink timeout when not needed to reduce unnecessary wakeups.

Purpose: When nothing needs blinking (normal battery), the loop currently wakes every 500ms unnecessarily. Wrapping the entire blink block in a needs_blink() guard lets the loop sleep up to TIMEOUT_MS (10s) or the next minute boundary at idle.
Output: Modified src/main.rs with conditional blink block.
</objective>

<execution_context>
@/home/agent/.claude/get-shit-done/workflows/execute-plan.md
@/home/agent/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@src/main.rs (lines 272-283)
</context>

<tasks>

<task type="auto">
  <name>Task 1: Wrap blink toggle and timeout in needs_blink() guard</name>
  <files>src/main.rs</files>
  <action>
In src/main.rs, replace lines 272-283 (the blink block) with:

```rust
if layer_mgr.needs_blink() {
    let now_i = Instant::now();
    if now_i.duration_since(last_blink).as_millis() >= 500 {
        blink_on = !blink_on;
        last_blink = now_i;
        needs_redraw = true;
    }
    timeout = min(
        timeout,
        (500 - now_i.duration_since(last_blink).as_millis() as i32).max(1),
    );
}
```

Key changes:
1. The outer `if layer_mgr.needs_blink()` wraps BOTH the toggle and the timeout reduction.
2. Inside the guard, the inner `if layer_mgr.needs_blink()` check around `needs_redraw = true` is no longer needed since we already checked -- just set `needs_redraw = true` directly.
3. When needs_blink() is false, `now_i` is not computed here (it may still be computed elsewhere if needed -- check if `now_i` is used by the battery_time_until block below on line 285. If so, move `let now_i = Instant::now();` BEFORE the needs_blink guard so it remains available for the battery block).

IMPORTANT: Check lines 285-292 for `now_i` usage. The battery_time_until block uses `now_i`. So `let now_i = Instant::now();` must remain OUTSIDE and BEFORE the needs_blink() guard. The corrected code is:

```rust
let now_i = Instant::now();
if layer_mgr.needs_blink() {
    if now_i.duration_since(last_blink).as_millis() >= 500 {
        blink_on = !blink_on;
        last_blink = now_i;
        needs_redraw = true;
    }
    timeout = min(
        timeout,
        (500 - now_i.duration_since(last_blink).as_millis() as i32).max(1),
    );
}
```

Run `cargo fmt` after editing.
  </action>
  <verify>
    <automated>cd /home/agent/dev/tiny-dfr && cargo build --release 2>&1 | tail -5</automated>
  </verify>
  <done>Blink toggle and timeout reduction only execute when layer_mgr.needs_blink() is true. Build succeeds.</done>
</task>

</tasks>

<verification>
- `cargo build --release` compiles without errors
- The blink block is fully wrapped in `if layer_mgr.needs_blink()`
- `now_i` remains available for the battery_time_until block below
</verification>

<success_criteria>
When needs_blink() returns false, the epoll timeout is no longer clamped to 500ms, allowing the loop to sleep longer at idle.
</success_criteria>

<output>
After completion, create `.planning/quick/9-skip-blink-timeout-when-not-needed-and-i/9-SUMMARY.md`
</output>
