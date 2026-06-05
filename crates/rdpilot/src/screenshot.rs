//! [`Screenshot`] — an owned RGBA framebuffer, plus the [`Rect`] used to crop it.
//!
//! `Screenshot` is the public capture type (D-09). The `image` crate is used
//! internally to encode PNG bytes and **never** appears in a public signature
//! (D-11). Cropping is row-major stride math over the retained RGBA buffer, so
//! it never re-decodes a PNG; an out-of-bounds [`Rect`] returns
//! [`Error::CropOutOfBounds`] and never panics (API-01, threat T-02-01).

use std::io::Cursor;

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

use crate::error::{Error, Result};

/// Bytes per pixel in the RGBA32 framebuffer (R, G, B, A — 8 bits each).
const BYTES_PER_PIXEL: usize = 4;

/// An owned, decoded screenshot in tightly-packed RGBA32.
///
/// `rgba` is row-major, `width * height * 4` bytes, with no stride padding —
/// exactly the layout IronRDP's `DecodedImage` produces for the bitmap / RLE /
/// RDP6 / RemoteFX paths (no YUV conversion needed).
#[derive(Clone, Debug)]
pub struct Screenshot {
    /// Image width, in pixels.
    pub width: u32,
    /// Image height, in pixels.
    pub height: u32,
    /// Tightly-packed RGBA32 pixel data (`width * height * 4` bytes).
    pub rgba: Vec<u8>,
}

/// A rectangle in pixel coordinates, used to crop a [`Screenshot`].
///
/// The caller supplies the rectangle (CAP-01 #3); deriving real window geometry
/// is a later phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Origin x (left edge), in pixels.
    pub x: u32,
    /// Origin y (top edge), in pixels.
    pub y: u32,
    /// Width, in pixels.
    pub w: u32,
    /// Height, in pixels.
    pub h: u32,
}

