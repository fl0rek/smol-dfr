# Code Review: tiny-dfr Cleanup Issues

Post-rewrite cleanup audit. Each issue is independent and can be addressed one-by-one.

**Status:** 35 of 45 original items resolved as of 2026-08-29 (`02efec4`). Resolved items
are removed rather than struck through; the original numbering is preserved so the item
references in commit messages still resolve, and gaps mean "done". Items 46–50 were found
while doing the work and were not in the original audit; 49 was fixed as it was written.

Ten items remain: nine originals plus the niri half of item 45.

> **Unverified on hardware:** item 34 (the red/blue double-swap, fixed in `a4b47f3`) was
> derived by reading the `iced_tiny_skia` source, not by looking at a touchbar. It changes the
> colour of everything on screen. If the display comes up with red and blue transposed, that
> commit is the one to revert. Nothing else in this audit is unverified in that way — the rest
> is covered by `cargo test`.

## Triage

Size: **XS** ≈ minutes · **S** ≈ under an hour · **M** ≈ a few hours · **L** ≈ a day or more.

| # | Item | Severity | Size | Blocked on |
|---|------|----------|------|------------|
| 18 | `set_var` called from a live thread in `NiriBackend::try_connect` | High | M | design |
| 45 | niri reader thread still leaks on config reload | Medium | M | |
| 46 | Screenshots are colour-swapped | Medium | XS | |
| 47 | `epoll.wait` error swallowed as "zero events" | Medium | XS | |
| 26 | `window_title()` allocates a `String` on every call | Low | XS | |
| 27 | `RenderContext` allocates a `String` per touch event | Low | S | |
| 48 | Dead match arm in `reconnect.rs` | Low | XS | |
| 50 | `WorkspacesConfigProxy::provider` parsed but never read | Low | XS | |
| 17 | `show_button_outlines` parsed but never read | Low | XS–S | **decision** |
| 20 | Hardcoded epoll data values | Low | S | |
| 24 | `VolumeWidget` is double-wrapped in `mouse_area` | Low | S | |
| 25 | `WorkspaceWidget` is double-wrapped in `mouse_area` | Low | S | |
| 33 | `f64` colors cast to `f32` at every use site | Low | S | 35 warnings |
| 36 | Inconsistent error handling across the codebase | Medium | L | 46 warnings |

### On performance

**Nothing remaining is performance-critical.** The idle path was settled by quick tasks 6–12,
`d8422fd` (redundant pre-epoll drain removed), item 23 (8 events per wait) and item 32 (dumb
buffer mapped once instead of ioctl + `mmap` + `munmap` per frame).

What is left is items 26 and 27, both on the touch path: every touch event builds a fresh
`RenderContext`, which calls `layer_mgr.window_title()`, which locks the niri state mutex and
clones a `String`. A finger drag produces a stream of those. On a 2048×64 framebuffer this is
very likely sub-millisecond — treat it as tidiness with a performance flavour, and measure
before changing anything.

---

## 17. `show_button_outlines` Parsed but Never Read

**File:** `src/config.rs`

`Config::show_button_outlines` is deserialised, merged, and required (`load_config` errors with
"missing ShowButtonOutlines in config" if absent) but never read. `button_style()` takes only
colour and active state.

This is a *documented* key — both shipped configs describe it — so deleting it removes an
advertised feature rather than an internal detail.

**Blocked on a decision**, but no longer on missing information: `TECH_DEBT.md` item 11 records
what the pre-iced Cairo path did — when `show_button_outlines` is false it *hides* button
backgrounds (sets the colour to black); the iced path always draws them at 0.2 gray. So
implementing it is a small change to `button_style` (XS–S).

The decision is whether to implement or delete. Implementing restores documented behaviour but
visibly changes the UI for every existing user who has the key set to false. Deleting drops an
advertised feature and means editing both shipped configs.

---

## 18. `set_var` Called From a Live Thread

**File:** `src/workspace/niri.rs:66`

```rust
unsafe { std::env::set_var("NIRI_SOCKET", &socket_path) };
```

