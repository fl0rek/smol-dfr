---
phase: quick-7
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - src/memory_graph.rs
  - src/widgets/memory.rs
autonomous: true
requirements: [QUICK-7]
must_haves:
  truths:
    - "MemoryWidget::needs_faster_refresh() returns false so it does not force 1-second wakeups"
    - "MemoryHistory::maybe_sample() returns false when the new sample equals the previous sample"
    - "maybe_sample() still returns true when the sampled value differs from the last"
  artifacts:
    - path: "src/widgets/memory.rs"
      provides: "needs_faster_refresh returns false"
      contains: "fn needs_faster_refresh"
    - path: "src/memory_graph.rs"
      provides: "Change detection in maybe_sample"
      contains: "maybe_sample"
  key_links:
    - from: "src/memory_graph.rs"
      to: "src/widgets/memory.rs"
      via: "maybe_sample() return value drives update() return value"
      pattern: "self\\.history\\.maybe_sample\\(\\)"
---

<objective>
Fix the memory widget to avoid unnecessary wakeups and redraws.

Purpose: Currently the memory widget forces the main loop into 1-second polling mode via needs_faster_refresh() returning true, and triggers redraws even when memory usage has not changed. Both waste CPU.

Output: Two small edits that eliminate unnecessary wakeups and redraws from the memory widget.
</objective>

<execution_context>
@/home/agent/.claude/get-shit-done/workflows/execute-plan.md
@/home/agent/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@src/widgets/memory.rs
@src/memory_graph.rs
@src/main.rs (lines 230-295 for timeout/refresh logic)
</context>

<tasks>

<task type="auto">
  <name>Task 1: Add change detection to maybe_sample and fix needs_faster_refresh</name>
  <files>src/memory_graph.rs, src/widgets/memory.rs</files>
  <action>
Two changes:

1. In `src/memory_graph.rs`, modify `maybe_sample()` to compare the new sample against the last sample in the deque. Only return `true` if the value actually changed. Specifically:
   - After computing `let usage = get_memory_usage();`, check if `self.samples.back() == Some(&usage)`. If so, update `self.last_sample = now` (to keep the timer ticking) and push the sample (to maintain the graph timeline), but return `false` since nothing visually changed.
   - If the value differs (or the deque was empty), push the sample and return `true` as before.
   - The key insight: we still push even when unchanged (to maintain correct graph window sizing), but we signal "no visual change" by returning false.

   Wait -- actually, if we always push the same value, the graph still has the same shape. The samples deque length growing does change max_samples boundary behavior, but once full it stays full. So pushing is correct for timeline accuracy, returning false is correct for redraw avoidance.

2. In `src/widgets/memory.rs`, change `needs_faster_refresh()` to return `false`. The memory widget's refresh is driven by its own `sample_interval_ms` timer inside `maybe_sample()`, not by the main loop's second-tick mechanism. The main loop already calls `layer_mgr.update()` on every iteration which calls `maybe_sample()`, so the sample will be taken whenever the epoll timeout fires. The memory widget does NOT need the main loop to wake every second -- it only needs wakeups at its own sample interval (already handled by the existing timeout logic with blink at 500ms and minute boundary).
  </action>
  <verify>
    <automated>cd /home/agent/dev/tiny-dfr && cargo build --release 2>&1 | tail -5</automated>
  </verify>
  <done>
    - needs_faster_refresh() returns false
    - maybe_sample() returns false when new sample equals previous sample
    - maybe_sample() returns true when new sample differs from previous sample
    - maybe_sample() still pushes samples on every interval tick (unchanged or not) to maintain graph timeline
    - Build succeeds with no warnings
  </done>
</task>

</tasks>

<verification>
- `cargo build --release` succeeds
- `grep -n "needs_faster_refresh" src/widgets/memory.rs` shows it returns false
- `grep -A5 "let usage = get_memory_usage" src/memory_graph.rs` shows change detection logic
</verification>

<success_criteria>
The memory widget no longer forces 1-second wakeups and only triggers redraws when memory usage actually changes between samples.
</success_criteria>

<output>
After completion, create `.planning/quick/7-fix-memory-widget-to-only-return-needs-u/7-SUMMARY.md`
</output>
