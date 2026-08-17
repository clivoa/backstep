//! Drawing: the arena, the emulator's framebuffer, and the overlay on top.

use anyhow::Result;
use rollback_arena::{Action, Arena, MAX_HEALTH, STAGE_MAX_X, STAGE_MIN_X};
use rollback_libretro::VideoFrame;
use sdl2::pixels::{Color, PixelFormatEnum};
use sdl2::rect::Rect;
use sdl2::render::{Canvas, Texture, TextureCreator};
use sdl2::video::{Window, WindowContext};

use crate::font;
use crate::overlay::Overlay;
use rollback_telemetry::MetricsSnapshot;

pub const WINDOW_W: u32 = 960;
pub const WINDOW_H: u32 = 600;

/// Height reserved at the bottom for the frame-history strip.
const STRIP_H: u32 = 18;
const OVERLAY_SCALE: u32 = 3;

/// Where the arena's floor sits, and how tall a fighter is drawn.
const FLOOR_Y: i32 = 430;
const FIGHTER_W: u32 = 28;
const FIGHTER_H: u32 = 72;

pub struct Renderer {
    canvas: Canvas<Window>,
}

impl Renderer {
    pub fn new(canvas: Canvas<Window>) -> Renderer {
        Renderer { canvas }
    }

    pub fn texture_creator(&self) -> TextureCreator<WindowContext> {
        self.canvas.texture_creator()
    }

    pub fn begin(&mut self) {
        self.canvas.set_draw_color(Color::RGB(16, 18, 24));
        self.canvas.clear();
    }

    pub fn present(&mut self) {
        self.canvas.present();
    }

    /// Draw the arena in pixel space.
    pub fn draw_arena(&mut self, arena: &Arena) -> Result<()> {
        let scale = |x_fixed: i32| -> i32 {
            let px = rollback_arena::fixed::to_px(x_fixed);
            let lo = rollback_arena::fixed::to_px(STAGE_MIN_X);
            let hi = rollback_arena::fixed::to_px(STAGE_MAX_X);
            let span = (hi - lo).max(1);
            40 + (px - lo) * (WINDOW_W as i32 - 80) / span
        };

        // Floor.
        self.canvas.set_draw_color(Color::RGB(40, 44, 56));
        self.canvas
            .fill_rect(Rect::new(0, FLOOR_Y, WINDOW_W, 8))
            .map_err(anyhow::Error::msg)?;

        for i in 0..2 {
            let f = &arena.fighters[i];
            let x = scale(f.x);
            let y = FLOOR_Y - FIGHTER_H as i32 - rollback_arena::fixed::to_px(f.y);
            let colour = match (i, f.action) {
                (_, Action::Hitstun) => Color::RGB(230, 90, 80),
                (_, Action::Blockstun) | (_, Action::Block) => Color::RGB(90, 150, 220),
                (_, Action::Attack) => Color::RGB(240, 200, 90),
                (_, Action::Special) => Color::RGB(200, 120, 230),
                (_, Action::Ko) => Color::RGB(90, 90, 90),
                (0, _) => Color::RGB(110, 200, 140),
                (_, _) => Color::RGB(200, 140, 110),
            };
            self.canvas.set_draw_color(colour);
            self.canvas
                .fill_rect(Rect::new(x - FIGHTER_W as i32 / 2, y, FIGHTER_W, FIGHTER_H))
                .map_err(anyhow::Error::msg)?;

            // A nub showing which way the fighter is facing.
            self.canvas.set_draw_color(Color::RGB(240, 240, 240));
            let nose = x + f.facing * (FIGHTER_W as i32 / 2 + 4);
            self.canvas
                .fill_rect(Rect::new(nose - 3, y + 12, 6, 6))
                .map_err(anyhow::Error::msg)?;
        }

        for p in arena.projectiles.iter().filter(|p| p.active) {
            self.canvas.set_draw_color(Color::RGB(250, 240, 160));
            let x = scale(p.x);
            let y = FLOOR_Y - 40 - rollback_arena::fixed::to_px(p.y);
            self.canvas
                .fill_rect(Rect::new(x - 6, y - 6, 12, 12))
                .map_err(anyhow::Error::msg)?;
        }

        self.draw_health_bars(arena)?;
        Ok(())
    }

