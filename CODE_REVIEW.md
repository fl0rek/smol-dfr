# Code Review: tiny-dfr Cleanup Issues

Post-rewrite cleanup audit. Each issue is independent and can be addressed one-by-one.

**Status:** re-verified against the tree at `9bba109` on 2026-08-29. Items 1–13, 35 and 38
were removed as resolved or invalid; the numbering of everything else is unchanged so that
the commit messages referencing item numbers still resolve. Gaps in the sequence mean
"done", not "missing".

Several surviving items had stale details — wrong line numbers, a rationale that depended
on since-deleted code, or a severity that changed when the shipped config changed. Those
carry a **Revised** note.

## Triage

Size: **XS** ≈ minutes · **S** ≈ under an hour · **M** ≈ a few hours · **L** ≈ a day or more.

| # | Item | Severity | Size | Hot path |
|---|------|----------|------|----------|
| 40 | Volume/workspace widgets never reconnect on the inactive layer | High | XS | |
| 15 | Memory graph is sample-starved by the main loop timeout | High | S | |
| 18 | `set_var` called from a live thread in `NiriBackend::try_connect` | High | M | |
| 31 | System config `unwrap()` panics with no message | Medium | XS | |
| 42 | `epoll.wait` timeout cast to `u16` without clamping | Medium | XS | |
| 32 | `DrmBackend::map()` mmaps on every redraw | Medium | S | per redraw |
| 27 | `RenderContext` allocates a `String` per touch event | Medium | S | per touch |
| 23 | `epoll.wait` uses a single-element event buffer | Medium | XS | per wakeup |
| 45 | Background threads leak on config reload | Medium | M | |
| 34 | R/B channel swap worked around in every custom widget | Medium | M | |
| 36 | Inconsistent error handling across the codebase | Medium | L | |
| 43 | Dead conditional in `TimeWidget::render` | Low | XS | |
| 44 | `display.rs` collects connectors/CRTCs it does not need | Low | XS | |
| 39 | `add_watch_safe` does a redundant `exists()` check | Low | XS | |
| 37 | `parse_session_properties` does not trim the value | Low | XS | |
| 19 | `println!` where everything else uses `eprintln!` | Low | XS | |
| 21 | Manual byte-by-byte C string construction | Low | XS | |
| 14 | `MemoryWidget::sample_interval_ms()` never called | Low | XS | |
| 16 | Dead `active` field on `MemoryWidget` | Low | XS | |
| 26 | `window_title()` allocates a `String` on every call | Low | XS | per touch |
| 28 | `VolumeConfig` proxy adds no value | Low | XS | |
| 17 | `show_button_outlines` parsed but never read | Low | XS–S | |
| 20 | Hardcoded epoll data values | Low | S | |
| 22 | `dispatch_message` closure takes 6 parameters | Low | S | |
| 24 | `VolumeWidget` is double-wrapped in `mouse_area` | Low | S | |
| 25 | `WorkspaceWidget` is double-wrapped in `mouse_area` | Low | S | |
| 29 | Config field merging is repetitive | Low | S | |
| 33 | `f64` colors cast to `f32` at every use site | Low | S | |
| 41 | `Mutex<Option<JoinHandle>>` for interior mutability | Low | S | |
| 30 | `BaseConfigProxy` duplicates most of `ConfigProxy` | Low | M | |

### On performance

**Nothing in this list is performance-critical at idle.** Idle CPU was the subject of quick
tasks 6–12 and `d8422fd`; the main loop now blocks in `epoll.wait` and only renders when a
widget reports a change. What is left touches two paths that are warm but not hot:

- **Per redraw** — item 32. `map_dumb_buffer` is an ioctl plus `mmap`, and the `DumbMapping`
  is dropped (so `munmap`) immediately after the `copy_from_slice`. That is three syscalls
  per frame that could be zero.
- **Per touch event** — items 27 and 26. Every touch event builds a fresh `RenderContext`,
  which calls `layer_mgr.window_title()`, which locks the niri state mutex and clones a
  `String`. A finger drag produces a stream of these.
- **Per wakeup** — item 23. One event per `epoll.wait` means one extra loop iteration per
  pending event, each running `layer_mgr.update()` and `poll()` over both layers.

On a 2048×64 framebuffer all three are very likely sub-millisecond. Treat them as tidiness
with a performance flavour, not as optimisation work, and measure before changing anything.

Item 15 reads like a performance issue but is a correctness one — see below.

---

## 14. `sample_interval_ms()` Never Called

**File:** `src/widgets/memory.rs:33-35`

