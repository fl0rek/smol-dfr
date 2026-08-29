//! 90-degree rotation and premultiplied-BGRA to XRGB8888 conversion.
//!
//! Kept in its own module because it is the one genuinely hot function in the
//! render path — it touches every pixel of the framebuffer on every redraw —
//! and because being self-contained lets `benches/rotate.rs` pull it in
//! directly, which a binary crate otherwise cannot do.

/// Unpremultiply one colour channel: `c * 255 / a`, saturated to a byte.
///
/// Integer throughout, so there is no lossy float conversion to suppress. The
/// `try_from` cannot fail after the `min`, and LLVM drops the branch.
///
/// Note this is exactly `⌊c·255/a⌋`. The previous float form computed
/// `c / (a / 255.0)`, and `a / 255.0` is not representable in binary, so it
/// could land one off the true value on some inputs.
#[inline]
fn unpremultiply_channel(c: u8, a: u8) -> u8 {
    debug_assert!(a != 0, "caller must handle the fully transparent case");
    let scaled = (u32::from(c) * 255 / u32::from(a)).min(255);
    u8::try_from(scaled).unwrap_or(u8::MAX)
}

/// Rotate 90 degrees clockwise and unpremultiply into XRGB8888, writing into a
/// caller-provided buffer to avoid per-frame allocation.
///
/// # Channel order
///
/// The channels are copied straight across, with no R/B swap. `iced_tiny_skia`
/// already stores colours byte-swapped: every draw path builds its tiny-skia
/// colour as `from_rgba(color.b, color.g, color.r, color.a)` — see
/// `engine::into_color`, and the equivalents in its `text`, `raster`, `geometry`
/// and `vector` modules — because iced's software renderer targets BGRA
/// surfaces. So the pixmap holds B,G,R,A, and DRM's XRGB8888 wants B,G,R,X in
/// memory on little-endian. They line up one-to-one.
///
/// Swapping here as well double-swaps and inverts red and blue across the whole
/// display; that bug is why `BatteryIconWidget` and `MemoryGraphWidget` used to
/// pre-swap their own colours to compensate.
///
/// # Traversal
///
/// Destination row `dy` reads source column `dy`, walking source rows from
/// `src_h - 1` down to 0. Writes are sequential; the source read is a strided
/// gather, which is the cache-unfriendly half and why the destination is the
/// one kept in order.
///
/// When `dst_w > src_h` the leading `dst_w - src_h` pixels of every destination
/// row have no source pixel and stay zeroed, alpha included.
///
/// Parameters:
/// - `src_data`: premultiplied BGRA pixel data (row-major, `src_w` pixels wide)
/// - `src_w`: source width (`logical_width`, the long axis)
/// - `src_h`: source height (visible rows to process, may be < pixmap height)
/// - `dst_w`: destination width after rotation (`fb_height`)
/// - `dst_h`: destination height after rotation (`logical_width`)
/// - `dst`: output buffer, must be at least `dst_w * dst_h * 4` bytes
pub fn rotate_and_convert_into(
    src_data: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
    dst: &mut [u8],
) {
    let Some(dst) = dst.get_mut(..dst_w * dst_h * 4) else {
        return;
    };
    // Padding pixels beyond src_h need to be black, including alpha.
    dst.fill(0);

    let pad = dst_w.saturating_sub(src_h);

    for (dy, dst_row) in dst.chunks_exact_mut(dst_w * 4).enumerate() {
        let Some(body) = dst_row.get_mut(pad * 4..) else {
            continue;
        };
        for (out, sy) in body.chunks_exact_mut(4).zip((0..src_h).rev()) {
            let base = (sy * src_w + dy) * 4;
            let Some(&[blue, green, red, alpha]) = src_data.get(base..base + 4) else {
                continue;
            };

            let (blue, green, red) = if alpha == 0 {
                (0, 0, 0)
            } else if alpha == u8::MAX {
                (blue, green, red)
            } else {
                (
                    unpremultiply_channel(blue, alpha),
                    unpremultiply_channel(green, alpha),
                    unpremultiply_channel(red, alpha),
                )
            };

            out.copy_from_slice(&[blue, green, red, 0xFF]);
        }
    }
}

