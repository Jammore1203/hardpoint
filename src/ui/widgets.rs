//! Interactive widgets.
//!
//! An immediate-mode layer over the painter. Every screen is a function that
//! draws itself and returns what the player did, which means there is no
//! widget tree to keep in sync and adding a screen is adding a function.
//!
//! Both the keyboard and the mouse drive the same focus index, so the entire
//! interface can be operated either way without duplicated code. That also
//! leaves the door open for a controller: it would only need to move focus
//! and press accept.

use super::draw::{Align, Painter};
use super::theme::{self, Color};
use crate::assets::font::FontAtlas;
use crate::render::UiVertex;

/// Sounds the interface asks the game to play.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UiSound {
    Move,
    Select,
    Back,
    Error,
}

/// Navigation input for one frame, however it arrived.
#[derive(Clone, Debug, Default)]
pub struct Nav {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub accept: bool,
    pub back: bool,
    pub page_up: bool,
    pub page_down: bool,
    pub mouse: (f32, f32),
    pub mouse_moved: bool,
    pub click: bool,
    pub wheel: f32,
    /// Characters typed this frame, routed to whichever field has focus.
    pub typed: String,
    pub backspace: bool,
}

pub struct Ui<'a> {
    pub p: Painter<'a>,
    pub nav: Nav,
    /// Which item has keyboard focus.
    pub focus: usize,
    /// Number of focusable items seen this frame.
    pub count: usize,
    /// Set when the caller should play a sound.
    pub sound: Option<UiSound>,
    /// Set by widgets that want the focus moved to them.
    want_focus: Option<usize>,
}

