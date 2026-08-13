//! Software renderer for the lock panel: the whole screen is composed into one
//! BGRX pixel buffer (background, buttons, antialiased TTF text) and uploaded
//! with `PutImage`. No X fonts, no server-side text — the panel looks the same
//! on every server, and the single buffer is what both the lock window and the
//! composite overlay receive. Pure code (no X11), so layout + blending are
//! unit-tested.

use crate::lock_ui::{card_rect, Button};
use crate::LockText;
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

/// Panel palette (0xRRGGBB).
pub const BG: u32 = 0x0F0F14; // fallback backdrop when no screen capture
const CARD_FACE: u32 = 0x20202A; // the dialog card
const CARD_BORDER: u32 = 0x3C3C48; // subtle card outline
const TITLE_FG: u32 = 0xF2F2F5; // off-white title
const DETAIL_FG: u32 = 0xB4B4BE; // soft grey detail line
pub const BTN_FACE: u32 = 0xE6E6EA; // light button face
const BTN_LABEL: u32 = 0x16161C; // near-black button label
const ASK_FACE: u32 = 0x5A6ACF; // the "Ask for more time" primary
const ASK_LABEL: u32 = 0xF4F4F8; // light label on the primary

const TITLE_PX: f32 = 32.0;
const DETAIL_PX: f32 = 19.0;
const LABEL_PX: f32 = 18.0;
/// How much of the captured desktop survives the dim (0 = black, 1 = as-is).
const DIM: f32 = 0.30;
const CARD_RADIUS: i32 = 16;
const BTN_RADIUS: i32 = 9;

/// A whole-screen BGRX (little-endian ZPixmap depth-24) buffer.
pub struct Canvas {
    pub w: u16,
    pub h: u16,
    pub buf: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u16, h: u16, color: u32) -> Self {
        let (r, g, b) = rgb(color);
        let mut buf = vec![0u8; w as usize * h as usize * 4];
        for px in buf.chunks_exact_mut(4) {
            px[0] = b;
            px[1] = g;
            px[2] = r;
        }
        Self { w, h, buf }
    }

    /// A canvas seeded from a captured BGRX screen frame, dimmed to `DIM` — the
    /// "semi-transparent over the desktop" backdrop. Falls back to the solid
    /// panel colour if the capture is missing or the wrong size.
    pub fn from_backdrop(w: u16, h: u16, capture: Option<Vec<u8>>) -> Self {
        let expected = w as usize * h as usize * 4;
        match capture {
            Some(mut buf) if buf.len() == expected => {
                for px in buf.chunks_exact_mut(4) {
                    px[0] = (px[0] as f32 * DIM) as u8;
                    px[1] = (px[1] as f32 * DIM) as u8;
                    px[2] = (px[2] as f32 * DIM) as u8;
                }
                Self { w, h, buf }
            }
            _ => Self::new(w, h, BG),
        }
    }

    /// Fill a rounded rectangle (hard-edged corner rounding — crisp at these
    /// radii on a dimmed backdrop).
    pub fn fill_round_rect(&mut self, x: i32, y: i32, rw: u32, rh: u32, r: i32, color: u32) {
        let (red, g, b) = rgb(color);
        let r = r.min(rw as i32 / 2).min(rh as i32 / 2).max(0);
        for yy in y.max(0)..(y + rh as i32).min(self.h as i32) {
            for xx in x.max(0)..(x + rw as i32).min(self.w as i32) {
                // Distance test against the nearest corner-circle centre.
                let cx = (xx - (x + r)).min(0) + (xx - (x + rw as i32 - 1 - r)).max(0);
                let cy = (yy - (y + r)).min(0) + (yy - (y + rh as i32 - 1 - r)).max(0);
                if cx * cx + cy * cy > r * r {
                    continue;
                }
                let o = (yy as usize * self.w as usize + xx as usize) * 4;
                self.buf[o] = b;
                self.buf[o + 1] = g;
                self.buf[o + 2] = red;
            }
        }
    }

    /// Alpha-blend one pixel of `color` at coverage `a` (0..=1).
    fn blend(&mut self, x: i32, y: i32, color: u32, a: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let (r, g, b) = rgb(color);
        let o = (y as usize * self.w as usize + x as usize) * 4;
        let mix = |dst: u8, src: u8| -> u8 {
            (dst as f32 * (1.0 - a) + src as f32 * a)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        self.buf[o] = mix(self.buf[o], b);
        self.buf[o + 1] = mix(self.buf[o + 1], g);
        self.buf[o + 2] = mix(self.buf[o + 2], r);
    }

    /// Draw `text` with its baseline-left at `(x, y)`.
    pub fn draw_text(&mut self, font: &FontRef, px: f32, x: i32, y: i32, color: u32, text: &str) {
        let scaled = font.as_scaled(PxScale::from(px));
        let mut pen = x as f32;
        let mut prev = None;
        for c in text.chars() {
            let id = scaled.glyph_id(c);
            if let Some(p) = prev {
                pen += scaled.kern(p, id);
            }
            let glyph =
                id.with_scale_and_position(PxScale::from(px), ab_glyph::point(pen, y as f32));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, cov| {
                    self.blend(
                        bounds.min.x as i32 + gx as i32,
                        bounds.min.y as i32 + gy as i32,
                        color,
                        cov,
                    );
                });
            }
            pen += scaled.h_advance(id);
            prev = Some(id);
        }
    }
}

