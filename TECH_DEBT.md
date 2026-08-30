# Tech Debt: iced Integration

## Touch Input

## 1. Single cursor fakes multi-touch

iced has one `mouse::Cursor`. Each touch event sets it to that finger's position.
With two fingers down simultaneously, processing finger B's event jumps the cursor
away from finger A's button, which can falsely trigger `on_exit` for finger A's
`mouse_area` and release that key.

**Fix**: Either track per-finger state outside iced (like Cairo path does with
`touches` HashMap), or write a custom widget that handles `touch::Finger` IDs
natively instead of relying on `mouse_area`'s cursor-based hit testing.

## 2. No slide-back re-activation

If a finger slides off a button (`on_exit` fires `ButtonUp`) then slides back
onto it, nothing happens -- the key stays released. The Cairo path re-activates
the key on every motion event via `set_active(hit)`.

**Fix**: Add `.on_enter(Message::ButtonDown(i))` to the `mouse_area`. This is
simple but interacts badly with issue #1 (cursor jumps from multi-touch would
cause spurious presses). Fix #1 first.

## 3. Widget tree + layout rebuilt on every touch event

`process_touch` calls `build_button_row` and `layout()` from scratch each time.
For 13 buttons this is cheap, but it's redundant work when nothing has changed.

**Fix**: Cache the layout `Node` and only rebuild when button count or dimensions
change. The `Tree` is already persisted; extend that to the layout.

## 4. ButtonDef Vec allocated on every touch event

`build_button_defs()` is called in the touch handler, allocating a `Vec<ButtonDef>`
with `String::clone()` for each label on every finger-down, motion, and finger-up.

**Fix**: Cache the `Vec<ButtonDef>` and invalidate only on layer switch, config
reload, or button state change.

## 5. `tree.take()`/put-back for borrow checker

The `Tree` is taken out of `self` with `Option::take()` and put back after use to
avoid split-borrow issues between `self.tree` and `self.renderer`. If a panic
occurs between take and put-back, the tree is lost (silently rebuilt on next call,
so not catastrophic, but loses hover state).

**Fix**: Restructure so `tree` and `renderer` are in separate structs, or use a
helper that guarantees put-back (e.g. a guard/drop pattern).

## 6. Duplicate ButtonUp from on_release + on_exit

If a finger is lifted right at a button boundary, both `on_exit` and `on_release`
can fire in the same `on_event` pass, producing two `ButtonUp` messages. This is
safe because `set_active` is idempotent, but it's wasted work and could be
confusing during debugging.

**Fix**: Use a message that carries intent (e.g. `ButtonExit` vs `ButtonRelease`)
and deduplicate in the handler, or accept the idempotency and leave as-is.

## 7. No per-finger button ownership in iced path

The Cairo path tracks which specific button each finger "owns" via
`touches: HashMap<slot, (layer, button_index)>`. This means finger A releasing
only affects finger A's button, regardless of where the cursor is.

The iced path has no such tracking -- it relies entirely on `mouse_area`'s
cursor-position hit testing. Combined with issue #1, this means one finger's
events can affect another finger's button state.

**Fix**: Maintain a per-finger ownership map alongside iced events (hybrid
approach), or build a touch-aware widget that tracks finger IDs internally.

---

## Renderer

### 8. Text-only buttons -- no SVG or bitmap icons

`ButtonDef` only carries a `label: String`. The Cairo path renders `Svg`, `Bitmap`,
and `Spacer` button types with their actual content. The iced path shows "?" for
anything that isn't `Text`, `Time`, or `Battery`.

**Fix**: Add an enum to `ButtonDef` (or a richer type) that carries SVG handles /
image data. Use `iced_widget::svg` or `iced_widget::image` in `build_button_row`.

### 9. Battery button loses icon and charging color

Cairo renders battery icons (charging/plain variants by level), a bolt icon, and
colors the button background green (charging) or red (low). The iced path renders
only the percentage text with a plain gray background.

