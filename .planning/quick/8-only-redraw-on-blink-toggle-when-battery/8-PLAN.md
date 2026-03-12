---
phase: quick-8
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - src/widgets/mod.rs
  - src/widgets/battery.rs
  - src/layer_manager.rs
  - src/main.rs
autonomous: true
requirements: [QUICK-8]
must_haves:
  truths:
    - "Blink toggle no longer triggers redraw when battery is not low"
    - "Blink toggle still triggers redraw when battery IS low (blinking icon)"
    - "Blink timer keeps running so low-battery blink activates immediately"
  artifacts:
    - path: "src/widgets/mod.rs"
      provides: "needs_blink() default method on Widget trait"
      contains: "fn needs_blink"
    - path: "src/widgets/battery.rs"
      provides: "needs_blink() override returning true when BatteryState::Low"
      contains: "fn needs_blink"
    - path: "src/layer_manager.rs"
      provides: "needs_blink() aggregation method"
      contains: "fn needs_blink"
    - path: "src/main.rs"
      provides: "Conditional redraw on blink toggle"
      contains: "needs_blink"
  key_links:
    - from: "src/main.rs"
      to: "src/layer_manager.rs"
      via: "layer_mgr.needs_blink()"
      pattern: "layer_mgr\\.needs_blink\\(\\)"
    - from: "src/layer_manager.rs"
      to: "src/widgets/mod.rs"
      via: "Widget::needs_blink() trait method"
      pattern: "w\\.needs_blink\\(\\)"
---

<objective>
Eliminate 2 unnecessary full renders per second at normal battery levels by only
redrawing on blink toggle when a widget actually uses the blink state.

Purpose: Currently the blink toggle at main.rs:273-276 unconditionally sets
needs_redraw=true every 500ms, but only BatteryWidget uses blink_on and only
when BatteryState::Low. This wastes CPU on every other state.

Output: Conditional blink redraw gated by a new `needs_blink()` trait method.
</objective>

<execution_context>
@/home/agent/.claude/get-shit-done/workflows/execute-plan.md
@/home/agent/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@src/widgets/mod.rs (Widget trait with existing needs_faster_refresh() pattern)
@src/widgets/battery.rs (BatteryWidget with last_state tracking)
@src/layer_manager.rs (aggregation methods like needs_faster_refresh())
@src/main.rs (blink toggle at lines 272-277)
</context>

<tasks>

<task type="auto">
  <name>Task 1: Add needs_blink() to Widget trait and implement in BatteryWidget</name>
  <files>src/widgets/mod.rs, src/widgets/battery.rs</files>
  <action>
1. In src/widgets/mod.rs, add a default method to the Widget trait, right after the
   existing `needs_faster_refresh()` method:

   ```rust
   /// Whether this widget currently needs blink redraws (e.g., low battery blinking).
   fn needs_blink(&self) -> bool {
       false
   }
   ```

2. In src/widgets/battery.rs, implement `needs_blink()` for BatteryWidget. It should
   return true when `self.last_state == Some(BatteryState::Low)`. This uses the
   already-tracked `last_state` field (set by `update()`), so no extra sysfs reads.

   ```rust
   fn needs_blink(&self) -> bool {
       self.last_state == Some(BatteryState::Low)
   }
   ```
  </action>
  <verify>
    <automated>cd /home/agent/dev/tiny-dfr && cargo check 2>&1 | tail -5</automated>
  </verify>
  <done>Widget trait has needs_blink() with default false; BatteryWidget returns true when Low</done>
</task>

<task type="auto">
  <name>Task 2: Add LayerManager aggregation and gate blink redraw in main loop</name>
  <files>src/layer_manager.rs, src/main.rs</files>
  <action>
1. In src/layer_manager.rs, add a `needs_blink()` method following the exact same
   pattern as the existing `needs_faster_refresh()` method (line 149-153):

   ```rust
   /// Whether any active widget needs blink redraws.
   pub fn needs_blink(&self) -> bool {
       self.layers[self.active_layer]
           .iter()
           .any(|w| w.needs_blink())
   }
   ```

2. In src/main.rs, change the blink toggle block (lines 272-277). Currently:

   ```rust
   if now_i.duration_since(last_blink).as_millis() >= 500 {
       blink_on = !blink_on;
       last_blink = now_i;
       needs_redraw = true;
   }
   ```

   Change to conditionally redraw:

   ```rust
   if now_i.duration_since(last_blink).as_millis() >= 500 {
       blink_on = !blink_on;
       last_blink = now_i;
       if layer_mgr.needs_blink() {
           needs_redraw = true;
       }
   }
   ```

   The blink timer still toggles every 500ms (so blink activates immediately when
   battery goes low), but redraw only happens when a widget actually uses blink.
  </action>
  <verify>
    <automated>cd /home/agent/dev/tiny-dfr && cargo build --release 2>&1 | tail -5</automated>
  </verify>
  <done>Blink toggle only triggers redraw when layer_mgr.needs_blink() is true; builds clean</done>
</task>

</tasks>

<verification>
- `cargo build --release` succeeds with no warnings related to changes
- `cargo fmt --check` passes
- Grep confirms: main.rs blink block has `if layer_mgr.needs_blink()` guard
- Grep confirms: Widget trait has `fn needs_blink` with default false
- Grep confirms: BatteryWidget overrides needs_blink checking for Low state
</verification>

<success_criteria>
- At normal battery levels, blink toggle no longer triggers needs_redraw (0 unnecessary redraws/sec saved)
- At low battery, blink still works identically (icon blinks on/off every 500ms)
- Blink timer keeps running unconditionally so transition to Low activates blinking within 500ms
</success_criteria>

<output>
After completion, create `.planning/quick/8-only-redraw-on-blink-toggle-when-battery/8-SUMMARY.md`
</output>
