---
phase: quick-6
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - src/main.rs
  - src/widgets/battery.rs
  - src/widgets/temperature.rs
  - src/widgets/load.rs
autonomous: true
requirements: [PERF-IDLE-CPU]

must_haves:
  truths:
    - "Epoll timeout floor is 1ms (not 50ms), so idle loop runs at ~2 Hz blink rate instead of ~20 Hz"
    - "Battery widget only triggers redraw when capacity or charge state actually changes"
    - "Temperature widget only triggers redraw when displayed temperature string changes"
    - "Load average widget only triggers redraw when displayed load string changes"
    - "Render pipeline (iced render + rotation + DRM dirty) is skipped when no widget reported a change and blink did not toggle"
  artifacts:
    - path: "src/main.rs"
      provides: "Fixed epoll timeout floor and render gating"
      contains: ".max(1)"
    - path: "src/widgets/battery.rs"
      provides: "Change-detection in BatteryWidget::update()"
      contains: "last_capacity"
    - path: "src/widgets/temperature.rs"
      provides: "Change-detection in TemperatureWidget::update()"
      contains: "last_reading"
    - path: "src/widgets/load.rs"
      provides: "Change-detection in LoadAvgWidget::update()"
      contains: "last_reading"
  key_links:
    - from: "src/widgets/battery.rs"
      to: "src/main.rs"
      via: "update() return value gates needs_redraw"
      pattern: "w\\.update\\(\\)"
    - from: "src/main.rs"
      to: "display::DrmBackend"
      via: "needs_redraw boolean gates drm.map + drm.dirty"
      pattern: "if needs_redraw"
---

<objective>
Reduce idle CPU usage from ~22% to near-zero by fixing three root causes: the 50ms epoll timeout floor that causes ~20 wakeups/sec, widget update() methods that always return true (forcing sysfs I/O + full re-render every wake), and the render pipeline running even when nothing changed.

Purpose: Eliminate unnecessary CPU usage that drains battery on Apple Silicon laptops with touchbar.
Output: Modified main.rs and three widget files with proper change detection and render gating.
</objective>

<execution_context>
@/home/agent/.claude/get-shit-done/workflows/execute-plan.md
@/home/agent/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@src/main.rs
@src/widgets/mod.rs
@src/widgets/battery.rs
@src/widgets/temperature.rs
@src/widgets/load.rs
@src/layer_manager.rs

<interfaces>
From src/widgets/mod.rs:
```rust
pub trait Widget {
    fn render(&self, ctx: &RenderContext) -> Element<'_, Message, Theme, IcedRenderer>;
    fn update(&mut self) -> bool;  // Returns true if state changed (triggers redraw)
    fn width_fraction(&self) -> f64;
    fn poll(&mut self) -> bool { false }
    fn handle_event(&mut self, action: WidgetAction) -> Vec<MainLoopAction>;
    fn needs_faster_refresh(&self) -> bool { false }
    // ...
}
```

From src/layer_manager.rs:
```rust
pub fn update(&mut self) -> bool {
    // Calls update() on all active widgets, returns true if any changed
    self.layers[self.active_layer]
        .iter_mut()
        .fold(false, |changed, w| w.update() || changed)
}
```
</interfaces>
</context>

<tasks>

<task type="auto">
  <name>Task 1: Add change detection to battery, temperature, and load widgets</name>
  <files>src/widgets/battery.rs, src/widgets/temperature.rs, src/widgets/load.rs</files>
  <action>
All three widgets currently always return `true` from `update()`, causing unnecessary redraws. Add previous-value caching so `update()` only returns `true` when the displayed value actually changes.

**battery.rs:**
- Add fields `last_capacity: Option<u32>` and `last_state: Option<BatteryState>` to `BatteryWidget` struct
- Initialize both to `None` in `try_new()`
- In `update()`: call `get_battery_state()`, compare `(capacity, state)` against `(last_capacity, last_state)`. Only return `true` if they differ (or on first read). Update cached values. Keep the existing battery_failed log-once pattern.

**temperature.rs:**
- Add field `last_reading: String` to `TemperatureWidget` struct, initialized to `String::new()`
- In `update()`: call `get_temperature()`, compare against `last_reading`. Only return `true` if different. Update `last_reading`. Keep the thermal_failed log-once pattern.

**load.rs:**
- Add field `last_reading: String` to `LoadAvgWidget` struct, initialized to `String::new()`
- In `update()`: call `get_load_avg()`, compare against `last_reading`. Only return `true` if different. Update `last_reading`. Keep the load_avg_failed log-once pattern.

Remove the "Always return true" comments from all three files.
  </action>
  <verify>
    <automated>cd /home/agent/dev/tiny-dfr && cargo build --release 2>&1</automated>
  </verify>
  <done>All three widget update() methods compare current reading against cached previous value and only return true when the value actually changed. No more unconditional `true` returns.</done>
</task>

<task type="auto">
  <name>Task 2: Fix epoll timeout floor and gate render pipeline on actual changes</name>
  <files>src/main.rs</files>
  <action>
Two changes in main.rs:

**1. Fix the epoll timeout floor (line 280):**
Change:
```rust
(500 - now_i.duration_since(last_blink).as_millis() as i32).max(50)
```
To:
```rust
(500 - now_i.duration_since(last_blink).as_millis() as i32).max(1)
```
This changes the minimum epoll sleep from 50ms to 1ms. In practice the blink timer will naturally produce ~500ms sleeps when idle (since the blink interval is 500ms and nothing else wakes the loop). The `.max(1)` just prevents negative/zero values from making epoll spin.

**2. The render gating is already correct** -- the existing `if needs_redraw { ... }` block on line 296 already gates the render pipeline. With Task 1 fixing widget update() to return false when nothing changed, `layer_mgr.update()` on line 292 will now correctly return false at idle, so `needs_redraw` stays false and the render pipeline is skipped. No additional gating logic needed.

Verify the blink toggle on line 273-276 still sets `needs_redraw = true` only when blink actually toggles (it already does -- the 500ms check is correct).
  </action>
  <verify>
    <automated>cd /home/agent/dev/tiny-dfr && cargo build --release 2>&1</automated>
  </verify>
  <done>Epoll timeout floor changed from 50ms to 1ms. At idle with no state changes, the loop sleeps ~500ms between blink toggles, widgets return false from update(), and the render pipeline is skipped -- reducing CPU from ~22% to near-zero.</done>
</task>

</tasks>

<verification>
1. `cargo build --release` compiles without errors or warnings
2. Manual verification on hardware: run the binary and observe CPU usage with `top` or `htop` -- idle CPU should drop from ~22% to under 2%
3. Blink animation (low battery indicator) still toggles every 500ms when battery is low
4. Touch interactions still trigger immediate redraws
5. Time display still updates on minute/second boundaries
</verification>

<success_criteria>
- All three widget update() methods implement change detection (compare against cached previous value)
- Epoll timeout floor is 1ms instead of 50ms
- `cargo build --release` succeeds
- At idle, the event loop sleeps ~500ms between iterations (blink period) instead of ~50ms
</success_criteria>

<output>
After completion, create `.planning/quick/6-we-need-to-address-the-performance-bar-a/6-01-SUMMARY.md`
</output>
