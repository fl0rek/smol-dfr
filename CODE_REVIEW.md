# Code Review: tiny-dfr Cleanup Issues

Post-rewrite cleanup audit. Each issue is independent and can be addressed one-by-one.

---

## 1. Duplicated `eventfd` / `signal_fd` / `drain_eventfd` Helpers

**Files:** `src/volume.rs:34-53`, `src/workspace/niri.rs:24-50`

Both modules define identical `create_eventfd()`, `signal_fd()`/`signal_eventfd()`, and `drain_eventfd()` functions. The niri module has *three* variants: `signal_eventfd(&OwnedFd)`, `signal_eventfd_raw(i32)`, and `drain_eventfd(&OwnedFd)`.

**Fix:** Extract a shared `eventfd` utility module. Consolidate `signal_eventfd` and `signal_eventfd_raw` into one function.
DONE

---

## 2. Duplicated Hex Color Parsing

**Files:** `src/config.rs:13-22` (`parse_color_str`) and `src/config.rs:142-162` (`parse_hex_color`)

Two nearly identical hex color parsing implementations. `parse_color_str` returns `Option<(f64,f64,f64)>`, `parse_hex_color` is a serde deserializer that does the same thing internally.

**Fix:** Have `parse_hex_color` call `parse_color_str` internally instead of reimplementing the parsing.

---

## 3. Duplicated `IcedRenderer` Type Alias

**Files:** `src/widgets/mod.rs:17`, `src/iced_renderer.rs:16`

Both define `type IcedRenderer = iced_tiny_skia::Renderer;`. The widgets module defines its own instead of re-exporting from `iced_renderer`.

**Fix:** Define once, re-export.

---

## 4. Duplicated Widget Render Boilerplate

**Files:** `src/widgets/static_button.rs`, `src/widgets/time.rs`, `src/widgets/temperature.rs`, `src/widgets/load.rs`, `src/widgets/window_title.rs`, `src/widgets/battery.rs`

Nearly identical render patterns: `container(text(...).font().size().color(WHITE).align_x().align_y().width(Fill).height(Fill)).width(Fill).height(Fill).padding(2).style(...)`. This ~10 line block is repeated 8+ times across widgets.

**Fix:** Extract a helper function like `styled_text_container(label, ctx, color, active)` that produces the common pattern.

---

## 5. Duplicated `handle_event` Press/Release Pattern

**Files:** `src/widgets/static_button.rs:133-147`, `src/widgets/time.rs:123-137`

Identical press/release key-sending logic with active state toggle. The `BatteryWidget` has a minor variation.

**Fix:** Extract a default implementation or helper for the common key-action press/release pattern.

---

## 6. Duplicated Rate-Limiting / `refresh_if_needed` Pattern

**Files:** `src/widgets/battery.rs:172-186`, `src/widgets/temperature.rs:77-86`, `src/widgets/load.rs:40-49`

Three widgets implement the same pattern: check if `last_sysfs_read` was >1s ago, read sysfs, update `last_sysfs_read`. The `MemoryHistory` uses a similar but slightly different approach.

**Fix:** Extract a `RateLimitedReader` or similar utility.

---

## 7. Duplicated Log-Once Pattern

**Files:** `src/widgets/battery.rs:289-296`, `src/widgets/temperature.rs:115-123`, `src/widgets/load.rs:77-85`

Three widgets use the same "report failure/recovery via log-once" pattern with a `*_failed` bool.

**Fix:** Extract into a small `LogOnce` struct with `check(ok: bool, fail_msg, recover_msg)`.

---

## 8. Memory Leak: Font Family String

**File:** `src/iced_renderer.rs:51`

```rust
Family::Name(font_family.to_string().leak())
```

The font family string is intentionally leaked. While this is common for `'static` requirements, it leaks on every config reload (each `TouchbarRenderer::new()` call leaks a new `String`).

**Fix:** Use a `once_cell` or `Box::leak` only once, or use a static buffer. Alternatively, cache the leaked string and reuse it if the font family hasn't changed.

---

## 9. Vestigial `_battery_mode` Parameter