impl<'a> Ui<'a> {
    pub fn new(verts: &'a mut Vec<UiVertex>, font: &'a FontAtlas, width: f32, height: f32, nav: Nav, focus: usize) -> Ui<'a> {
        Ui {
            p: Painter::new(verts, font, width, height),
            nav,
            focus,
            count: 0,
            sound: None,
            want_focus: None,
        }
    }

    /// Applies vertical navigation once every widget has been declared.
    /// Returns the focus index to carry into the next frame.
    pub fn finish(&mut self) -> usize {
        if let Some(f) = self.want_focus {
            self.focus = f;
        }
        if self.count == 0 { return 0; }
        if self.nav.down {
            self.focus = (self.focus + 1) % self.count;
            self.sound = Some(UiSound::Move);
        }
        if self.nav.up {
            self.focus = (self.focus + self.count - 1) % self.count;
            self.sound = Some(UiSound::Move);
        }
        self.focus.min(self.count.saturating_sub(1))
    }

    fn next_index(&mut self) -> usize {
        let i = self.count;
        self.count += 1;
        i
    }

    fn hovered(&self, r: [f32; 4]) -> bool {
        let (mx, my) = self.nav.mouse;
        mx >= r[0] && mx <= r[0] + r[2] && my >= r[1] && my <= r[1] + r[3]
    }

    /// Registers a focusable rectangle. Returns (focused, activated).
    fn focusable(&mut self, r: [f32; 4], enabled: bool) -> (usize, bool, bool) {
        let index = self.next_index();
        if !enabled { return (index, false, false); }
        let hovered = self.hovered(r);
        if hovered && self.nav.mouse_moved && self.focus != index {
            self.want_focus = Some(index);
            self.sound = Some(UiSound::Move);
        }
        let focused = self.focus == index || (hovered && self.want_focus == Some(index));
        let activated = (focused && self.nav.accept) || (hovered && self.nav.click);
        (index, focused, activated)
    }

    // ------------------------------------------------------------ widgets

    /// A full-width menu row.
    pub fn button(&mut self, x: f32, y: f32, w: f32, label: &str, hint: &str, enabled: bool) -> bool {
        let h = theme::ROW + 10.0;
        let (_, focused, activated) = self.focusable([x, y, w, h], enabled);
        self.p.menu_item(x, y, w, label, hint, focused, enabled);
        if activated {
            self.sound = Some(if enabled { UiSound::Select } else { UiSound::Error });
        }
        activated && enabled
    }

    /// A compact button, for toolbars and dialogs.
    pub fn small_button(&mut self, x: f32, y: f32, w: f32, h: f32, label: &str, enabled: bool) -> bool {
        let (_, focused, activated) = self.focusable([x, y, w, h], enabled);
        let bg = if focused { theme::with_alpha(theme::ACCENT, 0.22) } else { theme::PANEL };
        self.p.rect(x, y, w, h, bg);
        self.p.outline(x, y, w, h, 1.0, if focused { theme::ACCENT } else { theme::BORDER_DIM });
        let c = if !enabled { theme::TEXT_FAINT } else if focused { theme::TEXT_BRIGHT } else { theme::TEXT };
        // Step the label down a size rather than let it spill over the border:
        // button widths are derived from the frame and labels vary a lot.
        let mut size = theme::BODY;
        for candidate in [theme::BODY, theme::SMALL, theme::TINY] {
            size = candidate;
            if self.p.measure(label, candidate) <= w - 16.0 { break; }
        }
        self.p.text_aligned(x + w * 0.5, y + (h - size) * 0.5, size, c, label, Align::Center);
        if activated { self.sound = Some(if enabled { UiSound::Select } else { UiSound::Error }); }
        activated && enabled
    }

    /// A row that cycles through named options with left/right.
    pub fn option(&mut self, x: f32, y: f32, w: f32, label: &str, value: &str, enabled: bool) -> i32 {
        let h = theme::ROW + 6.0;
        let (_, focused, activated) = self.focusable([x, y, w, h], enabled);
        if focused {
            self.p.rect(x, y, w, h, theme::with_alpha(theme::ACCENT, 0.14));
            self.p.rect(x, y, 4.0, h, theme::ACCENT);
        }
        let tc = if !enabled { theme::TEXT_FAINT } else if focused { theme::TEXT_BRIGHT } else { theme::TEXT };
        self.p.text(x + 18.0, y + (h - theme::BODY) * 0.5, theme::BODY, tc, label);

        // Value with arrows either side, so it is obvious it can be changed.
        let vx = x + w - 22.0;
        let vw = self.p.measure(value, theme::BODY);
        let vc = if focused { theme::ACCENT } else { theme::TEXT_DIM };
        self.p.text_aligned(vx, y + (h - theme::BODY) * 0.5, theme::BODY, vc, value, Align::Right);
        if focused && enabled {
            self.p.text_aligned(vx - vw - 14.0, y + (h - theme::BODY) * 0.5, theme::BODY, theme::ACCENT, "<", Align::Right);
            self.p.text(vx + 6.0, y + (h - theme::BODY) * 0.5, theme::BODY, theme::ACCENT, ">");
        }

        if !enabled { return 0; }
        let mut delta = 0;
        if focused && self.nav.right { delta = 1; }
        if focused && self.nav.left { delta = -1; }
        if activated { delta = 1; }
        // Clicking the left third steps backwards, which is what a mouse user
        // expects from a control with arrows on it.
        if self.nav.click && self.hovered([x, y, w, h]) && self.nav.mouse.0 < x + w * 0.55 {
            delta = -1;
        }
        if delta != 0 { self.sound = Some(UiSound::Move); }
        delta
    }

    /// A slider. Returns true if the value changed.
    #[allow(clippy::too_many_arguments)]
    pub fn slider(&mut self, x: f32, y: f32, w: f32, label: &str, value: &mut f32, min: f32, max: f32, step: f32, fmt: &str) -> bool {
        let h = theme::ROW + 6.0;
        let (_, focused, _) = self.focusable([x, y, w, h], true);
        if focused {
            self.p.rect(x, y, w, h, theme::with_alpha(theme::ACCENT, 0.14));
            self.p.rect(x, y, 4.0, h, theme::ACCENT);
        }
        let tc = if focused { theme::TEXT_BRIGHT } else { theme::TEXT };
        self.p.text(x + 18.0, y + (h - theme::BODY) * 0.5, theme::BODY, tc, label);

        // The track takes what the label leaves. A fixed fraction runs the
        // track straight through long labels in a narrow column.
        let value_col = 128.0;
        let label_w = self.p.measure(label, theme::BODY);
        let avail = w - 18.0 - label_w - 18.0 - value_col;
        let track_w = avail.clamp(56.0, w * 0.38);
        let track_x = x + w - track_w - value_col;
        let track_y = y + h * 0.5 - 4.0;
        let frac = ((*value - min) / (max - min).max(0.0001)).clamp(0.0, 1.0);
        self.p.rect(track_x, track_y, track_w, 8.0, theme::PANEL_DEEP);
        self.p.rect(track_x, track_y, track_w * frac, 8.0, if focused { theme::ACCENT } else { theme::TEXT_DIM });
        self.p.outline(track_x, track_y, track_w, 8.0, 1.0, theme::BORDER_DIM);
        let knob = track_x + track_w * frac;
        self.p.rect(knob - 3.0, track_y - 5.0, 6.0, 18.0, if focused { theme::TEXT_BRIGHT } else { theme::TEXT_DIM });

        let text = fmt.replace("{}", &format_value(*value, step));
        self.p.text_aligned(x + w - 20.0, y + (h - theme::BODY) * 0.5, theme::BODY,
                            if focused { theme::ACCENT } else { theme::TEXT_DIM }, &text, Align::Right);

        let mut changed = false;
        if focused {
            if self.nav.right { *value = (*value + step).min(max); changed = true; }
            if self.nav.left { *value = (*value - step).max(min); changed = true; }
        }
        // Dragging or clicking anywhere on the track jumps to that value.
        if self.hovered([track_x - 8.0, y, track_w + 16.0, h]) && self.nav.click {
            let t = ((self.nav.mouse.0 - track_x) / track_w).clamp(0.0, 1.0);
            let raw = min + (max - min) * t;
            *value = (raw / step).round() * step;
            changed = true;
        }
        if changed { self.sound = Some(UiSound::Move); }
        changed
    }

    /// An on/off row.
    pub fn toggle(&mut self, x: f32, y: f32, w: f32, label: &str, value: &mut bool) -> bool {
        let d = self.option(x, y, w, label, if *value { "ON" } else { "OFF" }, true);
        if d != 0 { *value = !*value; return true; }
        false
    }

    /// A single-line text field. Returns true if the contents changed.
    pub fn text_field(&mut self, x: f32, y: f32, w: f32, label: &str, value: &mut String, max: usize) -> bool {
        let typed = self.nav.typed.clone();
        let backspace = self.nav.backspace;
        let h = theme::ROW + 6.0;
        let (_, focused, _) = self.focusable([x, y, w, h], true);
        if focused {
            self.p.rect(x, y, w, h, theme::with_alpha(theme::ACCENT, 0.14));
            self.p.rect(x, y, 4.0, h, theme::ACCENT);
        }
        self.p.text(x + 18.0, y + (h - theme::BODY) * 0.5, theme::BODY,
                    if focused { theme::TEXT_BRIGHT } else { theme::TEXT }, label);

        let field_x = x + w * 0.42;
        let field_w = w - (field_x - x) - 18.0;
        self.p.rect(field_x, y + 5.0, field_w, h - 10.0, theme::PANEL_DEEP);
        self.p.outline(field_x, y + 5.0, field_w, h - 10.0, 1.0, if focused { theme::ACCENT } else { theme::BORDER_DIM });

        let mut changed = false;
        if focused {
            for c in typed.chars() {
                if value.chars().count() >= max { break; }
                if c.is_control() { continue; }
                value.push(c);
                changed = true;
            }
            if backspace && !value.is_empty() {
                value.pop();
                changed = true;
            }
        }

        let shown = value.as_str();
        self.p.text(field_x + 10.0, y + (h - theme::BODY) * 0.5, theme::BODY, theme::TEXT_BRIGHT, shown);
        if focused {
            // Blinking caret.
            let caret_x = field_x + 12.0 + self.p.measure(shown, theme::BODY);
            let t = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_millis())
                .unwrap_or(0) / 500) % 2;
            if t == 0 {
                self.p.rect(caret_x, y + 9.0, 2.0, h - 18.0, theme::ACCENT);
            }
        }
        changed
    }

    /// A selectable row in a list. Returns (focused, activated).
    pub fn list_row(&mut self, x: f32, y: f32, w: f32, h: f32, index: usize) -> (bool, bool) {
        let (_, focused, activated) = self.focusable([x, y, w, h], true);
        self.p.row_background(x, y, w, h, index, focused);
        if focused {
            self.p.rect(x, y, 3.0, h, theme::ACCENT);
        }
        if activated { self.sound = Some(UiSound::Select); }
        (focused, activated)
    }

    /// Registers a non-drawing focus stop, used to align list navigation.
    pub fn skip(&mut self) { self.next_index(); }

    pub fn label(&mut self, x: f32, y: f32, size: f32, color: Color, text: &str) {
        self.p.text(x, y, size, color, text);
    }
}

fn format_value(v: f32, step: f32) -> String {
    if step >= 1.0 {
        format!("{}", v.round() as i64)
    } else if step >= 0.1 {
        format!("{:.1}", v)
    } else {
        format!("{:.2}", v)
    }
}
