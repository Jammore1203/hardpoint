//! The interface's visual language.
//!
//! A tight palette, one typeface at a few sizes, and a small set of panel
//! treatments. Everything in the game is drawn from these constants, which is
//! what stops a twelve-screen interface from drifting apart.

pub type Color = [f32; 4];

// ------------------------------------------------------------------ palette

/// Near-black with a green cast: the ground everything sits on.
pub const BG: Color = [0.043, 0.051, 0.043, 1.0];
pub const PANEL: Color = [0.075, 0.086, 0.078, 0.92];
pub const PANEL_SOLID: Color = [0.075, 0.086, 0.078, 1.0];
pub const PANEL_DEEP: Color = [0.035, 0.041, 0.037, 0.95];
pub const BORDER: Color = [0.32, 0.38, 0.30, 1.0];
pub const BORDER_DIM: Color = [0.17, 0.20, 0.16, 1.0];

/// Phosphor green, used for anything the interface is telling you.
pub const TEXT: Color = [0.80, 0.86, 0.74, 1.0];
pub const TEXT_DIM: Color = [0.46, 0.52, 0.43, 1.0];
pub const TEXT_FAINT: Color = [0.30, 0.34, 0.28, 1.0];
pub const TEXT_BRIGHT: Color = [0.94, 0.98, 0.88, 1.0];

/// Amber, for selection and anything urgent.
pub const ACCENT: Color = [0.95, 0.72, 0.24, 1.0];
pub const ACCENT_DIM: Color = [0.55, 0.42, 0.14, 1.0];
pub const WARN: Color = [0.92, 0.42, 0.20, 1.0];
pub const BAD: Color = [0.86, 0.24, 0.20, 1.0];
pub const GOOD: Color = [0.42, 0.80, 0.38, 1.0];

pub const PHANTOM: Color = [0.90, 0.42, 0.24, 1.0];
pub const VANGUARD: Color = [0.36, 0.62, 0.92, 1.0];
pub const NEUTRAL: Color = [0.72, 0.72, 0.68, 1.0];

pub const SHADOW: Color = [0.0, 0.0, 0.0, 0.75];

// ---------------------------------------------------------------- metrics

/// The interface is authored against a 1080-tall virtual screen and scaled to
/// whatever the window actually is, so it looks identical at any resolution.
pub const DESIGN_HEIGHT: f32 = 1080.0;

/// Type sizes, in design pixels.
pub const H1: f32 = 56.0;
pub const H2: f32 = 34.0;
pub const H3: f32 = 24.0;
pub const BODY: f32 = 19.0;
pub const SMALL: f32 = 15.0;
pub const TINY: f32 = 12.0;

pub const PAD: f32 = 18.0;
pub const ROW: f32 = 34.0;

/// Letter spacing as a fraction of the glyph size; the era's interfaces were
/// generously tracked.
pub const TRACKING: f32 = 0.16;

pub fn team_color(team: crate::game::types::Team) -> Color {
    use crate::game::types::Team;
    match team {
        Team::Phantom => PHANTOM,
        Team::Vanguard => VANGUARD,
        Team::Spectator => TEXT_DIM,
        Team::None => NEUTRAL,
    }
}

pub fn with_alpha(c: Color, a: f32) -> Color { [c[0], c[1], c[2], c[3] * a] }

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Health colour, from green through amber to red.
pub fn health_color(frac: f32) -> Color {
    if frac > 0.6 { mix(ACCENT, GOOD, (frac - 0.6) / 0.4) }
    else if frac > 0.25 { mix(WARN, ACCENT, (frac - 0.25) / 0.35) }
    else { mix(BAD, WARN, frac / 0.25) }
}
