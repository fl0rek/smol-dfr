- Theme customization (colors, fonts, spacing)
- Partial redraws: Track dirty regions from iced

## Done

- Conditional rendering: skip frames when no changes. The main loop gates the
  whole render on `needs_redraw`, which is set only when a widget's `update()`
  reports an actual change.
- Multiple layers (Primary/Media) and Fn-key layer switching.
- Adaptive brightness integration (`AdaptiveBrightness` in config, `backlight.rs`).
- Pixel shift. Dropped rather than finished: the offset was computed every
  iteration and never applied to anything, so the feature was removed along with
  its config key and the `rand` dependency.

## Rendering budget

**Answered.** The concern was that rotating the pixel buffer every frame would
be expensive. Measured on the real geometry (2048x64 source, 60 visible rows),
300 frames per run: **~0.19 ms per frame**, against the ~16 ms budget for 60fps.

The old proposals here — SIMD via `wide`/`packed_simd`, rendering directly in
rotated space by modifying `iced_tiny_skia`, or GPU rotation via `iced_wgpu` —
were written before anyone measured. None of them are worth the complexity at
0.19 ms.

The rotation was rewritten from index math to iterators anyway (`src/rotate.rs`),
which was ~25% faster *and* removed the unchecked indexing, so it was not a
trade. If rotation ever does become a problem, re-measure first.
