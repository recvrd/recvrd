//! Convert cover art to terminal half-block cells.
//!
//! The half-block renderer is the fallback. When the terminal supports a
//! graphics protocol, the decoded image is handed to `ratatui-image`
//! instead (see `App::cover_protocol`).

use image::DynamicImage;
use image::imageops::FilterType;
use ratatui::style::Color;

/// One terminal cell. The upper half block uses `top` as the foreground and
/// `bottom` as the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverCell {
    /// Color of the upper half.
    pub top: Color,
    /// Color of the lower half.
    pub bottom: Color,
}

/// A cover image that is ready to render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverArt {
    /// Width in terminal cells.
    pub width: u16,
    /// Height in terminal cells.
    pub height: u16,
    /// Cells in row-major order.
    pub cells: Vec<CoverCell>,
}

impl CoverArt {
    /// Return the cells for one row.
    pub fn row(&self, row: usize) -> &[CoverCell] {
        let start = row * usize::from(self.width);
        let end = start + usize::from(self.width);
        &self.cells[start..end]
    }
}

/// Decode cover art bytes into an image for the graphics-protocol renderer.
/// Returns `None` when the bytes are not a decodable image.
pub fn decode_image(bytes: &[u8]) -> Option<DynamicImage> {
    image::load_from_memory(bytes).ok()
}

/// Resize a decoded image to the requested half-block cell size.
pub fn render_image(image: &DynamicImage, width: u16, height: u16) -> CoverArt {
    let image = image.to_rgb8();
    let pixel_width = u32::from(width).max(1);
    let pixel_height = u32::from(height).max(1) * 2;
    let resized = image::imageops::resize(&image, pixel_width, pixel_height, FilterType::Triangle);
    let mut cells = Vec::with_capacity(usize::from(width) * usize::from(height));
    for row in 0..u32::from(height) {
        for column in 0..pixel_width {
            let top = resized.get_pixel(column, row * 2);
            let bottom = resized.get_pixel(column, row * 2 + 1);
            cells.push(CoverCell {
                top: Color::Rgb(top[0], top[1], top[2]),
                bottom: Color::Rgb(bottom[0], bottom[1], bottom[2]),
            });
        }
    }
    CoverArt {
        width,
        height,
        cells,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::RgbImage;

    fn encode(image: &DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut bytes = Vec::new();
        image
            .write_to(&mut std::io::Cursor::new(&mut bytes), format)
            .unwrap();
        bytes
    }

    #[test]
    fn decodes_png_jpeg_and_webp_covers() {
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(2, 2, image::Rgb([10, 20, 30])));
        for format in [
            image::ImageFormat::Png,
            image::ImageFormat::Jpeg,
            image::ImageFormat::WebP,
        ] {
            let bytes = encode(&image, format);
            let decoded =
                decode_image(&bytes).unwrap_or_else(|| panic!("could not decode {format:?}"));
            assert_eq!((decoded.width(), decoded.height()), (2, 2));
        }
    }

    #[test]
    fn resizes_to_the_requested_cell_count() {
        let image = RgbImage::from_pixel(4, 4, image::Rgb([10, 20, 30]));
        let art = render_image(&image::DynamicImage::ImageRgb8(image), 3, 2);
        assert_eq!(art.width, 3);
        assert_eq!(art.height, 2);
        assert_eq!(art.cells.len(), 6);
        assert_eq!(art.row(0).len(), 3);
        assert_eq!(art.cells[0].top, Color::Rgb(10, 20, 30));
        assert_eq!(art.cells[0].bottom, Color::Rgb(10, 20, 30));
    }
}
