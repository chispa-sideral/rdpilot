//! RGBA to I420 (planar 4:2:0) conversion: integer BT.709 coefficients,
//! limited range, chroma averaged over each 2x2 block. Odd sizes are
//! handled by averaging the pixels that exist.

/// One frame in I420.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct I420 {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) y: Vec<u8>,
    pub(crate) u: Vec<u8>,
    pub(crate) v: Vec<u8>,
}

impl I420 {
    /// Width of the chroma planes.
    pub(crate) fn chroma_width(&self) -> usize {
        self.width.div_ceil(2)
    }
}

fn clamp(v: i32) -> u8 {
    u8::try_from(v.clamp(0, 255)).unwrap_or(0)
}

/// Convert tightly packed RGBA (`width * height * 4` bytes) to I420.
/// Missing trailing bytes read as black.
pub(crate) fn from_rgba(rgba: &[u8], width: usize, height: usize) -> I420 {
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let mut y = vec![16_u8; width * height];
    let mut u = vec![128_u8; cw * ch];
    let mut v = vec![128_u8; cw * ch];
    let row_bytes = width * 4;
    let row = |j: usize| -> &[u8] {
        let start = (j * row_bytes).min(rgba.len());
        let end = (start + row_bytes).min(rgba.len());
        &rgba[start..end]
    };
    for (j, out) in y.chunks_exact_mut(width.max(1)).enumerate().take(height) {
        for (px, luma) in row(j).chunks_exact(4).zip(out.iter_mut()) {
            let (r, g, b) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
            *luma = clamp(((47 * r + 157 * g + 16 * b + 128) >> 8) + 16);
        }
    }
    for j in 0..ch {
        let top = row(2 * j);
        let bottom = if 2 * j + 1 < height {
            row(2 * j + 1)
        } else {
            &[]
        };
        for i in 0..cw {
            let (mut r, mut g, mut b, mut n) = (0, 0, 0, 0);
            for line in [top, bottom] {
                for x in [2 * i, 2 * i + 1] {
                    if x < width {
                        if let Some(px) = line.get(x * 4..x * 4 + 3) {
                            r += i32::from(px[0]);
                            g += i32::from(px[1]);
                            b += i32::from(px[2]);
                            n += 1;
                        } else if !line.is_empty() || 2 * j + 1 < height {
                            n += 1; // a missing pixel reads as black
                        }
                    }
                }
            }
            let n = n.max(1);
            let (r, g, b) = (r / n, g / n, b / n);
            u[j * cw + i] = clamp(((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128);
            v[j * cw + i] = clamp(((112 * r - 102 * g - 10 * b + 128) >> 8) + 128);
        }
    }
    I420 {
        width,
        height,
        y,
        u,
        v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h)
            .flat_map(|_| [rgb[0], rgb[1], rgb[2], 255])
            .collect()
    }

    #[test]
    fn black_white_and_grey_map_to_limited_range() {
        let black = from_rgba(&solid(4, 2, [0, 0, 0]), 4, 2);
        assert!(black.y.iter().all(|&v| v == 16));
        assert!(black.u.iter().chain(&black.v).all(|&v| v == 128));
        let white = from_rgba(&solid(4, 2, [255, 255, 255]), 4, 2);
        assert!(white.y.iter().all(|&v| (234..=236).contains(&v)));
        assert!(white
            .u
            .iter()
            .chain(&white.v)
            .all(|&v| (127..=129).contains(&v)));
    }

    #[test]
    fn odd_sizes_have_rounded_up_chroma_planes() {
        let f = from_rgba(&solid(5, 3, [200, 10, 10]), 5, 3);
        assert_eq!((f.y.len(), f.u.len(), f.v.len()), (15, 6, 6));
        assert_eq!(f.chroma_width(), 3);
        // Red: high V, low U; the partial right column and bottom row too.
        assert!(f.v.iter().all(|&v| v > 180), "{:?}", f.v);
        assert!(f.u.iter().all(|&u| u < 128), "{:?}", f.u);
    }

    #[test]
    fn short_input_reads_as_black() {
        let f = from_rgba(&[], 2, 2);
        assert!(f.y.iter().all(|&v| v == 16));
    }
}