**File:** `src/widgets/battery.rs:150`

```rust
pub fn try_new(_battery_mode: &str, ...) -> Option<Self> {
```

The `mode` field from config is passed in but completely ignored (prefixed with `_`). The config still has `mode: String` in `WidgetConfig::Battery`.

**Fix:** Either implement mode-based behavior or remove the parameter and config field.

---

## 10. Vestigial `theme` Field in Icon Config

**File:** `src/config.rs:174`

```rust
Icon { icon: String, theme: Option<String> }
```

In `src/widgets/mod.rs:195`:
```rust
WidgetConfig::Icon { icon, theme: _ } => { ... }
```

The `theme` field is parsed from config but explicitly ignored.

**Fix:** Either implement theme support or remove the field.

---

## 11. Vestigial `_provider` Parameter in WorkspaceManager

**File:** `src/workspace/mod.rs:50`

```rust
pub fn new(_provider: Option<&str>) -> Self {
    // Niri is the only supported compositor; always create NiriBackend.
```

The `provider` field from `WorkspacesConfig` is accepted but ignored. The comment acknowledges this.

**Fix:** Remove the parameter or implement provider selection.

---

## 12. `pixel_shift` Computed but Never Applied

**File:** `src/main.rs:249-255`, `src/pixel_shift.rs:96-106`

`PixelShiftManager` has an `update()` method that returns timing info and a `get()` method that returns `(x_offset, y_offset)`, but `get()` is never called in `main.rs`. The pixel shift offsets are calculated but never applied to the rendering. The `update()` return value only controls timing/redraw.

**Fix:** Either apply `pixel_shift.get()` offsets to the render pipeline or remove the feature entirely.

---

## 13. `has_reconnect_flash()` Never Called

**Files:** `src/widgets/workspace.rs:41-43`, `src/widgets/volume.rs:39-41`

Both `WorkspaceWidget::has_reconnect_flash()` and `VolumeWidget::has_reconnect_flash()` are defined as public methods but never called anywhere. The underlying `manager.has_reconnect_flash()` is also never consumed.

**Fix:** Either use the flash flag to trigger a visual indication or remove these methods.

---

## 14. `sample_interval_ms()` Never Called

**File:** `src/widgets/memory.rs:35-37`

`MemoryWidget::sample_interval_ms()` is defined but never called anywhere.

**Fix:** Remove or use it for main loop timeout calculation.

---

## 15. Suspicious `MemoryWidget::needs_faster_refresh()` Always Returns False

**File:** `src/widgets/memory.rs:91-93`

```rust
fn needs_faster_refresh(&self) -> bool { false }
```

This explicitly overrides the default (which already returns `false`), so it's redundant. But more importantly, the memory widget samples at configurable intervals (default 1000ms) which is faster than the default 10s timeout. Since `needs_faster_refresh()` returns false, the main loop won't wake up frequently enough to call `update()` and sample memory on time unless another widget forces faster refresh.

**Fix:** Return `true` or integrate the sample interval into the main loop timeout calculation.

---

## 16. `active` Field on Non-Interactive Widgets

**Files:** `src/widgets/memory.rs:17` (never set), window_title widget (no `active` field but always passes `false`)

`MemoryWidget` has an `active: bool` field that is initialized to `false` and never modified (`handle_event` is a no-op). The field and its plumbing through render are dead code.

**Fix:** Remove `active` from widgets that don't use it.

---

## 17. Inconsistent `show_button_outlines` Config Field

**File:** `src/config.rs:86`

`Config` has `show_button_outlines: bool` but it's never read or used anywhere in the rendering code. It was likely part of a previous implementation.

**Fix:** Remove if unused, or implement it.

---

## 18. `unsafe` `set_var` Calls

**Files:** `src/main.rs:141-145`, `src/workspace/niri.rs:93`

`std::env::set_var` is marked unsafe in Rust 2024 edition (and was already unsound in earlier editions when called from multi-threaded code). The niri backend calls it from a method that could be called while the PA thread is running.

**Fix:** Use `unsafe` blocks with safety comments, or restructure to set env vars before spawning threads.

---

