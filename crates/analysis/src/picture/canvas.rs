//! An RGB raster with the few things the picture draws: rectangles, lines
//! and text in the built-in font.

use super::font::{pixel, GLYPH_HEIGHT, GLYPH_WIDTH};

pub(crate) type Colour = [u8; 3];

pub(crate) const BLACK: Colour = [0, 0, 0];
pub(crate) const WHITE: Colour = [255, 255, 255];
pub(crate) const GRID: Colour = [215, 215, 215];
pub(crate) const MUTED: Colour = [110, 110, 110];

/// Text is drawn at twice the font's size: 10 × 14 pixels a character,
/// readable in a 1600-pixel picture shown at half size.
pub(crate) const TEXT_SCALE: usize = 2;
/// One character's advance and one line's height (px).
pub(crate) const CHARACTER_WIDTH: usize = (GLYPH_WIDTH + 1) * TEXT_SCALE;
pub(crate) const LINE_HEIGHT: usize = (GLYPH_HEIGHT + 3) * TEXT_SCALE;

pub(crate) struct Canvas {
    pub(crate) width: usize,
    pub(crate) height: usize,
    pub(crate) pixels: Vec<u8>,
}

impl Canvas {
    pub(crate) fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            pixels: vec![255; width * height * 3],
        }
    }

    pub(crate) fn set(&mut self, x: isize, y: isize, colour: Colour) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
            return;
        }
        let start = (y as usize * self.width + x as usize) * 3;
        self.pixels[start..start + 3].copy_from_slice(&colour);
    }

    pub(crate) fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, colour: Colour) {
        for row in y..y + height {
            for column in x..x + width {
                self.set(column as isize, row as isize, colour);
            }
        }
    }

    pub(crate) fn outline(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        colour: Colour,
    ) {
        self.fill(x, y, width, 1, colour);
        self.fill(x, y + height - 1, width, 1, colour);
        self.fill(x, y, 1, height, colour);
        self.fill(x + width - 1, y, 1, height, colour);
    }

    /// A straight line, Bresenham's.
    pub(crate) fn line(&mut self, from: (isize, isize), to: (isize, isize), colour: Colour) {
        let (mut x, mut y) = from;
        let delta_x = (to.0 - x).abs();
        let delta_y = -(to.1 - y).abs();
        let step_x = if x < to.0 { 1 } else { -1 };
        let step_y = if y < to.1 { 1 } else { -1 };
        let mut error = delta_x + delta_y;
        loop {
            self.set(x, y, colour);
            if x == to.0 && y == to.1 {
                break;
            }
            let doubled = 2 * error;
            if doubled >= delta_y {
                error += delta_y;
                x += step_x;
            }
            if doubled <= delta_x {
                error += delta_x;
                y += step_y;
            }
        }
    }

    /// A horizontal line drawn one pixel in every `period`.
    pub(crate) fn dotted(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        period: usize,
        colour: Colour,
    ) {
        for column in (x..x + width).step_by(period.max(1)) {
            self.set(column as isize, y as isize, colour);
        }
    }

    /// `text` with its top-left corner at `x`, `y`.
    pub(crate) fn text(&mut self, x: usize, y: usize, text: &str, colour: Colour) {
        for (index, character) in text.chars().enumerate() {
            let left = x + index * CHARACTER_WIDTH;
            for column in 0..GLYPH_WIDTH {
                for row in 0..GLYPH_HEIGHT {
                    if pixel(character, column, row) {
                        self.fill(
                            left + column * TEXT_SCALE,
                            y + row * TEXT_SCALE,
                            TEXT_SCALE,
                            TEXT_SCALE,
                            colour,
                        );
                    }
                }
            }
        }
    }

    /// `text` ending at `right`.
    pub(crate) fn text_right(&mut self, right: usize, y: usize, text: &str, colour: Colour) {
        let width = text.chars().count() * CHARACTER_WIDTH;
        self.text(right.saturating_sub(width), y, text, colour);
    }

    /// `text` centred on `centre`.
    pub(crate) fn text_centred(&mut self, centre: usize, y: usize, text: &str, colour: Colour) {
        let width = text.chars().count() * CHARACTER_WIDTH;
        self.text(centre.saturating_sub(width / 2), y, text, colour);
    }

    /// The raster as a PNG file.
    pub(crate) fn png(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, self.width as u32, self.height as u32);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        // Writing to memory with a valid size and colour type cannot fail.
        let mut writer = encoder.write_header().expect("a valid header");
        writer
            .write_image_data(&self.pixels)
            .expect("as many bytes as the header says");
        writer.finish().expect("written to memory");
        bytes
    }
}