    fn draw_health_bars(&mut self, arena: &Arena) -> Result<()> {
        let bar_w = 380u32;
        for (i, x) in [
            (0usize, 20i32),
            (1usize, WINDOW_W as i32 - 20 - bar_w as i32),
        ] {
            let health = arena.fighters[i].health.max(0) as u32;
            let filled = bar_w * health / MAX_HEALTH as u32;
            self.canvas.set_draw_color(Color::RGB(50, 54, 66));
            self.canvas
                .fill_rect(Rect::new(x, 20, bar_w, 18))
                .map_err(anyhow::Error::msg)?;
            self.canvas.set_draw_color(if i == 0 {
                Color::RGB(110, 200, 140)
            } else {
                Color::RGB(200, 140, 110)
            });
            // P2's bar drains from the right, arcade style.
            let fill_x = if i == 0 {
                x
            } else {
                x + (bar_w - filled) as i32
            };
            self.canvas
                .fill_rect(Rect::new(fill_x, 20, filled, 18))
                .map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }

    /// Blit the emulator's framebuffer, letterboxed to preserve its aspect.
    pub fn draw_video(&mut self, texture: &mut Texture<'_>, frame: &VideoFrame) -> Result<()> {
        if frame.is_empty() {
            return Ok(());
        }
        let pitch = frame.width as usize * 4;
        let bytes: &[u8] = bytemuck_cast(&frame.pixels);
        texture
            .update(None, bytes, pitch)
            .map_err(anyhow::Error::msg)?;

        let available_h = WINDOW_H - STRIP_H;
        let scale = ((WINDOW_W as f32 / frame.width as f32)
            .min(available_h as f32 / frame.height as f32))
        .max(0.01);
        let w = (frame.width as f32 * scale) as u32;
        let h = (frame.height as f32 * scale) as u32;
        let dst = Rect::new(
            ((WINDOW_W - w) / 2) as i32,
            ((available_h - h) / 2) as i32,
            w,
            h,
        );
        self.canvas
            .copy(texture, None, dst)
            .map_err(anyhow::Error::msg)?;
        Ok(())
    }

    /// Create the streaming texture the emulator's frames are blitted through.
    pub fn video_texture<'a>(
        creator: &'a TextureCreator<WindowContext>,
        width: u32,
        height: u32,
    ) -> Result<Texture<'a>> {
        Ok(creator.create_texture_streaming(
            // The host converts every core pixel format to XRGB8888, so the
            // texture only ever needs one layout.
            PixelFormatEnum::ARGB8888,
            width.max(1),
            height.max(1),
        )?)
    }

    /// Text lines in the corner and the frame-history strip at the bottom.
    pub fn draw_overlay(&mut self, overlay: &Overlay, snapshot: &MetricsSnapshot) -> Result<()> {
        let lines = overlay.lines(snapshot);
        let line_h = (font::GLYPH_H + 2) * OVERLAY_SCALE;
        let box_h = line_h * lines.len() as u32 + 8;
        let box_w = lines
            .iter()
            .map(|l| font::text_width(l, OVERLAY_SCALE))
            .max()
            .unwrap_or(0)
            + 12;

        self.canvas.set_draw_color(Color::RGBA(10, 12, 18, 220));
        self.canvas
            .fill_rect(Rect::new(12, 52, box_w, box_h))
            .map_err(anyhow::Error::msg)?;

        for (i, line) in lines.iter().enumerate() {
            let colour = if snapshot.desync && i == lines.len() - 1 {
                Color::RGB(240, 90, 80)
            } else {
                Color::RGB(220, 226, 236)
            };
            self.draw_text(
                line,
                18,
                56 + (i as u32 * line_h) as i32,
                OVERLAY_SCALE,
                colour,
            )?;
        }

        self.draw_strip(overlay)
    }

    fn draw_strip(&mut self, overlay: &Overlay) -> Result<()> {
        let strip = overlay.strip();
        if strip.is_empty() {
            return Ok(());
        }
        let y = (WINDOW_H - STRIP_H) as i32;
        let cell_w = (WINDOW_W / crate::overlay::HISTORY_LEN as u32).max(1);

        self.canvas.set_draw_color(Color::RGB(24, 26, 34));
        self.canvas
            .fill_rect(Rect::new(0, y, WINDOW_W, STRIP_H))
            .map_err(anyhow::Error::msg)?;

        for (i, mark) in strip.iter().enumerate() {
            let (r, g, b) = mark.colour();
            self.canvas.set_draw_color(Color::RGB(r, g, b));
            self.canvas
                .fill_rect(Rect::new(
                    i as i32 * cell_w as i32,
                    y + 2,
                    cell_w.saturating_sub(1).max(1),
                    STRIP_H - 4,
                ))
                .map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }

    /// Draw a string with the bitmap font.
    pub fn draw_text(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        scale: u32,
        colour: Color,
    ) -> Result<()> {
        self.canvas.set_draw_color(colour);
        let rects: Vec<Rect> = font::pixels(text)
            .into_iter()
            .map(|(px, py)| {
                Rect::new(
                    x + (px * scale) as i32,
                    y + (py * scale) as i32,
                    scale,
                    scale,
                )
            })
            .collect();
        if !rects.is_empty() {
            self.canvas.fill_rects(&rects).map_err(anyhow::Error::msg)?;
        }
        Ok(())
    }
}