## 19. `println!` Mixed with `eprintln!`

**File:** `src/backlight.rs:161`

```rust
println!("Lid Switch event: {:?}", self.lid_state);
```

All other logging uses `eprintln!`. This single `println!` goes to stdout instead of stderr.

**Fix:** Change to `eprintln!` for consistency.

---

## 20. Hardcoded Magic Numbers in Epoll Registration

**File:** `src/main.rs:164-171, 217-220`

Epoll data values (0, 1, 2, 3, 6, 10+) are scattered across `main.rs` and `layer_manager.rs` with only comments explaining their meaning. The gap at 4-5 is unexplained.

**Fix:** Use named constants (e.g., `EPOLL_INPUT_MAIN = 0`, `EPOLL_INPUT_TB = 1`, etc.).

---

## 21. C String Construction is Non-Idiomatic

**File:** `src/main.rs:194-198`

```rust
let mut dev_name_c = [0 as c_char; 80];
let dn = "Dynamic Function Row Virtual Input Device".as_bytes();
for i in 0..dn.len() {
    dev_name_c[i] = dn[i] as c_char;
}
```

Manual byte-by-byte copy to create a C string.

**Fix:** Use `CString` or at minimum a slice copy operation.

---

## 22. `dispatch_message` Uses Closure That Borrows Multiple Mutable References

**File:** `src/main.rs:401-452`

The `widget_action` closure inside `dispatch_message` takes 6 parameters including both `layer_mgr` and `uinput` by mutable reference, making the code hard to follow. The closure is only used twice (Pressed/Released).

**Fix:** Inline the closure or restructure into a method on `LayerManager`.

---

## 23. `epoll.wait` Uses Single-Element Event Buffer

**File:** `src/main.rs:309-310`

```rust
let mut ep_events = [EpollEvent::new(EpollFlags::EPOLLIN, 0)];
epoll.wait(&mut ep_events, timeout as u16).unwrap_or(0);
```

Only one event is retrieved per iteration. Multiple simultaneous events (e.g., input + widget fd) require multiple loop iterations. Not necessarily a bug, but wastes wakeups.

**Fix:** Consider a larger event buffer (e.g., 4-8 events) to batch process.

---

## 24. `VolumeWidget` Doesn't Forward `handle_event` Actions

**File:** `src/widgets/volume.rs:151-153`

```rust
fn handle_event(&mut self, _action: WidgetAction) -> Vec<MainLoopAction> {
    vec![]
}
```

The volume widget handles press/release through its own `Message::VolumeDownPress` etc. via `mouse_area`, but the generic `handle_event` is a no-op. This means generic press actions from the outer `mouse_area` wrapper in `build_widget_row` are silently dropped. Double-wrapping in mouse_area may cause confusing behavior.

**Fix:** Either handle the outer mouse_area press/release or skip wrapping VolumeWidget in the outer mouse_area.

---

## 25. `WorkspaceWidget` Similarly Double-Wrapped

**File:** `src/widgets/workspace.rs:88-92` and `src/iced_renderer.rs:384-388`

Workspace buttons have their own `mouse_area` handlers (WorkspaceDown/Up), but the outer `build_widget_row` wraps every widget in another `mouse_area` with WidgetPressed/Released. The workspace widget's `handle_event` is a no-op, so the outer press/release is wasted.

**Fix:** Allow widgets to opt out of the outer mouse_area wrapper, or have the workspace widget handle the outer events.

---

## 26. `window_title` Allocated Every Loop Iteration

**File:** `src/layer_manager.rs:160-167`

```rust
pub fn window_title(&self) -> String {
    // ... returns String::new() if no title
}
```

Called every render cycle, allocates a new `String` even when unchanged.

**Fix:** Return `&str` or `Cow<str>`, or cache the title.

---

## 27. `RenderContext` Allocates `window_title: String` Every Frame

**File:** `src/main.rs:289-295`

A new `RenderContext` with a freshly-cloned `window_title` String is created every render frame and every touch event.

**Fix:** Use `&str` lifetime or `Cow` in `RenderContext`.

---

## 28. `VolumeConfig` Wrapper Adds No Value

