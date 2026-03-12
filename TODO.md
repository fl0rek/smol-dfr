- Theme customization (colors, fonts, spacing)
- Partial redraws: Track dirty regions from iced
- Conditional rendering: Skip frames when no changes
- Pixel shift: Apply offset to widget tree bounds

- Multiple layers (Primary/Media/Custom)
- Keyboard event handling (Fn key layer switching)
- Button hold actions
- Adaptive brightness integration

- Rendering completes within frame budget (~16ms for 60fps)

## Known Challenges & Solutions

### Rotation Performance
**Issue**: Rotating pixel buffer on every frame may be expensive

**Solutions**:
- **Option A**: Optimize with SIMD (use `wide` or `packed_simd` crate)
- **Option B**: Render directly in rotated space (modify iced_tiny_skia)
- **Option C**: Use GPU rotation (requires iced_wgpu, more complex)