/// Reinterpret the XRGB8888 framebuffer as bytes for `Texture::update`.
///
/// Sound because `u32` has no padding and no invalid bit patterns, and the
/// slice is only read. Written by hand rather than pulled in as a dependency:
/// one function is not worth a crate.
fn bytemuck_cast(pixels: &[u32]) -> &[u8] {
    // SAFETY: `u32` is `Copy` with no niches; the resulting slice covers exactly
    // the same allocation, is read-only, and has a smaller alignment requirement.
    unsafe {
        std::slice::from_raw_parts(pixels.as_ptr() as *const u8, std::mem::size_of_val(pixels))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn casting_pixels_to_bytes_preserves_length_and_order() {
        let pixels = vec![0x0011_2233u32, 0x4455_6677];
        let bytes = bytemuck_cast(&pixels);
        assert_eq!(bytes.len(), 8);
        // Little-endian: the low byte comes first.
        assert_eq!(&bytes[..4], &[0x33, 0x22, 0x11, 0x00]);
    }

    #[test]
    fn casting_an_empty_framebuffer_is_empty() {
        assert!(bytemuck_cast(&[]).is_empty());
    }

    #[test]
    fn the_history_strip_fits_the_window() {
        let cell_w = (WINDOW_W / crate::overlay::HISTORY_LEN as u32).max(1);
        assert!(cell_w * crate::overlay::HISTORY_LEN as u32 <= WINDOW_W);
    }

    #[test]
    fn the_overlay_box_fits_the_window() {
        let overlay = Overlay::new();
        let snapshot = MetricsSnapshot::new(rollback_telemetry::SessionInfo::new(
            rollback_core::SimulationKind::Arena,
            "combined",
            "p1",
        ));
        let widest = overlay
            .lines(&snapshot)
            .iter()
            .map(|l| font::text_width(l, OVERLAY_SCALE))
            .max()
            .unwrap();
        assert!(widest + 30 < WINDOW_W, "overlay is {widest}px wide");
    }
}