**Files:** `src/config.rs:56-72`

`VolumeConfig` contains only `pulse_server: Option<String>`, and `VolumeConfigProxy` is a 1:1 mirror. The proxy-to-config conversion is trivial.

**Fix:** Remove the proxy, deserialize directly into `VolumeConfig`.

---

## 29. Config Field Merging is Repetitive

**File:** `src/config.rs:272-283`

```rust
base.media_layer_default = user.media_layer_default.or(base.media_layer_default);
base.show_button_outlines = user.show_button_outlines.or(base.show_button_outlines);
// ... 10 more lines
```

Each config field is manually merged with the same `user.X.or(base.X)` pattern.

**Fix:** Consider a macro or a merge trait to reduce boilerplate.

---

## 30. `BaseConfigProxy` Duplicates Most of `ConfigProxy`

**Files:** `src/config.rs:98-113` vs `src/config.rs:214-225`

`BaseConfigProxy` is a subset of `ConfigProxy` (without layer keys, workspaces, volume). The fields are manually transcribed.

**Fix:** Consider using `#[serde(default)]` on `ConfigProxy` fields instead of maintaining a separate struct.

---

## 31. `system config unwrap` Will Panic If Missing

**File:** `src/config.rs:249`

```rust
let sys_str = read_to_string("/usr/share/smol-dfr/config.toml").unwrap();
```

This panics if the system config file doesn't exist. Other failures are handled gracefully.

**Fix:** Return an error instead of panicking, or at least provide a clear panic message.

---

## 32. `DrmBackend::map()` Called Every Frame

**File:** `src/main.rs:297`

```rust
drm.map().unwrap().as_mut()[..buf.len()].copy_from_slice(buf);
```

`map()` calls `map_dumb_buffer()` which likely involves an mmap syscall each time. The mapping is immediately dropped after copy.

**Fix:** Consider keeping the mapping alive across frames if the DRM API allows it.

---

## 33. `f64` Color Representation Throughout

**Files:** All widget files, `src/config.rs`

Colors are represented as `(f64, f64, f64)` tuples but always used as `f32` in iced. Every usage site casts: `r as f32, g as f32, b as f32`.

**Fix:** Use `(f32, f32, f32)` or iced's `Color` type directly.

---

## 34. R/B Swap Hack in Custom Widgets

**Files:** `src/battery_icon_widget.rs:23-35`, `src/memory_graph_widget.rs:21-28`

Both custom widgets manually swap R and B channels because `fill_quad` uses BGRA byte order internally:

```rust
// Red — R/B swapped for fill_quad BGRA: desired (1,0,0) → (0,0,1)
Color::from_rgb(0.0, 0.0, 1.0)
```

This is fragile and breaks if the iced renderer internals change.

**Fix:** Investigate why `fill_quad` has swapped channels. This may be a bug in the interaction with `iced_tiny_skia` or the rotation pipeline. Fix at the source rather than working around it in every custom widget.

---

## 35. `PIXEL_SHIFT_WIDTH_PX` Is `pub` Unnecessarily

**File:** `src/pixel_shift.rs:15`

`PIXEL_SHIFT_WIDTH_PX` is `pub` but only used within the module (and never imported elsewhere).

**Fix:** Remove `pub`.

---

## 36. Inconsistent Error Handling: Mix of `unwrap()`, `expect()`, `eprintln!`, and `Result`

Throughout the codebase, error handling varies:
- `main.rs`: Heavy use of `.unwrap()` (DRM, uinput, epoll)
- `config.rs`: Returns `Result<_, String>` (not `anyhow::Error`)
- `display.rs`: Returns `anyhow::Result`
- Widgets: `eprintln!` + fallback values
- `backlight.rs`: `eprintln!` + graceful degradation

**Fix:** Standardize on `anyhow::Result` for initialization code, keep `eprintln!` for runtime degradation. Replace bare `.unwrap()` with `.expect("context")` or proper error propagation.

---

## 37. `session_detect::parse_session_properties` Doesn't Trim `=` Value

**File:** `src/session_detect.rs:35-41`