`MemoryWidget::sample_interval_ms()` forwards to `MemoryHistory::sample_interval_ms()` and
is called from nowhere. Its doc comment ("Expose sample interval for main loop timeout
calculation") describes the wiring that item 15 says is missing.

**Fix:** Remove it, or use it to fix item 15 — those are the same decision, so resolve 15 first.

---

## 15. Memory Graph Is Sample-Starved by the Main Loop Timeout

**File:** `src/widgets/memory.rs:85-87`, `src/main.rs:246-251`

```rust
fn needs_faster_refresh(&self) -> bool { false }
```

The main loop timeout is `min((60 - second) * 1000, TIMEOUT_MS)` with `TIMEOUT_MS = 10_000`,
shortened to 1000ms only when some widget returns true from `needs_faster_refresh()`.
`MemoryWidget` returns false, so unless another widget forces a faster refresh the loop can
sleep for up to 10s while `MemoryHistory::maybe_sample()` expects to be called at
`sample_interval_ms` (default 1000ms).

The shipped `config.toml` makes this concrete: its time widgets use `%H:%M` and `%Y-%m-%d`,
neither of which contains seconds, so `TimeWidget::needs_faster_refresh()` is false too.
Nothing forces the fast path, and the graph collects roughly one sample per 10s while its
x-axis is scaled for one per second.

**Revised 2026-08-29:** the original entry called this "suspicious" and "redundant". It is
neither — it is a live bug in the default configuration, and the redundant-override framing
buried that.

**Fix:** Fold the sample interval into the main loop timeout (this is what item 14's dead
accessor was for) rather than returning `true`, which would pin the loop at 1000ms even when
the configured interval is longer.

---

## 16. `active` Field on Non-Interactive Widgets

**File:** `src/widgets/memory.rs:14,28,41`

`MemoryWidget` has an `active: bool` initialised to `false` and never written — it does not
implement `handle_event`, so it takes the trait's no-op default. The field is threaded
through `render` into `button_style` where it is always `false`.

**Revised 2026-08-29:** `window_title.rs` no longer has the mirror problem the original entry
mentioned; it passes a literal `false` to `styled_text_widget`, which is honest.

**Fix:** Remove `active` from `MemoryWidget` and pass `false` at the call site.

---

## 17. `show_button_outlines` Parsed but Never Read

**File:** `src/config.rs:84,99,201,213,254,307`

`Config::show_button_outlines` is deserialised, merged, and required (`load_config` errors
out with "missing ShowButtonOutlines in config" if absent) but never read. `button_style()`
takes only colour and active state.

Note this is a *documented* key — both shipped configs describe it, so deleting it removes
an advertised feature rather than an internal detail.

**Fix:** Decide deliberately. Implementing it is a small change to `button_style` (XS–S);
removing it means also removing it from both config files and accepting the behaviour change.

---

## 18. `set_var` Called From a Live Thread

**File:** `src/workspace/niri.rs:66`

```rust
unsafe { std::env::set_var("NIRI_SOCKET", &socket_path) };
```

`setenv(3)` is not thread-safe. glibc may free the old environment block while another
thread is inside `getenv`, so a concurrent reader can dereference freed memory. This call
sits in `NiriBackend::try_connect()`, which runs at reconnect time — by which point the
PulseAudio mainloop thread and possibly a previous niri reader thread are alive, and
libpulse does read environment variables.

**Revised 2026-08-29:** the original entry also flagged `src/main.rs:140-145`. Those are
fine: the crate is edition 2021 and all three calls happen after `PrivDrop::apply()` but
before `LayerManager::new()` spawns anything, so the process is still single-threaded there.
The `unsafe` block on the niri call is doing no work — it silences the lint without
establishing the invariant.

**Fix:** Stop using the environment as a side channel. `niri_ipc::Socket::connect_to(path)`
takes an explicit path; thread the discovered socket path through `NiriBackend` instead of
setting a global.

---

## 19. `println!` Mixed With `eprintln!`

**File:** `src/backlight.rs:160`

```rust
println!("Lid Switch event: {:?}", self.lid_state);
```

The only `println!` in the crate; everything else logs to stderr.

**Fix:** Change to `eprintln!`. Folds into the tracing migration seeded in `1534e7c`.

---

## 20. Hardcoded Magic Numbers in Epoll Registration

**Files:** `src/main.rs:163,166,170,218`, `src/layer_manager.rs:37`, `src/widgets/mod.rs:89`

Epoll data values are scattered as bare integers: 0 and 1 (input seats) and 3 (udev) in
`main.rs`, 2 (config inotify) in `layer_manager.rs`, 6 (reconnect watcher) back in
`main.rs`, and widget fds from 10 up in `FdRegistry`. `main.rs` carries a comment
explaining that 2 is claimed elsewhere, which is a workaround for the layout not being
written down anywhere.

**Revised 2026-08-29:** 4 and 5 used to be the workspace and volume eventfds. Since
`f4322f0` those go through `FdRegistry` starting at 10, so the gap is now genuinely unused
rather than merely undocumented.

**Fix:** Named constants in one module.

---

## 21. C String Construction Is Non-Idiomatic

**File:** `src/main.rs:193-197`

```rust
let mut dev_name_c = [0 as c_char; 80];
let dn = "Dynamic Function Row Virtual Input Device".as_bytes();
for i in 0..dn.len() {
    dev_name_c[i] = dn[i] as c_char;
}
```

Manual byte-by-byte copy. Also silently truncates nothing today but would overflow the
array if the name ever exceeded 80 bytes.

**Fix:** Slice copy with an explicit length check, or `CString`.

---

## 22. `dispatch_message` Closure Takes Six Parameters

**File:** `src/main.rs:392-416`

The `widget_action` closure takes `layer_mgr`, `idx`, `action`, `uinput`, `btu` and `redraw`
— it captures nothing, so every dependency is threaded through the parameter list. It is
called exactly twice, for `WidgetPressed` and `WidgetReleased`.

**Fix:** Promote it to a free function or a `LayerManager` method.

---

## 23. `epoll.wait` Uses a Single-Element Event Buffer

**File:** `src/main.rs:293-294`

```rust
let mut ep_events = [EpollEvent::new(EpollFlags::EPOLLIN, 0)];
epoll.wait(&mut ep_events, timeout as u16).unwrap_or(0);
```

One event is dequeued per iteration, so N simultaneously-ready fds cost N full loop
iterations — each running `layer_mgr.update()` and `layer_mgr.poll()` across both layers.
The return value is discarded via `unwrap_or(0)`, so the count is not even inspected.

**Fix:** Use a 4–8 element buffer and iterate the returned slice.

---

## 24. `VolumeWidget` Is Double-Wrapped in `mouse_area`

**Files:** `src/widgets/volume.rs`, `src/iced_renderer.rs:396-401`

`build_widget_row` wraps every widget in a `mouse_area` emitting
`WidgetPressed`/`WidgetReleased`, and `VolumeWidget::render` builds its own inner
`mouse_area`s emitting `VolumeDownPress` and friends. The widget does not implement
`handle_event`, so the outer messages round-trip through `dispatch_message` and do nothing.

**Revised 2026-08-29:** the explicit `fn handle_event(&mut self, _) -> vec![]` the original
entry quoted was removed in `256eeef`; the widget now inherits the trait default. Same
behaviour, different code.

**Fix:** Let widgets opt out of the outer wrapper.

---

## 25. `WorkspaceWidget` Is Double-Wrapped in `mouse_area`

**Files:** `src/widgets/workspace.rs`, `src/iced_renderer.rs:396-401`

As item 24: per-workspace `mouse_area`s emitting `WorkspaceDown`/`WorkspaceUp` inside the
generic wrapper, with no `handle_event` to consume the outer messages.

**Fix:** Same as item 24 — one opt-out mechanism resolves both.

---

## 26. `window_title()` Allocates on Every Call

**File:** `src/layer_manager.rs:160-167`

Walks the active layer, returns the first widget's `Option<String>`, or `String::new()`.
The workspace widget's implementation locks the niri state mutex and clones the title.

**Revised 2026-08-29:** the original entry said "called every render cycle". It is now
called from exactly two places — building the `RenderContext` for a redraw
(`src/main.rs:285`) and building one per touch event (`src/main.rs:361`). Redraws are gated
on actual change, so the idle cost is gone; the touch-event path remains.

**Fix:** Resolve with item 27 — they are the same allocation.

---

## 27. `RenderContext` Allocates a `String` Per Touch Event

**File:** `src/main.rs:280-286` and `src/main.rs:356-362`

`RenderContext` owns `window_title: String`. One is constructed per redraw and one per
touch event, each cloning the title out from behind the niri mutex. A drag across the
touchbar generates a continuous stream of touch events.

**Fix:** Borrow instead — `window_title: &str` with a lifetime on `RenderContext`, or
`Cow<'_, str>`. Caching the title in `LayerManager` and invalidating on the workspace
eventfd would also work and avoids touching every widget signature.

---

## 28. `VolumeConfig` Wrapper Adds No Value

**File:** `src/config.rs:53-70`

`VolumeConfig` holds one field, `VolumeConfigProxy` mirrors it exactly, and the `From` impl
moves it across. `WorkspacesConfig` has the same shape but earns its proxy — it applies
defaults and parses colours.

**Fix:** Derive `Deserialize` on `VolumeConfig` directly and delete the proxy.

---

## 29. Config Field Merging Is Repetitive

**File:** `src/config.rs:252-264`

Twelve consecutive lines of `base.X = user.X.or(base.X)`. Adding a config key means
remembering to add a line here, and forgetting silently drops the user's override.

**Fix:** A `merge!` macro over the field list, or a derive.

---

## 30. `BaseConfigProxy` Duplicates Most of `ConfigProxy`

**Files:** `src/config.rs:97-110` vs `src/config.rs:199-210`

`BaseConfigProxy` is `ConfigProxy` minus the layer keys, workspaces and volume, with the
fields transcribed by hand and an `into_config_proxy()` that copies them across. It exists
to re-parse global settings when the system config has old-format layer entries.

**Fix:** `#[serde(default)]` plus `#[serde(flatten)]` on a shared globals struct, so the
field list lives in one place.

---

## 31. System Config `unwrap()` Panics With No Message

**File:** `src/config.rs:230`

```rust
let sys_str = read_to_string("/usr/share/smol-dfr/config.toml").unwrap();
```

Panics with a bare `Os { code: 2 }` if the shipped config is missing — a plausible packaging
or dev-checkout failure. Every other read in the function degrades gracefully, and
`load_config` already returns `Result<_, String>`, so the error has somewhere to go.

**Fix:** Propagate as an error naming the path.

---

## 32. `DrmBackend::map()` Called Every Redraw

**Files:** `src/main.rs:288`, `src/display.rs:210-212`

```rust
drm.map().unwrap().as_mut()[..buf.len()].copy_from_slice(buf);
```

`map()` calls `map_dumb_buffer`, which issues `DRM_IOCTL_MODE_MAP_DUMB` and `mmap`. The
returned `DumbMapping` is a temporary, so it is unmapped at the end of the statement —
three syscalls per frame for a mapping that could persist.

**Fix:** Hold the mapping in `DrmBackend` across frames if the borrow checker and the `drm`
crate's lifetimes allow (`map()` takes `&mut self` and `DumbMapping` borrows the buffer, so
this needs care). Measure first — see the performance note above.

