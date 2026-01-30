//! XRGB8888 to 8bpp grayscale conversion using BT.601 luminance.

/// Convert XRGB8888 pixel data to 8bpp grayscale.
///
/// Uses integer BT.601 coefficients: gray = (R*77 + G*150 + B*29) >> 8
///
/// `src` is the source XRGB8888 buffer, `src_stride` is bytes per row,
/// `dst` is the destination 8bpp buffer (width*height bytes),
/// laid out row-major with stride = `width`.
/// If `bgr` is true, byte order is R,G,B,X (XBGR8888 in little-endian);
/// if false, byte order is B,G,R,X (XRGB8888 in little-endian).
pub fn xrgb8888_to_gray(
    src: &[u8],
    src_stride: usize,
    dst: &mut [u8],
    width: usize,
    height: usize,
    bgr: bool,
) {
    for y in 0..height {
        let src_row = &src[y * src_stride..];
        let dst_row = &mut dst[y * width..y * width + width];
        for x in 0..width {
            let off = x * 4;
            let (r, g, b) = if bgr {
                // XBGR8888 little-endian: bytes are R, G, B, X
                (src_row[off] as u32, src_row[off + 1] as u32, src_row[off + 2] as u32)
            } else {
                // XRGB8888 little-endian: bytes are B, G, R, X
                (src_row[off + 2] as u32, src_row[off + 1] as u32, src_row[off] as u32)
            };
            dst_row[x] = ((r * 77 + g * 150 + b * 29) >> 8) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_white_pixel_xrgb() {
        let src = [0xFF, 0xFF, 0xFF, 0x00]; // XRGB: B,G,R,X
        let mut dst = [0u8; 1];
        xrgb8888_to_gray(&src, 4, &mut dst, 1, 1, false);
        assert_eq!(dst[0], 255);
    }

    #[test]
    fn test_black_pixel() {
        let src = [0x00, 0x00, 0x00, 0x00];
        let mut dst = [0u8; 1];
        xrgb8888_to_gray(&src, 4, &mut dst, 1, 1, false);
        assert_eq!(dst[0], 0);
    }

    #[test]
    fn test_white_pixel_xbgr() {
        let src = [0xFF, 0xFF, 0xFF, 0x00]; // XBGR: R,G,B,X
        let mut dst = [0u8; 1];
        xrgb8888_to_gray(&src, 4, &mut dst, 1, 1, true);
        assert_eq!(dst[0], 255);
    }
}