Values after `=` are not trimmed, but the test at line 156-163 uses inputs with trailing whitespace and expects them to work because `line.trim()` is called first. However, if a value itself contains leading/trailing spaces (e.g., `Name= user `), those spaces would be preserved in the parsed result.

This is minor but inconsistent with the comment "with extra whitespace".

---

## 38. `reconnect_watcher.ensure_watches()` Called Unconditionally Every Loop

**File:** `src/main.rs:319`

```rust
reconnect_watcher.ensure_watches();
```

Called every loop iteration regardless of whether any watches were invalidated. Each call does `Path::new(path).exists()` stat calls.

**Fix:** Only call after handling IN_IGNORED events (when watches were actually invalidated).

---

## 39. `add_watch_safe` Does Redundant Existence Check

**File:** `src/reconnect.rs:160-161`

```rust
if !Path::new(path).exists() { return None; }
match inotify.add_watch(path, flags) {
    Err(Errno::ENOENT) => None,
```

The `exists()` check is redundant since `add_watch` already handles `ENOENT`. The check just adds an extra stat syscall.

**Fix:** Remove the `exists()` check.

---

## 40. `WorkspaceWidget` and `VolumeWidget` Don't Reconnect on Inactive Layer

**File:** `src/layer_manager.rs:128-136`

```rust
pub fn reconnect(&mut self) -> bool {
    for w in &mut self.layers[self.active_layer] { ... }
}
```

Only the active layer's widgets get reconnection attempts. If a workspace or volume widget is on the inactive layer and its service restarts, it won't reconnect until the user switches layers.

**Fix:** Reconnect widgets on both layers.

---

## 41. `VolumeManager::thread` Uses `Mutex<Option<JoinHandle<()>>>` for No Clear Reason

**File:** `src/volume.rs:32`

The `thread` field is wrapped in `Mutex` but is only accessed from `try_connect()`, which takes `&self`. The `Mutex` is needed because `try_connect` takes `&self` rather than `&mut self`. But the trait could be changed.

Similarly in `NiriBackend` (`src/workspace/niri.rs:22-23`).

**Fix:** Consider taking `&mut self` in `try_connect()` to avoid interior mutability, or document why `&self` is required.

---

## 42. `epoll.wait` Timeout Cast

**File:** `src/main.rs:310`

```rust
epoll.wait(&mut ep_events, timeout as u16).unwrap_or(0);
```

`timeout` is `i32` but cast to `u16`. If timeout exceeds 65535ms (~65s), it silently wraps. The default `TIMEOUT_MS` is 10000 which is fine, but the pixel shift `PROLONGED_INTERVAL_MS` is 50000 and combined timeouts could theoretically exceed this.

**Fix:** Clamp before casting: `timeout.min(u16::MAX as i32) as u16`.

---

## 43. `time_widget` Render Has Dead Code Branch

**File:** `src/widgets/time.rs:96-104`

```rust
if self.action.is_empty() {
    inner
} else {
    // Wrap in mouse_area -- widget index is set by the renderer...
    inner  // <-- same as the if branch!
}
```

Both branches return `inner` unchanged. The comment suggests wrapping in mouse_area but the code doesn't actually do it. The outer `build_widget_row` already wraps in mouse_area.

**Fix:** Remove the dead conditional.

---

## 44. `display.rs` Collects All Connectors/CRTCs But Only Uses First

**File:** `src/display.rs:74-83`

```rust
let coninfo = res.connectors().iter().flat_map(...).collect::<Vec<_>>();
let crtcinfo = res.crtcs().iter().flat_map(...).collect::<Vec<_>>();
```

Collects all connectors into a Vec but only uses `find(connected)` and `first()`. The allocation is unnecessary.

**Fix:** Use iterators directly without collecting.

---

## 45. Missing `Drop` / Cleanup for Background Threads

**Files:** `src/volume.rs`, `src/workspace/niri.rs`

Neither `VolumeManager` nor `NiriBackend` implements `Drop` to join their background threads. When these objects are dropped during config reload, the background threads may be left dangling.

**Fix:** Implement `Drop` to signal thread termination and join.