---

## 33. `f64` Color Representation Throughout

**Files:** `src/config.rs`, all widget files

Colours are `(f64, f64, f64)` (28 occurrences) and cast at every use site (22 `as f32`).
iced wants `f32` and the source is 8-bit hex, so the extra precision is never real.

**Fix:** Use `iced_core::Color` from the config boundary inward.

---

## 34. R/B Swap Hack in Custom Widgets

**Files:** `src/battery_icon_widget.rs:27,30`, `src/memory_graph_widget.rs:26`

Both custom widgets pre-swap red and blue because `fill_quad` writes BGRA:

```rust
// Red — R/B swapped for fill_quad BGRA: desired (1,0,0) → (0,0,1)
Color::from_rgb(0.0, 0.0, 1.0)
```

Every new custom widget has to rediscover this, and an `iced_tiny_skia` upgrade could
silently invert every colour.

**Fix:** Find the actual source. The suspects are the `Pixmap` format, the RGBA→XRGB8888
conversion in the rotation pass, and the DRM framebuffer format — the fix belongs in
whichever one is lying, not in the widgets.

---

## 36. Inconsistent Error Handling

Error handling varies by module with no stated rule:

- `main.rs` — heavy bare `.unwrap()` on DRM, uinput and epoll setup
- `config.rs` — `Result<_, String>`
- `display.rs` — `anyhow::Result`
- widgets — `eprintln!` plus a fallback value
- `backlight.rs` — `eprintln!` plus graceful degradation