`setenv(3)` is not thread-safe. glibc may free the old environment block while another thread
is inside `getenv`, so a concurrent reader can dereference freed memory. This call sits in
`NiriBackend::try_connect()`, which runs at reconnect time — by which point the PulseAudio
mainloop thread is alive, and libpulse does read environment variables.

The three `set_var` calls in `main.rs` are fine: the crate is edition 2021 and they run after
`PrivDrop::apply()` but before `LayerManager::new()` spawns anything, so the process is still
single-threaded there. The `unsafe` block on the niri call silences the lint without
establishing the invariant.

**Fix:** Stop using the environment as a side channel. `niri_ipc::Socket::connect_to(path)`
takes an explicit path; thread the discovered socket path through `NiriBackend` instead of
setting a global. Wants a decision on the plumbing shape before coding.

---

## 20. Hardcoded Magic Numbers in Epoll Registration

**Files:** `src/main.rs`, `src/layer_manager.rs`, `src/widgets/mod.rs`

Epoll data values are scattered as bare integers: 0 and 1 (input seats) and 3 (udev) in
`main.rs`, 2 (config inotify) in `layer_manager.rs`, 6 (reconnect watcher) back in `main.rs`,
and widget fds from 10 up in `FdRegistry`. `main.rs` carries a comment explaining that 2 is
claimed elsewhere, which is a workaround for the layout not being written down.

4 and 5 used to be the workspace and volume eventfds; since `f4322f0` those go through
`FdRegistry` starting at 10, so the gap is now genuinely unused.

Note `EPOLL_EVENT_BUF` already exists as a named constant from item 23 — the data tags are what
remain literal.

**Fix:** Named constants in one module.

---

## 24. `VolumeWidget` Is Double-Wrapped in `mouse_area`

**Files:** `src/widgets/volume.rs`, `src/iced_renderer.rs`

`build_widget_row` wraps every widget in a `mouse_area` emitting `WidgetPressed`/
`WidgetReleased`, and `VolumeWidget::render` builds its own inner `mouse_area`s emitting
`VolumeDownPress` and friends. The widget does not implement `handle_event`, so the outer
messages round-trip through `dispatch_message` and do nothing.

**Fix:** Let widgets opt out of the outer wrapper.

---

## 25. `WorkspaceWidget` Is Double-Wrapped in `mouse_area`

**Files:** `src/widgets/workspace.rs`, `src/iced_renderer.rs`

As item 24: per-workspace `mouse_area`s emitting `WorkspaceDown`/`WorkspaceUp` inside the
generic wrapper, with no `handle_event` to consume the outer messages.

**Fix:** Same as item 24 — one opt-out mechanism resolves both.

---

## 26. `window_title()` Allocates on Every Call

**File:** `src/layer_manager.rs`

Walks the active layer, returns the first widget's `Option<String>`, or `String::new()`. The
workspace widget's implementation locks the niri state mutex and clones the title. Called from
two places: building the `RenderContext` for a redraw, and building one per touch event.

**Fix:** Resolve with item 27 — same allocation.

---

## 27. `RenderContext` Allocates a `String` Per Touch Event

**File:** `src/main.rs`, `src/widgets/mod.rs`

`RenderContext` owns `window_title: String`. One is constructed per redraw and one per touch
event, each cloning the title out from behind the niri mutex.

**Fix:** Borrow instead — `window_title: &str` with a lifetime on `RenderContext`, or
`Cow<'_, str>`. Caching the title in `LayerManager` and invalidating on the workspace eventfd
also works and avoids touching every widget signature.

---

## 33. `f64` Color Representation Throughout

**Files:** `src/config.rs`, all widget files

Colours are `(f64, f64, f64)` and cast at every use site with `as f32`. iced wants `f32` and
the source is 8-bit hex, so the extra precision is never real.

**Fix:** Use `iced_core::Color` from the config boundary inward.

As of `355c991` this accounts for the 35 remaining `cast_*` warnings, clustered in
`src/iced_renderer.rs` (14) and `src/widgets/battery.rs` (10). Those are widget-layout
coordinates, not pixel math — the pixel path no longer casts at all.

---

## 36. Inconsistent Error Handling

Error handling varies by module with no stated rule:

- `main.rs` — heavy bare `.unwrap()` on DRM, uinput and epoll setup
- `config.rs` — `Result<_, String>`
- `display.rs` — `anyhow::Result`
- widgets — `eprintln!` plus a fallback value
- `backlight.rs` — `eprintln!` plus graceful degradation

**Fix:** Write the rule down first — `anyhow::Result` for anything on the startup path,
degrade-and-log for anything on the runtime path — then converge on it.

This item now has a number attached, which is what it was always missing: the clippy panic
family enabled in `5ffba20` reports **46** sites (40 `unwrap` on `Result`, 3 on `Option`,
3 `expect`), concentrated in `main.rs` startup and `config.rs`. The sites are independent, so
this parallelises well across agents — it is the most mechanical of the remaining items despite
the `L` sizing.

---

## 45. niri Reader Thread Still Leaks on Config Reload

**File:** `src/workspace/niri.rs`

The PulseAudio half is done: `VolumeManager` now implements `Drop`, signalling a dedicated
shutdown eventfd registered with the mainloop as an IO event and then joining the thread
(`src/volume.rs`). `NiriBackend` has no equivalent.

`LayerManager::reload()` rebuilds both layers on config hot-reload, dropping the old backend,
but its reader thread stays blocked in `read_events()` forever. Each reload adds another.

This is a resource leak, not a use-after-close — the thread holds its own `dup()`ed `OwnedFd`,
so the backend's fd closing does not invalidate it.

**Fix:** The blocking `read_events()` has to become interruptible. `shutdown(2)` on the socket
fd to force the read to return is the least invasive route; a shutdown flag checked around a
`poll` is the alternative. Deliberately deferred once already because it needs the reader loop
restructured — do not attempt it as a quick patch.

---

## 46. Screenshots Are Colour-Swapped

**File:** `src/iced_renderer.rs`, behind the `screenshot` feature

`self.pixmap.encode_png()` writes the pixmap out directly, but `tiny_skia`'s PNG encoder treats
the buffer as RGBA and the pixmap actually holds BGRA — `iced_tiny_skia` byte-swaps every
colour on the way in (see the note on `rotate_and_convert_into`, and item 34's fix). So saved
screenshots have red and blue inverted.

Not in the original audit; it follows from what item 34 established.

**Fix:** Swap channels into a scratch buffer before encoding. Debug-only, so low priority — but
a screenshot that lies about colour is worse than no screenshot when debugging a colour bug.

---

## 47. `epoll.wait` Error Swallowed as "Zero Events"

**File:** `src/main.rs`

```rust
epoll.wait(&mut ep_events, timeout).unwrap_or(0);
```

`unwrap_or(0)` turns a genuine failure — `EBADF` from a closed fd, say — into "no events were
ready". The loop would then spin at full speed with no diagnostic, which is exactly the
failure mode quick task 12 and `d8422fd` were chasing.

Now that the returned count is meaningful (item 23 widened the buffer), the error deserves
handling too.

**Fix:** Log the error at minimum; consider treating a repeated hard error as fatal rather than
spinning silently.

---

## 48. Dead Match Arm in `reconnect.rs`

**File:** `src/reconnect.rs:72`

```rust
Err(Errno::EAGAIN) | Err(Errno::EWOULDBLOCK) => return ReconnectEvents::default(),
```

On Linux `EAGAIN` and `EWOULDBLOCK` are the same value, so the second pattern is unreachable.
This is one of the crate's live warnings.

**Fix:** Drop the `EWOULDBLOCK` arm, or add a comment if the intent is portability.

---

## 50. `WorkspacesConfigProxy::provider` Parsed but Never Read

**File:** `src/config.rs`

`WorkspacesConfig::provider` was removed in `256eeef` (item 11) because niri is the only
backend, but the field survives on the deserialisation proxy. So `Provider = "niri"` in a
config file is still accepted and silently ignored, and rustc reports
`field \`provider\` is never read`.

Same shape as the `Battery { mode }` and `Icon { theme }` fields that item 9 and 10 removed.

**Fix:** Drop the field from the proxy. Existing configs setting it keep working —
`WidgetEntry` uses `#[serde(flatten)]` so unknown keys are ignored, not rejected.