**Fix**: Port the battery icon selection logic into `build_button_defs` (or a
battery-specific widget). Use `iced_widget::svg` for the icon, and pass a
background color through `ButtonDef` based on `BatteryState`.

### 10. No stretch / variable-width buttons

Cairo buttons use `virtual_button_count` and per-button `stretch` values to span
multiple slots. The iced path gives every button `Length::Fill`, making them all
equal width regardless of config.

**Fix**: Set each button's width to `Length::FillPortion(stretch)` based on the
button's span (computed from `start` indices in `FunctionLayer.buttons`).

### 11. No `show_button_outlines` config support

Cairo hides button backgrounds when `show_button_outlines` is false (sets color
to 0.0 = black). The iced path always shows backgrounds at 0.2 gray.

**Fix**: Pass `show_button_outlines` through to `build_button_row` and set
inactive `bg_color` to 0.0 when outlines are disabled.

> Tracked as `CODE_REVIEW.md` item 17, which was blocked on not knowing what the
> key was supposed to do. This entry is the answer: it hides backgrounds. Note
> the key is still parsed and required by `load_config`, so it is a documented
> feature that silently does nothing — implementing it will visibly change the
> UI for every existing user.

### 12. No pixel shift (OLED burn-in prevention) — OBSOLETE

Resolved by removal, not implementation. `PixelShiftManager` computed an offset
every loop iteration that nothing ever applied, so it only cost wakeups and
redraws. The module, the `EnablePixelShift` config key and the `rand` dependency
were all dropped. Re-adding it means designing it from scratch.

### 13. No partial/dirty-region redraws

Cairo tracks per-button `ClipRect` dirty regions and only redraws changed buttons.
The iced path always does a full-screen redraw and dirties the entire framebuffer.

**Fix**: Track which buttons changed between frames and compute dirty `ClipRect`s
from widget layout bounds (post-rotation). Or accept full redraws if perf is fine.

### 14. Pixmap + clip mask + rotation buffer allocated every frame — RESOLVED

All three are now stored in `TouchbarRenderer` and reused; the pixmap is cleared
per frame instead of reallocated. Original entry below.


`render_to_buffer` creates a new `Pixmap`, `Mask`, and `Vec<u8>` rotation buffer
on every call. For ~2170x64 at 4 bytes/pixel this is ~540 KB allocated and zeroed
per frame.

**Fix**: Store the pixmap, clip mask, and rotation buffer in `TouchbarRenderer`
and reuse them. Clear/zero at the start of each frame instead of reallocating.

### 15. Naive per-pixel rotation loop — RESOLVED, and it was not the problem

Rewritten in `src/rotate.rs` from index math to `chunks_exact`/`chunks_exact_mut`
with integer unpremultiply. ~25% faster and it removed the unchecked indexing, so
it was not a safety-for-speed trade.

More usefully: it was **measured**. ~0.19 ms per frame on the real geometry
against a ~16 ms budget. The SIMD and rotated-rendering suggestions here were
written before anyone had a number, and are not worth the complexity.

### 16. Hardcoded theme and font size

Theme is hardcoded to `Theme::KanagawaDragon`, font size to 20px, spacing to 4px,
padding to 2px. Cairo uses `config.font_face` and `config.show_button_outlines`
from the config file.

**Fix**: Read theme/font/spacing from `Config` and pass through to the renderer.

### 17. Spacer buttons rendered as visible "?" buttons

`ButtonImage::Spacer` is a zero-action invisible gap in the Cairo path (no
background, no text). The iced path renders it as a "?" button with a gray
background and a `mouse_area` that can be pressed.

**Fix**: Detect spacers in `build_button_defs` (e.g. via a `ButtonDef::Spacer`
variant or a flag) and render them as `iced_widget::space::Space` with no
`mouse_area` wrapper.