**Fix:** Write the rule down first — `anyhow::Result` for anything on the startup path,
degrade-and-log for anything on the runtime path — then converge on it. Large and diffuse;
best done opportunistically alongside the tracing migration rather than as one change.

---

## 37. `parse_session_properties` Does Not Trim the Value

**File:** `src/session_detect.rs:28-45`

`line.trim()` runs before `strip_prefix`, so trailing whitespace is handled but a space
after the `=` is not: `Name= user` parses to `" user"`. The test inputs only exercise
trailing whitespace, so this passes.

**Fix:** `val.trim()` at each of the three call sites.

---

## 39. `add_watch_safe` Does a Redundant Existence Check

**File:** `src/reconnect.rs:160-162`

```rust
if !Path::new(path).exists() { return None; }
match inotify.add_watch(path, flags) {
    Err(Errno::ENOENT) => None,
```

`add_watch` already maps `ENOENT` to `None` on the next line, so the `exists()` stat is
pure overhead — and racy besides, since the directory can appear between the two calls.

**Fix:** Delete the `exists()` check.

---

## 40. Widgets on the Inactive Layer Never Reconnect

**File:** `src/layer_manager.rs:128-143`

```rust
pub fn reconnect(&mut self) -> bool {
    for w in &mut self.layers[self.active_layer] { ... }
}
```

