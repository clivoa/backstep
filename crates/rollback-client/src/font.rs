//! A 3x5 bitmap font.
//!
//! Hand-rolled because the alternative is `SDL2_ttf` plus a font file, and the
//! overlay only ever draws digits, uppercase letters and a handful of symbols.
//! Shipping a 200-byte table beats shipping a font dependency that has to find
//! a TTF on the user's machine at runtime.
//!
//! Each glyph is five rows; the low three bits of each byte are the pixels,
//! most significant of the three on the left.

/// Glyph width and height, in font pixels.
pub const GLYPH_W: u32 = 3;
pub const GLYPH_H: u32 = 5;
/// Blank columns between glyphs.
pub const TRACKING: u32 = 1;

/// Rows of a glyph, or `None` for a character with no glyph.
pub fn glyph(c: char) -> Option<[u8; 5]> {
    let c = c.to_ascii_uppercase();
    Some(match c {
        ' ' => [0b000, 0b000, 0b000, 0b000, 0b000],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b011, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'A' => [0b111, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b111, 0b100, 0b100, 0b100, 0b111],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b111, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b111, 0b100, 0b100],
        'G' => [0b111, 0b100, 0b101, 0b101, 0b111],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'J' => [0b001, 0b001, 0b001, 0b101, 0b111],
        'K' => [0b101, 0b101, 0b110, 0b101, 0b101],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'M' => [0b101, 0b111, 0b111, 0b101, 0b101],
        'N' => [0b101, 0b111, 0b111, 0b111, 0b101],
        'O' => [0b111, 0b101, 0b101, 0b101, 0b111],
        'P' => [0b111, 0b101, 0b111, 0b100, 0b100],
        'Q' => [0b111, 0b101, 0b101, 0b111, 0b011],
        'R' => [0b111, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b111, 0b100, 0b111, 0b001, 0b111],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'U' => [0b101, 0b101, 0b101, 0b101, 0b111],
        'V' => [0b101, 0b101, 0b101, 0b101, 0b010],
        'W' => [0b101, 0b101, 0b111, 0b111, 0b101],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        'Z' => [0b111, 0b001, 0b010, 0b100, 0b111],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        ',' => [0b000, 0b000, 0b000, 0b010, 0b100],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '+' => [0b000, 0b010, 0b111, 0b010, 0b000],
        '/' => [0b001, 0b001, 0b010, 0b100, 0b100],
        '%' => [0b101, 0b001, 0b010, 0b100, 0b101],
        '=' => [0b000, 0b111, 0b000, 0b111, 0b000],
        '!' => [0b010, 0b010, 0b010, 0b000, 0b010],
        '?' => [0b111, 0b001, 0b011, 0b000, 0b010],
        '(' => [0b001, 0b010, 0b010, 0b010, 0b001],
        ')' => [0b100, 0b010, 0b010, 0b010, 0b100],
        '[' => [0b011, 0b010, 0b010, 0b010, 0b011],
        ']' => [0b110, 0b010, 0b010, 0b010, 0b110],
        '#' => [0b101, 0b111, 0b101, 0b111, 0b101],
        '*' => [0b000, 0b101, 0b010, 0b101, 0b000],
        _ => return None,
    })
}

/// Width of `text` at `scale`, in screen pixels.
pub fn text_width(text: &str, scale: u32) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let glyphs = text.chars().count() as u32;
    (glyphs * (GLYPH_W + TRACKING) - TRACKING) * scale
}

/// The lit pixels of `text`, as `(x, y)` offsets in font pixels.
///
/// Returned rather than drawn so the renderer stays the only thing that knows
/// about SDL, which is also what makes this testable without a display.
pub fn pixels(text: &str) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut pen_x = 0u32;
    for c in text.chars() {
        // A character with no glyph is drawn as a blank, not skipped: dropping
        // it would silently shift the rest of the line.
        let rows = glyph(c).unwrap_or([0; 5]);
        for (y, row) in rows.iter().enumerate() {
            for x in 0..GLYPH_W {
                if row & (1 << (GLYPH_W - 1 - x)) != 0 {
                    out.push((pen_x + x, y as u32));
                }
            }
        }
        pen_x += GLYPH_W + TRACKING;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_glyph_fits_the_cell() {
        for c in ('A'..='Z').chain('0'..='9') {
            let rows = glyph(c).unwrap_or_else(|| panic!("{c} has no glyph"));
            for row in rows {
                assert_eq!(row & !0b111, 0, "{c} has pixels outside the 3-bit cell");
            }
        }
    }

    #[test]
    fn lowercase_maps_to_uppercase() {
        assert_eq!(glyph('a'), glyph('A'));
        assert_eq!(glyph('z'), glyph('Z'));
    }

    #[test]
    fn an_unknown_character_has_no_glyph_but_still_takes_space() {
        assert!(glyph('\u{263A}').is_none());
        assert_eq!(text_width("AB", 1), text_width("A\u{263A}", 1));
        assert_eq!(pixels("\u{263A}").len(), 0, "but draws nothing");
    }

    #[test]
    fn width_accounts_for_tracking_and_scale() {
        assert_eq!(text_width("", 2), 0);
        assert_eq!(text_width("A", 1), GLYPH_W);
        assert_eq!(text_width("AB", 1), GLYPH_W * 2 + TRACKING);
        assert_eq!(text_width("AB", 3), (GLYPH_W * 2 + TRACKING) * 3);
    }

    #[test]
    fn a_space_lights_nothing_and_a_full_block_lights_everything() {
        assert!(pixels(" ").is_empty());
        // '8' is the densest digit; check it lights the corners of the cell.
        let eight = pixels("8");
        assert!(eight.contains(&(0, 0)));
        assert!(eight.contains(&(2, 0)));
        assert!(eight.contains(&(0, 4)));
        assert!(eight.contains(&(2, 4)));
    }

    #[test]
    fn glyphs_advance_left_to_right() {
        let two = pixels("11");
        let first_max_x = two.iter().filter(|(_, y)| *y == 0).map(|(x, _)| *x).max();
        assert!(
            two.iter().any(|(x, _)| *x >= GLYPH_W + TRACKING),
            "the second glyph must be to the right of the first (max x of row 0: {first_max_x:?})"
        );
    }

    #[test]
    fn no_pixel_escapes_the_line_height() {
        for (_, y) in pixels("ROLLBACK 60HZ 100%") {
            assert!(y < GLYPH_H);
        }
    }
}