fn rgb(color: u32) -> (u8, u8, u8) {
    ((color >> 16) as u8, (color >> 8) as u8, color as u8)
}

/// The advance width of `text` at `px`, for centering.
pub fn text_width(font: &FontRef, px: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            w += scaled.kern(p, id);
        }
        w += scaled.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Compose the full panel: the child's dimmed desktop (when captured) with a
/// centred dialog card — title, detail, and the buttons along the card's
/// bottom (whose rects are the SAME ones `hit_test` uses — what is drawn is
/// what is clickable). The shape mirrors the desktop's own logout dialog.
pub fn render_panel(
    w: u16,
    h: u16,
    text: &LockText,
    buttons: &[Button],
    title_font: &FontRef,
    body_font: &FontRef,
    capture: Option<Vec<u8>>,
) -> Canvas {
    let mut c = Canvas::from_backdrop(w, h, capture);

    // The dialog card (grown per extra line + the ask primary when offered):
    // subtle border, then the face inset by 1px.
    let can_ask = buttons
        .iter()
        .any(|b| b.action == crate::lock_ui::Action::AskMoreTime);
    let card = card_rect(w, h, text.lines.len() as u16, can_ask);
    let (cx, cy, cw, ch) = (
        card.x as i32,
        card.y as i32,
        card.width as u32,
        card.height as u32,
    );
    c.fill_round_rect(cx, cy, cw, ch, CARD_RADIUS, CARD_BORDER);
    c.fill_round_rect(cx + 1, cy + 1, cw - 2, ch - 2, CARD_RADIUS - 1, CARD_FACE);

    // Title + detail, centred in the card above the button row.
    let center = |tw: f32| cx + (((cw as f32) - tw) / 2.0).max(0.0) as i32;
    let tw = text_width(title_font, TITLE_PX, &text.title);
    c.draw_text(
        title_font,
        TITLE_PX,
        center(tw),
        cy + 92,
        TITLE_FG,
        &text.title,
    );
    let dw = text_width(body_font, DETAIL_PX, &text.detail);
    c.draw_text(
        body_font,
        DETAIL_PX,
        center(dw),
        cy + 138,
        DETAIL_FG,
        &text.detail,
    );

    // Schedule/usage lines (the child's "when can I go on next?").
    for (i, line) in text.lines.iter().enumerate() {
        let lw = text_width(body_font, LABEL_PX, line);
        c.draw_text(
            body_font,
            LABEL_PX,
            center(lw),
            cy + 182 + (i as i32) * crate::lock_ui::LINE_STEP as i32,
            DETAIL_FG,
            line,
        );
    }

    for b in buttons {
        let primary = b.action == crate::lock_ui::Action::AskMoreTime;
        let (face, label_fg) = if primary {
            (ASK_FACE, ASK_LABEL)
        } else {
            (BTN_FACE, BTN_LABEL)
        };
        c.fill_round_rect(
            b.rect.x as i32,
            b.rect.y as i32,
            b.rect.width as u32,
            b.rect.height as u32,
            BTN_RADIUS,
            face,
        );
        let lw = text_width(body_font, LABEL_PX, b.label);
        let lx = b.rect.x as i32 + (((b.rect.width as f32) - lw) / 2.0).max(0.0) as i32;
        // Baseline ~2/3 down the face reads visually centred for these sizes.
        let ly = b.rect.y as i32 + (b.rect.height as i32 * 2) / 3;
        c.draw_text(body_font, LABEL_PX, lx, ly, label_fg, b.label);
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock_ui::button_layout;

    fn fonts() -> (FontRef<'static>, FontRef<'static>) {
        (
            FontRef::try_from_slice(crate::TITLE_TTF).expect("vendored bold parses"),
            FontRef::try_from_slice(crate::BODY_TTF).expect("vendored body parses"),
        )
    }

    #[test]
    fn canvas_is_bgrx_and_fill_clips_to_bounds() {
        let mut c = Canvas::new(4, 2, 0x112233);
        assert_eq!(&c.buf[0..4], &[0x33, 0x22, 0x11, 0x00]); // B,G,R,X
        c.fill_round_rect(-5, -5, 100, 100, 0, 0xFF0000); // out-of-range clips, no panic
        assert_eq!(&c.buf[0..4], &[0x00, 0x00, 0xFF, 0x00]);
    }

    #[test]
    fn text_width_grows_with_content() {
        let (_, body) = fonts();
        let short = text_width(&body, 24.0, "Log out");
        let long = text_width(&body, 24.0, "Log out of this session");
        assert!(short > 10.0);
        assert!(long > short);
    }

    #[test]
    fn panel_paints_card_title_and_button_pixels() {
        let (title, body) = fonts();
        let text = LockText {
            title: "Time's up for now".into(),
            detail: "Ask your guardian — access resumes later.".into(),
            lines: vec![],
        };
        let buttons = button_layout(1920, 1200, 0, false);
        let c = render_panel(1920, 1200, &text, &buttons, &title, &body, None);
        // Backdrop (no capture -> solid BG) where nothing is drawn.
        assert_eq!(&c.buf[0..3], &[0x14, 0x0F, 0x0F]);
        // The card face at the card's centre-top area.
        let card = crate::lock_ui::card_rect(1920, 1200, 0, false);
        let o = ((card.y as usize + 20) * 1920 + card.x as usize + card.width as usize / 2) * 4;
        assert_eq!(&c.buf[o..o + 3], &[0x2A, 0x20, 0x20]);
        // A button face pixel (centre of the first button).
        let b = &buttons[0].rect;
        let o =
            ((b.y as usize + b.height as usize / 2) * 1920 + b.x as usize + b.width as usize / 4)
                * 4;
        assert_eq!(&c.buf[o..o + 3], &[0xEA, 0xE6, 0xE6]);
        // The card's corner stays backdrop (rounded off).
        let o = (card.y as usize * 1920 + card.x as usize) * 4;
        assert_eq!(&c.buf[o..o + 3], &[0x14, 0x0F, 0x0F]);
        // The title area contains SOME light pixels (text was rasterized).
        let row = card.y as usize + 80;
        let painted = (card.x as usize..card.x as usize + card.width as usize).any(|x| {
            let o = (row * 1920 + x) * 4;
            c.buf[o] > 0xB0
        });
        assert!(painted, "no title pixels rasterized");
    }

    #[test]
    fn info_lines_grow_the_card_and_render() {
        let (title, body) = fonts();
        let text = LockText {
            title: "Outside allowed hours".into(),
            detail: "You can come back at 07:00 tomorrow.".into(),
            lines: vec![
                "Mon–Fri   07:00 – 20:00".into(),
                "Sat–Sun   08:00 – 21:00".into(),
                "Screen time: up to 2h a day — 1h 05m used today".into(),
            ],
        };
        let n = text.lines.len() as u16;
        let buttons = button_layout(1920, 1200, n, false);
        let card = crate::lock_ui::card_rect(1920, 1200, n, false);
        assert_eq!(
            card.height,
            crate::lock_ui::CARD_H + n * crate::lock_ui::LINE_STEP
        );
        let c = render_panel(1920, 1200, &text, &buttons, &title, &body, None);
        // The lines region contains rasterized (non-face) pixels.
        let row = card.y as usize + 176;
        let painted = (card.x as usize..card.x as usize + card.width as usize).any(|x| {
            let o = (row * 1920 + x) * 4;
            c.buf[o] > 0x60
        });
        assert!(painted, "no info-line pixels rasterized");
    }

    #[test]
    fn backdrop_capture_is_dimmed_and_bad_capture_falls_back() {
        // A pure-white 2x2 capture dims to 30%.
        let cap = vec![0xFFu8; 2 * 2 * 4];
        let c = Canvas::from_backdrop(2, 2, Some(cap));
        assert!(c.buf[0] < 0x60 && c.buf[0] > 0x30, "dimmed: {}", c.buf[0]);
        // Wrong-sized capture -> solid fallback.
        let c = Canvas::from_backdrop(2, 2, Some(vec![0xFF; 3]));
        assert_eq!(&c.buf[0..3], &[0x14, 0x0F, 0x0F]);
    }
}