`reconnect()` and `any_disconnected()` both only look at the active layer. Initial
connection is fine — `build_widget_layer` calls `try_connect()` for both layers at
construction — but once a service restarts, a widget on the inactive layer stays
disconnected until the user switches layers.

**Revised 2026-08-29:** this went from theoretical to default-configuration behaviour in
`9bba109`, which moved the volume widget into `MediaLayerKeys` while `MediaLayerDefault`
is `false`. The volume widget now lives on the inactive layer out of the box, so a
PulseAudio restart leaves it dead until the user holds Fn. The startup warning
("will reconnect when available") is actively wrong for it.

**Fix:** Iterate both layers in `reconnect()` and `any_disconnected()`.

---

## 41. `Mutex<Option<JoinHandle<()>>>` for Interior Mutability

**Files:** `src/volume.rs:31`, `src/workspace/niri.rs:23`

The join handle is wrapped in a `Mutex` only because `try_connect()` takes `&self`, which
in turn is because `WorkspaceBackend::try_connect` is declared that way. Nothing else
contends for the lock.

**Fix:** Take `&mut self` in the trait method and drop the `Mutex`, or document why `&self`
is required. Interacts with item 45 — both concern thread ownership.

---

## 42. `epoll.wait` Timeout Cast Without Clamping

**File:** `src/main.rs:294`

```rust
epoll.wait(&mut ep_events, timeout as u16).unwrap_or(0);
```

`timeout` is `i32` and wraps silently above 65535.

**Revised 2026-08-29:** currently unreachable. The original entry justified this with pixel
shift's `PROLONGED_INTERVAL_MS = 50000`, which `963356a` deleted. Today `timeout` starts at
`min((60 - second) * 1000, TIMEOUT_MS)` with `TIMEOUT_MS = 10_000` and is only ever reduced,
so it cannot exceed 10000. Kept because the trap is one constant away from reopening —
raising `TIMEOUT_MS` past 65535 would produce a truncated timeout with no diagnostic. Item
15's fix touches exactly this arithmetic.

**Fix:** `timeout.clamp(0, u16::MAX as i32) as u16`.

---

## 43. Dead Conditional in `TimeWidget::render`

**File:** `src/widgets/time.rs:82-90`

```rust
if self.action.is_empty() {
    inner
} else {
    // Wrap in mouse_area -- widget index is set by the renderer ...
    inner  // <-- same as the if branch
}
```

Both branches return `inner`. The comment describes wrapping that `build_widget_row` already
does.

**Fix:** Delete the conditional and the comment.

---

## 44. `display.rs` Collects Connectors/CRTCs It Does Not Need

**File:** `src/display.rs:74-83`

Both `connectors()` and `crtcs()` are collected into `Vec`s, then reduced to a single
element by `find(connected)` and `first()`.

**Fix:** Drop the `collect()` and use the iterators directly. Startup-only, so this is
tidiness rather than performance.

---

## 45. Background Threads Leak on Config Reload

**Files:** `src/volume.rs`, `src/workspace/niri.rs`

Neither `VolumeManager` nor `NiriBackend` implements `Drop`. `LayerManager::reload()`
rebuilds both layers, dropping the old managers, but their threads keep running — the niri
reader blocked in `read_events()` forever, the PulseAudio mainloop spinning with its server
connection still open. Each config hot-reload adds another pair.

**Revised 2026-08-29:** not a use-after-close. Both threads receive their own `dup()`ed
`OwnedFd` (`src/volume.rs:70`, `src/workspace/niri.rs:115`), so the manager's `OwnedFd`
closing does not invalidate the thread's. `VolumeManager::try_connect` also joins its
previous thread before spawning. The problem is confined to the drop path, which makes this
a resource leak rather than the memory-safety hazard the original entry implied.

**Fix:** Implement `Drop`: signal the thread to stop (shutdown flag plus an eventfd poke
for niri, `mainloop.quit()` for PulseAudio) and join it. Shutting down the niri reader
cleanly needs the socket to be poll-able rather than blocking, so this is more than a
few lines.