impl Screenshot {
    /// Construct a screenshot from raw RGBA32 bytes.
    ///
    /// Returns [`Error::Decode`] if `rgba` is not exactly
    /// `width * height * 4` bytes, so a malformed buffer can never produce an
    /// out-of-bounds read later.
    pub fn from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(BYTES_PER_PIXEL))
            .ok_or_else(|| {
                Error::Decode(format!(
                    "image dimensions {width}x{height} overflow the buffer size"
                ))
            })?;
        if rgba.len() != expected {
            return Err(Error::Decode(format!(
                "RGBA buffer length {} does not match {width}x{height} (expected {expected})",
                rgba.len()
            )));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Encode the screenshot as PNG bytes.
    ///
    /// The `image` crate is an internal detail — only owned `Vec<u8>` PNG bytes
    /// cross the public boundary (D-11). The caller is responsible for writing
    /// the bytes wherever it wants.
    pub fn to_png(&self) -> Result<Vec<u8>> {
        let mut out = Cursor::new(Vec::new());
        let encoder = PngEncoder::new(&mut out);
        encoder
            .write_image(
                &self.rgba,
                self.width,
                self.height,
                ExtendedColorType::Rgba8,
            )
            .map_err(|e| Error::Encode(e.to_string()))?;
        Ok(out.into_inner())
    }

    /// Crop to `rect`, returning a new owned [`Screenshot`].
    ///
    /// Pure row-major stride math over the retained RGBA buffer (no PNG
    /// re-decode). A rectangle that exceeds the image bounds — or a zero-area
    /// rectangle — returns [`Error::CropOutOfBounds`] and never panics
    /// (API-01, threat T-02-01).
    pub fn crop(&self, rect: Rect) -> Result<Screenshot> {
        // Reject zero-area and any rectangle whose far edge exceeds the image.
        // `checked_add` guards against u32 overflow on the far-edge computation.
        let right = rect.x.checked_add(rect.w);
        let bottom = rect.y.checked_add(rect.h);
        let in_bounds = rect.w > 0
            && rect.h > 0
            && right.is_some_and(|r| r <= self.width)
            && bottom.is_some_and(|b| b <= self.height);
        if !in_bounds {
            return Err(Error::crop_out_of_bounds(&rect, self.width, self.height));
        }

        let row_bytes = (rect.w as usize) * BYTES_PER_PIXEL;
        let stride = (self.width as usize) * BYTES_PER_PIXEL;
        let mut out = Vec::with_capacity(row_bytes * rect.h as usize);
        for y in rect.y..rect.y + rect.h {
            let row_start = (y as usize) * stride + (rect.x as usize) * BYTES_PER_PIXEL;
            // Bounds already validated above, so this slice is always valid.
            out.extend_from_slice(&self.rgba[row_start..row_start + row_bytes]);
        }

        Ok(Screenshot {
            width: rect.w,
            height: rect.h,
            rgba: out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `w x h` image where each pixel's RGBA encodes its (x, y):
    /// R = x, G = y, B = x + y, A = 255. Lets crop assertions check exact
    /// sub-pixels via row-major stride math.
    fn checkerboard(w: u32, h: u32) -> Screenshot {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                rgba.push(x as u8);
                rgba.push(y as u8);
                rgba.push((x + y) as u8);
                rgba.push(255);
            }
        }
        Screenshot::from_rgba(w, h, rgba).expect("valid buffer")
    }

    #[test]
    fn crop_inside_bounds_has_expected_dims_and_len() {
        let img = checkerboard(10, 8);
        let cropped = img
            .crop(Rect {
                x: 2,
                y: 3,
                w: 4,
                h: 2,
            })
            .expect("in-bounds crop succeeds");
        assert_eq!(cropped.width, 4);
        assert_eq!(cropped.height, 2);
        assert_eq!(cropped.rgba.len(), (4 * 2 * 4) as usize);
    }

    #[test]
    fn crop_extracts_correct_subpixels() {
        let img = checkerboard(10, 8);
        let rect = Rect {
            x: 2,
            y: 3,
            w: 4,
            h: 2,
        };
        let cropped = img.crop(rect).expect("in-bounds crop succeeds");
        // First pixel of the crop is source (x=2, y=3): R=2, G=3, B=5, A=255.
        assert_eq!(&cropped.rgba[0..4], &[2, 3, 5, 255]);
        // Pixel at crop (3, 1) maps to source (x=5, y=4): R=5, G=4, B=9, A=255.
        let (cx, cy) = (3u32, 1u32);
        let idx = ((cy * cropped.width + cx) * 4) as usize;
        assert_eq!(&cropped.rgba[idx..idx + 4], &[5, 4, 9, 255]);
    }

    #[test]
    fn crop_out_of_bounds_returns_err_never_panics() {
        let img = checkerboard(10, 8);
        // Far edge exceeds width.
        let err = img.crop(Rect {
            x: 8,
            y: 0,
            w: 4,
            h: 1,
        });
        assert!(matches!(err, Err(Error::CropOutOfBounds { .. })));

        // Zero-area rectangle is rejected too.
        let zero = img.crop(Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 1,
        });
        assert!(matches!(zero, Err(Error::CropOutOfBounds { .. })));

        // Far edge exceeds height.
        let tall = img.crop(Rect {
            x: 0,
            y: 7,
            w: 1,
            h: 4,
        });
        assert!(matches!(tall, Err(Error::CropOutOfBounds { .. })));
    }

    #[test]
    fn to_png_returns_png_magic_bytes() {
        // 2x2 RGBA image.
        let img = Screenshot::from_rgba(
            2,
            2,
            vec![
                255, 0, 0, 255, // red
                0, 255, 0, 255, // green
                0, 0, 255, 255, // blue
                255, 255, 0, 255, // yellow
            ],
        )
        .expect("valid buffer");
        let png = img.to_png().expect("encode succeeds");
        assert!(!png.is_empty());
        // PNG magic: 0x89 'P' 'N' 'G'.
        assert_eq!(&png[0..4], &[0x89, b'P', b'N', b'G']);
    }

    #[test]
    fn from_rgba_rejects_mismatched_buffer() {
        let bad = Screenshot::from_rgba(2, 2, vec![0; 3]);
        assert!(matches!(bad, Err(Error::Decode(_))));
    }
}
