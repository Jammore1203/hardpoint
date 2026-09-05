//! Immediate-mode drawing onto the interface layer.
//!
//! Everything the interface draws is a textured quad, accumulated into one
//! vertex stream and issued as a single draw call. There is no retained
//! widget tree: screens describe themselves every frame, which keeps the
//! interface code readable and means state lives in one place.

use super::theme::{self, Color};
use crate::assets::font::FontAtlas;
use crate::assets::texgen::Sprite;
use crate::render::UiVertex;

const MODE_SOLID: f32 = 0.0;
const MODE_TEXT: f32 = 1.0;
const MODE_SPRITE: f32 = 2.0;

/// Horizontal alignment for text.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Align {
    Left,
    Center,
    Right,
}

pub struct Painter<'a> {
    verts: &'a mut Vec<UiVertex>,
    font: &'a FontAtlas,
    /// Scale from design pixels to physical pixels.
    pub scale: f32,
    pub width: f32,
    pub height: f32,
    clip: Option<[f32; 4]>,
}

impl<'a> Painter<'a> {
    pub fn new(verts: &'a mut Vec<UiVertex>, font: &'a FontAtlas, width: f32, height: f32) -> Painter<'a> {
        let scale = height / theme::DESIGN_HEIGHT;
        Painter { verts, font, scale, width, height, clip: None }
    }

    /// Width of the virtual screen in design units.
    pub fn design_width(&self) -> f32 { self.width / self.scale }
    pub fn design_height(&self) -> f32 { theme::DESIGN_HEIGHT }
    pub fn center_x(&self) -> f32 { self.design_width() * 0.5 }

    /// Restricts drawing to a rectangle, in design units.
    pub fn push_clip(&mut self, x: f32, y: f32, w: f32, h: f32) -> Option<[f32; 4]> {
        let prev = self.clip;
        self.clip = Some([x, y, x + w, y + h]);
        prev
    }
    pub fn pop_clip(&mut self, prev: Option<[f32; 4]>) { self.clip = prev; }

    #[inline]
    fn push_quad(&mut self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], color: Color, mode: f32) {
        if color[3] <= 0.001 || w <= 0.0 || h <= 0.0 { return; }
        let (mut x0, mut y0, mut x1, mut y1) = (x, y, x + w, y + h);
        let (mut u0, mut v0, mut u1, mut v1) = (uv[0], uv[1], uv[0] + uv[2], uv[1] + uv[3]);

        if let Some(c) = self.clip {
            if x1 <= c[0] || x0 >= c[2] || y1 <= c[1] || y0 >= c[3] { return; }
            // Clip the geometry and the texture coordinates together, so a
            // partially visible glyph is cut rather than squashed.
            let du = (u1 - u0) / (x1 - x0).max(0.0001);
            let dv = (v1 - v0) / (y1 - y0).max(0.0001);
            if x0 < c[0] { u0 += (c[0] - x0) * du; x0 = c[0]; }
            if x1 > c[2] { u1 -= (x1 - c[2]) * du; x1 = c[2]; }
            if y0 < c[1] { v0 += (c[1] - y0) * dv; y0 = c[1]; }
            if y1 > c[3] { v1 -= (y1 - c[3]) * dv; y1 = c[3]; }
        }

        let s = self.scale;
        let (px0, py0, px1, py1) = (x0 * s, y0 * s, x1 * s, y1 * s);
        let v = |px: f32, py: f32, u: f32, vv: f32| UiVertex::new([px, py], [u, vv], color, mode);
        self.verts.push(v(px0, py0, u0, v0));
        self.verts.push(v(px1, py0, u1, v0));
        self.verts.push(v(px1, py1, u1, v1));
        self.verts.push(v(px0, py0, u0, v0));
        self.verts.push(v(px1, py1, u1, v1));
        self.verts.push(v(px0, py1, u0, v1));
    }

    // -------------------------------------------------------------- shapes

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.push_quad(x, y, w, h, [0.0, 0.0, 0.0, 0.0], color, MODE_SOLID);
    }

    pub fn outline(&mut self, x: f32, y: f32, w: f32, h: f32, thickness: f32, color: Color) {
        self.rect(x, y, w, thickness, color);
        self.rect(x, y + h - thickness, w, thickness, color);
        self.rect(x, y + thickness, thickness, h - thickness * 2.0, color);
        self.rect(x + w - thickness, y + thickness, thickness, h - thickness * 2.0, color);
    }

    /// The standard panel: a dark translucent slab with a hairline border and
    /// clipped corners, which is the era's whole interface idiom.
    pub fn panel(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.rect(x, y, w, h, theme::PANEL);
        self.outline(x, y, w, h, 1.0, theme::BORDER_DIM);
        // Corner ticks.
        let t = 7.0;
        self.rect(x, y, t, 2.0, theme::BORDER);
        self.rect(x, y, 2.0, t, theme::BORDER);
        self.rect(x + w - t, y + h - 2.0, t, 2.0, theme::BORDER);
        self.rect(x + w - 2.0, y + h - t, 2.0, t, theme::BORDER);
    }

    pub fn panel_deep(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.rect(x, y, w, h, theme::PANEL_DEEP);
        self.outline(x, y, w, h, 1.0, theme::BORDER_DIM);
    }

    /// A header strip with a title, used at the top of every screen.
    pub fn header(&mut self, x: f32, y: f32, w: f32, title: &str, subtitle: &str) {
        self.rect(x, y, w, 3.0, theme::ACCENT);
        self.text(x, y + 14.0, theme::H2, theme::TEXT_BRIGHT, title);
        if !subtitle.is_empty() {
            self.text(x, y + 14.0 + theme::H2 + 6.0, theme::SMALL, theme::TEXT_DIM, subtitle);
        }
    }

    /// Horizontal progress or resource bar.
    pub fn bar(&mut self, x: f32, y: f32, w: f32, h: f32, frac: f32, fill: Color, back: Color) {
        self.rect(x, y, w, h, back);
        let f = frac.clamp(0.0, 1.0);
        if f > 0.0 { self.rect(x, y, w * f, h, fill); }
        self.outline(x, y, w, h, 1.0, theme::BORDER_DIM);
    }

    /// A segmented bar, which reads faster than a smooth one at a glance.
    pub fn segmented_bar(&mut self, x: f32, y: f32, w: f32, h: f32, segments: u32, filled: u32, fill: Color, back: Color) {
        if segments == 0 { return; }
        let gap = 2.0;
        let seg_w = (w - gap * (segments - 1) as f32) / segments as f32;
        for i in 0..segments {
            let sx = x + i as f32 * (seg_w + gap);
            self.rect(sx, y, seg_w, h, if i < filled { fill } else { back });
        }
    }

    pub fn sprite(&mut self, x: f32, y: f32, w: f32, h: f32, sprite: Sprite, color: Color) {
        self.push_quad(x, y, w, h, sprite.uv(), color, MODE_SPRITE);
    }

    /// Diagonal hatching, used to mark disabled or locked entries.
    pub fn hatch(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let step = 8.0;
        let mut i = 0.0;
        while i < w + h {
            let x0 = (x + i).min(x + w);
            let y0 = y + (i - (x0 - x));
            let len = ((x0 - x).min(h - (y0 - y).max(0.0))).max(0.0);
            if len > 0.0 && y0 < y + h {
                self.rect(x0 - len, y0.max(y), 2.0, len.min(h), color);
            }
            i += step;
        }
    }

    // ---------------------------------------------------------------- text

    /// Draws text and returns the advance width.
    pub fn text(&mut self, x: f32, y: f32, size: f32, color: Color, s: &str) -> f32 {
        self.text_aligned(x, y, size, color, s, Align::Left)
    }

    pub fn text_aligned(&mut self, x: f32, y: f32, size: f32, color: Color, s: &str, align: Align) -> f32 {
        let scale = size / crate::assets::font::GLYPH_H as f32;
        let tracking = size * theme::TRACKING;
        let width = self.measure(s, size);
        let start = match align {
            Align::Left => x,
            Align::Center => x - width * 0.5,
            Align::Right => x - width,
        };
        let mut cx = start;
        for c in s.chars() {
            let i = FontAtlas::index_of(c);
            let adv = self.font.advance[i] as f32 * scale;
            let bearing = self.font.bearing[i] as f32 * scale;
            if !c.is_whitespace() {
                let uv = FontAtlas::uv(i);
                self.push_quad(
                    cx - bearing,
                    y,
                    crate::assets::font::GLYPH_W as f32 * scale,
                    crate::assets::font::GLYPH_H as f32 * scale,
                    uv,
                    color,
                    MODE_TEXT,
                );
            }
            cx += adv + tracking;
        }
        width
    }

    /// Text with a hard drop shadow, which is how it stays readable over the
    /// game world without a panel behind it.
    pub fn text_shadow(&mut self, x: f32, y: f32, size: f32, color: Color, s: &str) -> f32 {
        self.text_shadow_aligned(x, y, size, color, s, Align::Left)
    }

    pub fn text_shadow_aligned(&mut self, x: f32, y: f32, size: f32, color: Color, s: &str, align: Align) -> f32 {
        let off = (size * 0.08).max(1.0);
        self.text_aligned(x + off, y + off, size, theme::SHADOW, s, align);
        self.text_aligned(x, y, size, color, s, align)
    }

    pub fn measure(&self, s: &str, size: f32) -> f32 {
        let scale = size / crate::assets::font::GLYPH_H as f32;
        let tracking = size * theme::TRACKING;
        self.font.measure(s, tracking / scale.max(0.0001)) * scale
    }

    /// Wraps text to a width, returning the number of lines drawn.
    /// Draws a single line, cutting it short with an ellipsis if it will not
    /// fit. Names come from the network, so nothing may assume a length.
    pub fn text_clipped(&mut self, x: f32, y: f32, width: f32, size: f32, color: Color, s: &str) {
        if self.measure(s, size) <= width {
            self.text(x, y, size, color, s);
            return;
        }
        let mut cut = s.len();
        while cut > 1 {
            cut -= 1;
            if !s.is_char_boundary(cut) { continue; }
            let candidate = format!("{}.", &s[..cut]);
            if self.measure(&candidate, size) <= width {
                self.text(x, y, size, color, &candidate);
                return;
            }
        }
    }

    pub fn text_wrapped(&mut self, x: f32, y: f32, width: f32, size: f32, color: Color, s: &str) -> u32 {
        let mut line = String::new();
        let mut cy = y;
        let mut lines = 0;
        let line_h = size * 1.42;
        for word in s.split_whitespace() {
            let candidate = if line.is_empty() { word.to_string() } else { format!("{} {}", line, word) };
            if self.measure(&candidate, size) > width && !line.is_empty() {
                self.text(x, cy, size, color, &line);
                cy += line_h;
                lines += 1;
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        if !line.is_empty() {
            self.text(x, cy, size, color, &line);
            lines += 1;
        }
        lines
    }

    // ------------------------------------------------------------ widgets

    /// A menu row. Returns its bounds so the caller can hit-test it.
    pub fn menu_item(&mut self, x: f32, y: f32, w: f32, label: &str, hint: &str, selected: bool, enabled: bool) -> [f32; 4] {
        let h = theme::ROW + 10.0;
        if selected {
            self.rect(x, y, w, h, theme::with_alpha(theme::ACCENT, 0.16));
            self.rect(x, y, 4.0, h, theme::ACCENT);
        }
        let color = if !enabled {
            theme::TEXT_FAINT
        } else if selected {
            theme::TEXT_BRIGHT
        } else {
            theme::TEXT
        };
        self.text(x + 18.0, y + (h - theme::H3) * 0.5, theme::H3, color, label);
        if !hint.is_empty() {
            // Only draw the hint if it will not collide with the label; a
            // long label always wins.
            let label_end = 18.0 + self.measure(label, theme::H3);
            let hint_w = self.measure(hint, theme::SMALL);
            if label_end + hint_w + 40.0 < w {
                self.text_aligned(
                    x + w - 16.0,
                    y + (h - theme::SMALL) * 0.5,
                    theme::SMALL,
                    if selected { theme::ACCENT } else { theme::TEXT_DIM },
                    hint,
                    Align::Right,
                );
            }
        }
        if !enabled {
            self.hatch(x, y, w, h, theme::with_alpha(theme::TEXT_FAINT, 0.30));
        }
        [x, y, w, h]
    }

    /// A tabular row, for scoreboards and server lists.
    pub fn row_background(&mut self, x: f32, y: f32, w: f32, h: f32, index: usize, selected: bool) {
        if selected {
            self.rect(x, y, w, h, theme::with_alpha(theme::ACCENT, 0.20));
        } else if index % 2 == 0 {
            self.rect(x, y, w, h, [1.0, 1.0, 1.0, 0.035]);
        }
    }

    /// Faint horizontal rule.
    pub fn rule(&mut self, x: f32, y: f32, w: f32) {
        self.rect(x, y, w, 1.0, theme::BORDER_DIM);
    }

    /// Full-screen tint, used for pause and death overlays.
    pub fn dim(&mut self, alpha: f32) {
        let (w, h) = (self.design_width(), self.design_height());
        self.rect(0.0, 0.0, w, h, [0.0, 0.0, 0.0, alpha]);
    }

    /// Static noise band, used behind headings for texture.
    pub fn noise_band(&mut self, x: f32, y: f32, w: f32, h: f32, seed: u32, color: Color) {
        let mut rng = crate::core::Rng::seeded(seed);
        let count = (w / 6.0) as u32;
        for _ in 0..count {
            let bx = x + rng.range(0.0, w);
            let bh = rng.range(1.0, h);
            self.rect(bx, y + (h - bh) * 0.5, 2.0, bh, theme::with_alpha(color, rng.range(0.05, 0.25)));
        }
    }

    pub fn vertex_count(&self) -> usize { self.verts.len() }
}