/// Allocating version for tests. Make this `pub` (and drop the `cfg`) if a
/// benchmark target is ever added — see the note at the top of this module.
#[cfg(test)]
fn rotate_and_convert(
    src_data: &[u8],
    src_w: usize,
    src_h: usize,
    dst_w: usize,
    dst_h: usize,
) -> Vec<u8> {
    let mut dst = vec![0u8; dst_w * dst_h * 4];
    rotate_and_convert_into(src_data, src_w, src_h, dst_w, dst_h, &mut dst);
    dst
}
#[cfg(test)]
mod tests {
    use super::rotate_and_convert;

    /// Helper: create a premultiplied BGRA pixel, matching what
    /// `iced_tiny_skia` writes into the pixmap.
    fn px(b: u8, g: u8, r: u8, a: u8) -> [u8; 4] {
        [b, g, r, a]
    }

    /// Build a flat BGRA buffer from a 2D array of pixels (row-major).
    fn build_src(rows: &[&[[u8; 4]]]) -> Vec<u8> {
        rows.iter()
            .flat_map(|row| row.iter().flat_map(|p| p.iter().copied()))
            .collect()
    }

    /// Read a BGRX pixel from output buffer at (x, y) with given stride.
    fn read_dst(buf: &[u8], x: usize, y: usize, stride: usize) -> (u8, u8, u8, u8) {
        let idx = (y * stride + x) * 4;
        (buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]) // B, G, R, X
    }

    #[test]
    fn test_4x3_rotation_layout() {
        // Source: 4 wide x 3 tall, each pixel unique for tracking
        // Pixel values are fully opaque (a=255) so RGB passes through
        let row0: &[[u8; 4]] = &[
            px(10, 20, 30, 255),
            px(11, 21, 31, 255),
            px(12, 22, 32, 255),
            px(13, 23, 33, 255),
        ];
        let row1: &[[u8; 4]] = &[
            px(40, 50, 60, 255),
            px(41, 51, 61, 255),
            px(42, 52, 62, 255),
            px(43, 53, 63, 255),
        ];
        let row2: &[[u8; 4]] = &[
            px(70, 80, 90, 255),
            px(71, 81, 91, 255),
            px(72, 82, 92, 255),
            px(73, 83, 93, 255),
        ];
        let src = build_src(&[row0, row1, row2]);

        let src_w = 4;
        let src_h = 3;
        let dst_w = 3; // fb_height = src_h
        let dst_h = 4; // logical_width = src_w

        let out = rotate_and_convert(&src, src_w, src_h, dst_w, dst_h);

        // Output is dst_w * dst_h * 4 bytes
        assert_eq!(out.len(), dst_w * dst_h * 4);

        // 90 CW rotation: src(sx, sy) -> dst(src_h - 1 - sy, sx)
        // So src(0,0) -> dst(2, 0), src(1,0) -> dst(2,1), etc.
        // dst(dx, dy): sx = dy, sy = dst_w - 1 - dx

        // Check src(0,0) = (10,20,30) -> dst(2,0) in BGRX
        let (b, g, r, x) = read_dst(&out, 2, 0, dst_w);
        assert_eq!((b, g, r, x), (10, 20, 30, 0xFF));

        // Check src(3,2) = (73,83,93) -> dst(0, 3)
        let (b, g, r, x) = read_dst(&out, 0, 3, dst_w);
        assert_eq!((b, g, r, x), (73, 83, 93, 0xFF));

        // Check src(1,1) = (41,51,61) -> dst(1, 1)
        let (b, g, r, x) = read_dst(&out, 1, 1, dst_w);
        assert_eq!((b, g, r, x), (41, 51, 61, 0xFF));
    }

    #[test]
    fn test_transparent_pixels_produce_black() {
        let row: &[[u8; 4]] = &[px(0, 0, 0, 0), px(128, 64, 32, 0)];
        let src = build_src(&[row]);

        let out = rotate_and_convert(&src, 2, 1, 1, 2);
        // dst(0,0): sy = 0, sx = 0 -> src(0,0) transparent
        let (b, g, r, x) = read_dst(&out, 0, 0, 1);
        assert_eq!((r, g, b, x), (0, 0, 0, 0xFF));

        // dst(0,1): sy = 0, sx = 1 -> src(1,0) transparent
        let (b, g, r, x) = read_dst(&out, 0, 1, 1);
        assert_eq!((r, g, b, x), (0, 0, 0, 0xFF));
    }

    #[test]
    fn test_opaque_pixels_passthrough() {
        let row: &[[u8; 4]] = &[px(200, 100, 50, 255)];
        let src = build_src(&[row]);

        let out = rotate_and_convert(&src, 1, 1, 1, 1);
        let (b, g, r, x) = read_dst(&out, 0, 0, 1);
        assert_eq!((b, g, r, x), (200, 100, 50, 0xFF));
    }

    #[test]
    fn test_semitransparent_unpremultiply() {
        // Premultiplied: r=128, g=0, b=0, a=128
        // Unpacked: r = 128 / (128/255.0) ~= 254..255 due to float rounding
        // The division 128.0 / (128.0/255.0) can yield 254 or 255 depending on
        // floating-point intermediates. We accept either.
        let row: &[[u8; 4]] = &[px(128, 0, 0, 128)];
        let src = build_src(&[row]);

        let out = rotate_and_convert(&src, 1, 1, 1, 1);
        let (b, g, r, x) = read_dst(&out, 0, 0, 1);
        assert!(b >= 254, "b should be ~255 after unpremultiply, got {b}");
        assert_eq!((g, r, x), (0, 0, 0xFF));

        // Also test a case where unpremultiply clearly changes the value:
        // Premultiplied: r=64, g=32, b=16, a=128 -> unpacked: r=128, g=64, b=32
        let row2: &[[u8; 4]] = &[px(64, 32, 16, 128)];
        let src2 = build_src(&[row2]);
        let out2 = rotate_and_convert(&src2, 1, 1, 1, 1);
        let (b2, g2, r2, x2) = read_dst(&out2, 0, 0, 1);
        // Allow +/-1 for float rounding
        assert!((127..=129).contains(&b2), "b should be ~128, got {b2}");
        assert!((63..=65).contains(&g2), "g should be ~64, got {g2}");
        assert!((31..=33).contains(&r2), "r should be ~32, got {r2}");
        assert_eq!(x2, 0xFF);
    }

    #[test]
    fn test_output_dimensions_transposed() {
        // 5 wide x 2 tall -> dst should be (2 wide x 5 tall)
        let row0: &[[u8; 4]] = &[px(0, 0, 0, 255); 5];
        let row1: &[[u8; 4]] = &[px(0, 0, 0, 255); 5];
        let src = build_src(&[row0, row1]);

        let dst_w = 2;
        let dst_h = 5;
        let out = rotate_and_convert(&src, 5, 2, dst_w, dst_h);
        assert_eq!(out.len(), dst_w * dst_h * 4);
    }

    #[test]
    fn test_padding_rows_stay_black() {
        // src_h < dst_w means some destination pixels map to rows beyond visible area
        // src: 2 wide x 1 tall, dst_w = 3 (fb_height > vis_h), dst_h = 2
        let row: &[[u8; 4]] = &[px(255, 128, 64, 255), px(200, 100, 50, 255)];
        // Need source buffer to be 2 wide * 3 tall (dst_w rows) to avoid OOB
        // Actually src_data comes from pixmap which is src_w * fb_h, so pad with zeros
        let mut src = build_src(&[row]);
        // Add 2 more rows of zeros (for fb_h=3 total)
        src.extend(vec![0u8; 2 * 4 * 2]);

        let out = rotate_and_convert(&src, 2, 1, 3, 2);
        // dst(2, 0): sy = 3-1-2 = 0, sx = 0 -> src(0,0) visible
        let (b, g, r, x) = read_dst(&out, 2, 0, 3);
        assert_eq!((b, g, r, x), (255, 128, 64, 0xFF));

        // dst(1, 0): sy = 3-1-1 = 1, sx = 0 -> sy=1 >= src_h=1, padding -> black/zero
        let (b, g, r, x) = read_dst(&out, 1, 0, 3);
        assert_eq!((r, g, b, x), (0, 0, 0, 0));

        // dst(0, 0): sy = 3-1-0 = 2, sx = 0 -> sy=2 >= src_h=1, padding -> black/zero
        let (b, g, r, x) = read_dst(&out, 0, 0, 3);
        assert_eq!((r, g, b, x), (0, 0, 0, 0));
    }
}
