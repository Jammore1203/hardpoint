//! The heads-up display.
//!
//! Minimal by design. Health and ammunition where the era put them, a
//! crosshair that tells you your actual spread, a kill feed, and objective
//! markers. Everything else appears only when it matters and gets out of the
//! way again.

use crate::assets::texgen::Sprite;
use crate::game::events::{AnnounceLine, DeathCause};
use crate::game::types::{HitZone, Team};
use crate::game::weapons::WeaponId;
use crate::modes::{ModeId, Phase};
use crate::net::client::Client;
use crate::ui::draw::{Align, Painter};
use crate::ui::theme::{self, Color};
use glam::{Mat4, Vec3, Vec4};

const KILLFEED_LIFE: f64 = 6.0;
const KILLFEED_MAX: usize = 6;
const HITMARKER_LIFE: f32 = 0.55;
const DAMAGE_INDICATOR_LIFE: f32 = 1.4;

#[derive(Clone)]
pub struct KillEntry {
    pub killer: String,
    pub victim: String,
    pub killer_team: Team,
    pub victim_team: Team,
    pub weapon: WeaponId,
    pub cause: DeathCause,
    pub time: f64,
    pub involves_me: bool,
}

#[derive(Clone, Copy)]
struct HitMarker {
    life: f32,
    zone: HitZone,
    lethal: bool,
}

#[derive(Clone, Copy)]
struct DamageIndicator {
    /// Direction the damage came from, in world space.
    dir: Vec3,
    life: f32,
    strength: f32,
}

#[derive(Clone, Copy)]
struct FloatingNumber {
    world: Vec3,
    value: u16,
    life: f32,
    lethal: bool,
}

/// Transient HUD state that has to persist between frames.
pub struct Hud {
    pub killfeed: Vec<KillEntry>,
    markers: Vec<HitMarker>,
    damage: Vec<DamageIndicator>,
    numbers: Vec<FloatingNumber>,
    pub announce: Option<(AnnounceLine, f64)>,
    pub pickup_text: Option<(String, f64)>,
    /// Ammo counter flash when the magazine gets low.
    pub low_ammo_pulse: f32,
    pub scoreboard_open: bool,
    pub chat_open: bool,
    pub chat_team: bool,
    pub chat_buffer: String,
    pub crosshair_bloom: f32,
    /// Set briefly when the player is hit, to shove the crosshair.
    pub flinch: (f32, f32),
}

impl Default for Hud {
    fn default() -> Self { Hud::new() }
}

impl Hud {
    pub fn new() -> Hud {
        Hud {
            killfeed: Vec::with_capacity(KILLFEED_MAX),
            markers: Vec::new(),
            damage: Vec::new(),
            numbers: Vec::new(),
            announce: None,
            pickup_text: None,
            low_ammo_pulse: 0.0,
            scoreboard_open: false,
            chat_open: false,
            chat_team: false,
            chat_buffer: String::new(),
            crosshair_bloom: 0.0,
            flinch: (0.0, 0.0),
        }
    }

    pub fn reset(&mut self) {
        self.killfeed.clear();
        self.markers.clear();
        self.damage.clear();
        self.numbers.clear();
        self.announce = None;
        self.pickup_text = None;
        self.chat_open = false;
        self.chat_buffer.clear();
    }

    pub fn push_kill(&mut self, e: KillEntry) {
        self.killfeed.push(e);
        while self.killfeed.len() > KILLFEED_MAX { self.killfeed.remove(0); }
    }

    pub fn hit_marker(&mut self, zone: HitZone, lethal: bool) {
        self.markers.push(HitMarker { life: 0.0, zone, lethal });
        if self.markers.len() > 8 { self.markers.remove(0); }
    }

    pub fn damage_from(&mut self, dir: Vec3, strength: f32) {
        self.damage.push(DamageIndicator { dir: dir.normalize_or_zero(), life: 0.0, strength });
        if self.damage.len() > 6 { self.damage.remove(0); }
        // A hit shoves the view a little, which is what tells you where it
        // came from before you have read the indicator.
        self.flinch.0 += (dir.x * 4.0).clamp(-3.0, 3.0);
        self.flinch.1 += (strength * 2.5).clamp(0.0, 4.0);
    }

    pub fn damage_number(&mut self, world: Vec3, value: u16, lethal: bool) {
        self.numbers.push(FloatingNumber { world, value, life: 0.0, lethal });
        if self.numbers.len() > 24 { self.numbers.remove(0); }
    }

    pub fn update(&mut self, dt: f32, now: f64) {
        self.markers.retain_mut(|m| { m.life += dt; m.life < HITMARKER_LIFE });
        self.damage.retain_mut(|d| { d.life += dt; d.life < DAMAGE_INDICATOR_LIFE });
        self.numbers.retain_mut(|n| { n.life += dt; n.life < 1.1 });
        self.killfeed.retain(|k| now - k.time < KILLFEED_LIFE);
        if let Some((_, t)) = self.announce {
            if now - t > 3.2 { self.announce = None; }
        }
        if let Some((_, t)) = self.pickup_text {
            if now - t > 2.2 { self.pickup_text = None; }
        }
        self.flinch.0 *= (-9.0 * dt).exp();
        self.flinch.1 *= (-9.0 * dt).exp();
        self.low_ammo_pulse += dt * 6.0;
    }
}

/// Everything the HUD needs to draw itself, gathered once per frame.
pub struct HudFrame<'a> {
    pub client: &'a Client,
    pub hud: &'a Hud,
    pub now: f64,
    pub view_proj: Mat4,
    /// Camera yaw. The damage compass needs a horizontal basis; deriving one
    /// from the view matrix drags pitch into it and swings the arc across the
    /// screen whenever the player looks up or down.
    pub cam_yaw: f32,
    pub spread: f32,
    pub crosshair_style: u8,
    pub show_damage_numbers: bool,
    pub scale: f32,
    /// Objective positions and labels the map wants shown.
    pub objectives: &'a [(Vec3, &'a str, Team, bool)],
    pub alive: bool,
    /// How far into aiming down the sights the player is, 0..1.
    pub ads: f32,
    /// Team-mates currently being heard, and whether we are transmitting.
    pub voices: &'a [(String, f32)],
    pub transmitting: bool,
    pub respawn_in: f32,
    pub health: f32,
    pub armor: f32,
    pub ammo: u16,
    pub reserve: u16,
    pub weapon: WeaponId,
    pub lethal: u8,
    pub tactical: u8,
    pub lethal_name: &'a str,
    pub tactical_name: &'a str,
    pub reloading: bool,
    pub bomb_progress: f32,
    pub bomb_label: &'a str,
}

pub fn draw(p: &mut Painter, f: &HudFrame) {
    let w = p.design_width();
    let h = p.design_height();
    let s = f.scale;

    draw_top_bar(p, f, w);
    if f.alive {
        // Aiming replaces the crosshair with the weapon's own sights. A
        // crosshair painted over an aligned rear aperture is the clearest
        // possible statement that the sights are decoration.
        if f.ads < 0.55 { draw_crosshair(p, f, w, h); }
        draw_hit_markers(p, f, w, h);
    }
    draw_bottom_left(p, f, h, s);
    draw_bottom_right(p, f, w, h, s);
    draw_killfeed(p, f, w);
    draw_voice(p, f, w, h, s);
    draw_objectives(p, f, w, h);
    draw_damage_indicators(p, f, w, h);
    if f.show_damage_numbers { draw_damage_numbers(p, f); }
    draw_announce(p, f, w, h);
    draw_bomb_progress(p, f, w, h);

    if !f.alive {
        draw_death_overlay(p, f, w, h);
    }
    draw_chat(p, f, w, h);
}

fn draw_top_bar(p: &mut Painter, f: &HudFrame, w: f32) {
    let mi = &f.client.match_info;
    let phase = Phase::from_u8(mi.phase);
    let cx = w * 0.5;

    // Clock.
    let seconds = mi.seconds_left;
    let clock = format!("{}:{:02}", seconds / 60, seconds % 60);
    let urgent = seconds <= 30 && phase == Phase::Live;
    let clock_color = if urgent { theme::WARN } else { theme::TEXT_BRIGHT };

    p.rect(cx - 130.0, 0.0, 260.0, 54.0, theme::with_alpha(theme::PANEL_DEEP, 0.85));
    p.rect(cx - 130.0, 52.0, 260.0, 2.0, theme::BORDER_DIM);
    p.text_shadow_aligned(cx, 8.0, theme::H3, clock_color, &clock, Align::Center);
    p.text_aligned(cx, 34.0, theme::TINY, theme::TEXT_DIM, mi.mode.short(), Align::Center);

    // Team or personal score either side of the clock.
    if mi.mode.is_team_game() {
        let ph = mi.team_scores[0];
        let vg = mi.team_scores[1];
        let mine = f.client.my_team();
        let (left, right, lc, rc) = if mine == Team::Vanguard {
            (vg, ph, theme::VANGUARD, theme::PHANTOM)
        } else {
            (ph, vg, theme::PHANTOM, theme::VANGUARD)
        };
        p.text_shadow_aligned(cx - 148.0, 8.0, theme::H3, lc, &left.to_string(), Align::Right);
        p.text_shadow(cx + 148.0, 8.0, theme::H3, rc, &right.to_string());
    } else {
        let me = f.client.player_info(f.client.slot);
        let kills = me.map(|m| m.kills).unwrap_or(0);
        p.text_shadow_aligned(cx - 148.0, 8.0, theme::H3, theme::TEXT, &kills.to_string(), Align::Right);
        p.text_shadow(cx + 148.0, 8.0, theme::H3, theme::TEXT_DIM, &mi.score_limit.to_string());
    }

    if phase != Phase::Live {
        p.text_shadow_aligned(cx, 62.0, theme::BODY, theme::ACCENT, phase.label(), Align::Center);
    }
}

fn draw_crosshair(p: &mut Painter, f: &HudFrame, w: f32, h: f32) {
    let cx = w * 0.5 + f.hud.flinch.0;
    let cy = h * 0.5 - f.hud.flinch.1;

    // The gap tracks the actual weapon spread, so the crosshair is telling
    // the player something true rather than decorating the screen.
    let spread_px = (f.spread * h * 0.9).clamp(2.0, 90.0);
    let gap = 5.0 + spread_px;
    let len = 9.0;
    let thick = 2.0;
    let c = theme::with_alpha(theme::TEXT_BRIGHT, 0.9);

    match f.crosshair_style {
        1 => {
            // Dot only.
            p.rect(cx - 2.0, cy - 2.0, 4.0, 4.0, c);
        }
        2 => {
            // Circle of four ticks plus a dot.
            p.rect(cx - 1.5, cy - 1.5, 3.0, 3.0, c);
            for (dx, dy) in [(0.0, -1.0), (0.0, 1.0), (-1.0, 0.0), (1.0, 0.0)] {
                p.rect(cx + dx * gap - thick * 0.5, cy + dy * gap - thick * 0.5, thick, thick, c);
            }
        }
        3 => {
            // Chevron.
            for i in 0..8 {
                let t = i as f32;
                p.rect(cx - gap - t, cy + gap + t * 0.6, 2.0, 2.0, c);
                p.rect(cx + gap + t, cy + gap + t * 0.6, 2.0, 2.0, c);
            }
        }
        _ => {
            // The default: four lines that open with the spread.
            p.rect(cx - gap - len, cy - thick * 0.5, len, thick, c);
            p.rect(cx + gap, cy - thick * 0.5, len, thick, c);
            p.rect(cx - thick * 0.5, cy - gap - len, thick, len, c);
            p.rect(cx - thick * 0.5, cy + gap, thick, len, c);
        }
    }

    if f.reloading {
        p.text_aligned(cx, cy + 34.0, theme::SMALL, theme::ACCENT, "RELOADING", Align::Center);
    }
}

fn draw_hit_markers(p: &mut Painter, f: &HudFrame, w: f32, h: f32) {
    let cx = w * 0.5;
    let cy = h * 0.5;
    for m in &f.hud.markers {
        // Hold at full strength for the first third, then fade. A marker that
        // starts fading immediately is one you never quite see.
        let age = (m.life / HITMARKER_LIFE).clamp(0.0, 1.0);
        let t = if age < 0.35 { 1.0 } else { 1.0 - (age - 0.35) / 0.65 };
        // A small outward punch on arrival sells the hit.
        let spread = 11.0 + age * 7.0;
        let (color, thick, len) = if m.lethal {
            (theme::with_alpha(theme::BAD, t), 4.0, 16.0)
        } else if m.zone == HitZone::Head {
            (theme::with_alpha(theme::ACCENT, t), 4.0, 14.0)
        } else {
            (theme::with_alpha(theme::TEXT_BRIGHT, t), 3.0, 12.0)
        };
        let s = f.scale;
        for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            // Four diagonal ticks: instantly readable, and the shape everyone
            // who played these games recognises. Drawn as one run of squares
            // along the diagonal so it reads as a solid stroke.
            let steps = (len / 2.0) as i32;
            for i in 0..steps {
                let o = (spread + i as f32 * 2.0) * s;
                p.rect(cx + dx * o - thick * 0.5, cy + dy * o - thick * 0.5,
                       thick, thick, color);
            }
        }
        // A kill also gets a ring, so a finished target is unmistakable.
        if m.lethal {
            let r = 26.0 * s;
            for i in 0..24 {
                let a = i as f32 / 24.0 * std::f32::consts::TAU;
                p.rect(cx + a.cos() * r - 2.0, cy + a.sin() * r - 2.0, 4.0, 4.0, color);
            }
        }
    }
}

fn draw_bottom_left(p: &mut Painter, f: &HudFrame, h: f32, s: f32) {
    let x = 34.0;
    let y = h - 118.0 * s;
    let bar_w = 260.0 * s;

    // Health.
    let frac = (f.health / 100.0).clamp(0.0, 1.0);
    let hc = theme::health_color(frac);
    p.text_shadow(x, y - 24.0, theme::SMALL, theme::TEXT_DIM, "VITALS");
    p.bar(x, y, bar_w, 16.0 * s, frac, hc, theme::PANEL_DEEP);
    p.text_shadow(x + bar_w + 12.0, y - 2.0, theme::H3, hc, &format!("{}", f.health.ceil() as i32));

    // Armour, only when there is any.
    if f.armor > 0.5 {
        let af = (f.armor / 75.0).clamp(0.0, 1.0);
        p.bar(x, y + 22.0 * s, bar_w * 0.72, 8.0 * s, af, theme::VANGUARD, theme::PANEL_DEEP);
    }

    // Equipment.
    let ey = y + 42.0 * s;
    let lethal_c = if f.lethal > 0 { theme::TEXT } else { theme::TEXT_FAINT };
    let tac_c = if f.tactical > 0 { theme::TEXT } else { theme::TEXT_FAINT };
    p.text_shadow(x, ey, theme::SMALL, lethal_c, &format!("{} x{}", f.lethal_name, f.lethal));
    p.text_shadow(x + 150.0 * s, ey, theme::SMALL, tac_c, &format!("{} x{}", f.tactical_name, f.tactical));
}

fn draw_bottom_right(p: &mut Painter, f: &HudFrame, w: f32, h: f32, s: f32) {
    let right = w - 34.0;
    let y = h - 118.0 * s;
    let def = f.weapon.def();

    p.text_shadow_aligned(right, y - 26.0, theme::SMALL, theme::TEXT_DIM, def.name, Align::Right);

    // The ammunition counter is the single most-read element on the screen,
    // so it is the largest thing in the corner.
    let low = f.ammo * 4 <= def.mag && def.mag > 0;
    let pulse = if low { 0.72 + 0.28 * (f.hud.low_ammo_pulse.sin() * 0.5 + 0.5) } else { 1.0 };
    let ammo_color = if f.ammo == 0 { theme::BAD } else if low { theme::WARN } else { theme::TEXT_BRIGHT };
    let ammo_color = theme::with_alpha(ammo_color, pulse);

    let ammo_text = if def.is_melee() { "-".to_string() } else { format!("{}", f.ammo) };
    let reserve_text = if def.is_melee() { String::new() } else { format!("/ {}", f.reserve) };
    let aw = p.measure(&reserve_text, theme::BODY);
    p.text_shadow_aligned(right - aw - 8.0, y - 4.0, theme::H1, ammo_color, &ammo_text, Align::Right);
    p.text_shadow_aligned(right, y + 24.0, theme::BODY, theme::TEXT_DIM, &reserve_text, Align::Right);

    // Magazine as discrete segments, which reads faster than a number when
    // you are actually looking down the sights.
    if !def.is_melee() && def.mag <= 40 {
        let segs = def.mag as u32;
        let filled = f.ammo as u32;
        let seg_w = (200.0 * s).min(seg_w_for(segs));
        p.segmented_bar(right - seg_w, y + 46.0 * s, seg_w, 6.0 * s, segs, filled, ammo_color, theme::PANEL_DEEP);
    }

    p.text_shadow_aligned(right, y + 58.0 * s, theme::TINY, theme::TEXT_FAINT, def.fire_mode.label(), Align::Right);
}

fn seg_w_for(segs: u32) -> f32 { (segs as f32 * 6.0).min(240.0) }

fn draw_killfeed(p: &mut Painter, f: &HudFrame, w: f32) {
    let x = w - 30.0;
    let mut y = 76.0;
    for k in f.killfeed_iter() {
        let age = (f.now - k.time) as f32;
        let alpha = if age > (KILLFEED_LIFE as f32 - 0.6) {
            ((KILLFEED_LIFE as f32 - age) / 0.6).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let verb = match k.cause {
            DeathCause::Headshot => "**",
            DeathCause::Melee => "><",
            DeathCause::Explosion => "*",
            DeathCause::Fire => "~",
            DeathCause::Fall => "v",
            DeathCause::World => "x",
            DeathCause::Bomb => "#",
            DeathCause::Bullet => "-",
        };
        let victim_c = theme::with_alpha(theme::team_color(k.victim_team), alpha);
        let killer_c = theme::with_alpha(theme::team_color(k.killer_team), alpha);
        let mid_c = theme::with_alpha(if k.involves_me { theme::ACCENT } else { theme::TEXT_DIM }, alpha);

        let vw = p.measure(&k.victim, theme::SMALL);
        let mw = p.measure(verb, theme::SMALL) + 22.0;
        p.text_shadow_aligned(x, y, theme::SMALL, victim_c, &k.victim, Align::Right);
        p.text_shadow_aligned(x - vw - 10.0, y, theme::SMALL, mid_c, verb, Align::Right);
        p.text_shadow_aligned(x - vw - mw, y, theme::SMALL, killer_c, &k.killer, Align::Right);
        y += 22.0;
    }
}

fn draw_objectives(p: &mut Painter, f: &HudFrame, w: f32, h: f32) {
    for (pos, label, team, contested) in f.objectives.iter() {
        let clip = f.view_proj * Vec4::new(pos.x, pos.y + 1.6, pos.z, 1.0);
        if clip.w <= 0.05 { continue; }
        let ndc = clip.truncate() / clip.w;
        // Off-screen markers are pinned to the edge, so you always know which
        // way an objective is.
        let mut sx = (ndc.x * 0.5 + 0.5) * w;
        let mut sy = (0.5 - ndc.y * 0.5) * h;
        let mut offscreen = false;
        if ndc.x < -1.0 || ndc.x > 1.0 || ndc.y < -1.0 || ndc.y > 1.0 {
            offscreen = true;
            sx = sx.clamp(48.0, w - 48.0);
            sy = sy.clamp(90.0, h - 150.0);
        }
        let color = if *contested { theme::ACCENT } else { theme::team_color(*team) };
        let size = if offscreen { 14.0 } else { 20.0 };
        p.sprite(sx - size * 0.5, sy - size * 0.5, size, size, Sprite::Ring, theme::with_alpha(color, 0.9));
        p.text_shadow_aligned(sx, sy - size * 0.5 - 18.0, theme::BODY, color, label, Align::Center);
    }
}

fn draw_damage_indicators(p: &mut Painter, f: &HudFrame, w: f32, h: f32) {
    if f.hud.damage.is_empty() { return; }
    let cx = w * 0.5;
    let cy = h * 0.5;
    // A compass, so it only ever answers "which way do I turn". Using the full
    // camera forward instead put the arc somewhere else entirely as soon as
    // the player looked up or down, which is most of a firefight.
    let (forward, right) = crate::math::move_basis(f.cam_yaw);

    for d in &f.hud.damage {
        let t = 1.0 - (d.life / DAMAGE_INDICATOR_LIFE).clamp(0.0, 1.0);
        let flat = Vec3::new(d.dir.x, 0.0, d.dir.z).normalize_or_zero();
        if flat.length_squared() < 1e-6 { continue; }
        let angle = flat.dot(right).atan2(flat.dot(forward));

        let radius = 170.0 * f.scale;
        let fade = t * (0.45 + d.strength * 0.55);
        // A thick tapered arc: widest at the bearing, fading off to the sides.
        for i in -7i32..=7 {
            let a = angle + i as f32 * 0.055;
            let taper = 1.0 - (i.abs() as f32 / 8.0);
            let thick = 4.0 + taper * 6.0;
            let bx = cx + a.sin() * radius;
            let by = cy - a.cos() * radius;
            p.rect(bx - thick * 0.5, by - thick * 0.5, thick, thick,
                   theme::with_alpha(theme::BAD, fade * (0.35 + taper * 0.65)));
        }
    }
}

fn draw_damage_numbers(p: &mut Painter, f: &HudFrame) {
    for n in &f.hud.numbers {
        let clip = f.view_proj * Vec4::new(n.world.x, n.world.y + n.life * 0.9, n.world.z, 1.0);
        if clip.w <= 0.05 { continue; }
        let ndc = clip.truncate() / clip.w;
        if ndc.x.abs() > 1.0 || ndc.y.abs() > 1.0 { continue; }
        let sx = (ndc.x * 0.5 + 0.5) * p.design_width();
        let sy = (0.5 - ndc.y * 0.5) * p.design_height();
        let t = 1.0 - (n.life / 1.1).clamp(0.0, 1.0);
        let color = if n.lethal { theme::with_alpha(theme::BAD, t) } else { theme::with_alpha(theme::TEXT_BRIGHT, t) };
        p.text_shadow_aligned(sx, sy, theme::BODY, color, &n.value.to_string(), Align::Center);
    }
}

fn draw_announce(p: &mut Painter, f: &HudFrame, w: f32, h: f32) {
    if let Some((line, t)) = f.hud.announce {
        let age = (f.now - t) as f32;
        let alpha = if age < 0.2 { age / 0.2 } else if age > 2.6 { ((3.2 - age) / 0.6).clamp(0.0, 1.0) } else { 1.0 };
        let text = line.text();
        let cx = w * 0.5;
        let y = h * 0.24;
        let tw = p.measure(text, theme::H3) + 40.0;
        p.rect(cx - tw * 0.5, y - 8.0, tw, 38.0, theme::with_alpha(theme::PANEL_DEEP, alpha * 0.8));
        p.rect(cx - tw * 0.5, y - 8.0, 3.0, 38.0, theme::with_alpha(theme::ACCENT, alpha));
        p.text_shadow_aligned(cx, y, theme::H3, theme::with_alpha(theme::TEXT_BRIGHT, alpha), text, Align::Center);
    }
    if let Some((text, t)) = &f.hud.pickup_text {
        let age = (f.now - t) as f32;
        let alpha = ((2.2 - age) / 0.5).clamp(0.0, 1.0);
        p.text_shadow_aligned(w * 0.5, h * 0.62, theme::BODY, theme::with_alpha(theme::ACCENT, alpha), text, Align::Center);
    }
}

fn draw_bomb_progress(p: &mut Painter, f: &HudFrame, w: f32, h: f32) {
    if f.bomb_progress <= 0.001 { return; }
    let cx = w * 0.5;
    let y = h * 0.66;
    let bw = 320.0;
    p.text_shadow_aligned(cx, y - 26.0, theme::BODY, theme::ACCENT, f.bomb_label, Align::Center);
    p.bar(cx - bw * 0.5, y, bw, 14.0, f.bomb_progress, theme::ACCENT, theme::PANEL_DEEP);
}

fn draw_death_overlay(p: &mut Painter, f: &HudFrame, w: f32, h: f32) {
    p.rect(0.0, 0.0, w, h, [0.35, 0.02, 0.02, 0.18]);
    let cy = h * 0.42;
    p.text_shadow_aligned(w * 0.5, cy, theme::H2, theme::BAD, "YOU WERE KILLED", Align::Center);

    if f.respawn_in > 0.05 {
        p.text_shadow_aligned(
            w * 0.5, cy + 48.0, theme::H3, theme::TEXT,
            &format!("RESPAWNING IN {:.0}", f.respawn_in.ceil()), Align::Center,
        );
    } else {
        let pulse = 0.6 + 0.4 * ((f.now * 4.0).sin() as f32 * 0.5 + 0.5);
        p.text_shadow_aligned(
            w * 0.5, cy + 48.0, theme::H3, theme::with_alpha(theme::ACCENT, pulse),
            "PRESS SPACE TO DEPLOY", Align::Center,
        );
    }
}

fn draw_chat(p: &mut Painter, f: &HudFrame, w: f32, h: f32) {
    let x = 34.0;
    let mut y = h * 0.52;
    for line in f.client.chat.iter().rev().take(6) {
        let age = (f.now - line.time) as f32;
        if age > 12.0 && !f.hud.chat_open { continue; }
        let alpha = if f.hud.chat_open { 1.0 } else { ((12.0 - age) / 2.0).clamp(0.0, 1.0) };
        let name = f.client.name_of(line.slot);
        let team = f.client.player_info(line.slot).map(|p| p.team).unwrap_or(Team::None);
        let prefix = if line.team_only { "[TEAM] " } else { "" };
        let nc = theme::with_alpha(theme::team_color(team), alpha);
        let tc = theme::with_alpha(theme::TEXT, alpha);
        let head = format!("{}{}: ", prefix, name);
        let hw = p.measure(&head, theme::SMALL);
        p.text_shadow(x, y, theme::SMALL, nc, &head);
        p.text_shadow(x + hw, y, theme::SMALL, tc, &line.text);
        y -= 20.0;
    }

    if f.hud.chat_open {
        let prompt = if f.hud.chat_team { "TEAM SAY:" } else { "SAY:" };
        let py = h * 0.56;
        p.rect(x - 6.0, py - 4.0, w * 0.55, 28.0, theme::with_alpha(theme::PANEL_DEEP, 0.9));
        p.text(x, py, theme::BODY, theme::ACCENT, prompt);
        let pw = p.measure(prompt, theme::BODY) + 10.0;
        p.text(x + pw, py, theme::BODY, theme::TEXT_BRIGHT, &f.hud.chat_buffer);
        let cw = p.measure(&f.hud.chat_buffer, theme::BODY);
        p.rect(x + pw + cw + 2.0, py + 2.0, 2.0, theme::BODY, theme::ACCENT);
    }
}

impl<'a> HudFrame<'a> {
    fn killfeed_iter(&self) -> impl Iterator<Item = &KillEntry> {
        self.hud.killfeed.iter()
    }
}

/// Draws the scoreboard over the top of everything.
/// `top` and `bottom` are the margins the caller wants left clear: the results
/// screen puts a title above the table and a summary below it, the in-game
/// overlay uses neither.
pub fn draw_scoreboard(p: &mut Painter, client: &Client, mode: ModeId, now: f64, top: f32, bottom: f32) {
    let w = p.design_width();
    let h = p.design_height();
    p.dim(0.72);

    let panel_w = (w * 0.78).min(1180.0);
    let x = (w - panel_w) * 0.5;
    let y = top;
    let panel_h = (h - top - bottom).max(240.0);
    p.panel(x, y, panel_w, panel_h);

    let mi = &client.match_info;
    p.header(x + theme::PAD, y + theme::PAD, panel_w - theme::PAD * 2.0,
             mode.name(), &format!("{}   {}", mi.map.name(), mi.map.theme()));

    if mode.is_team_game() {
        p.text_aligned(x + panel_w - theme::PAD, y + 24.0, theme::H2, theme::PHANTOM,
                       &mi.team_scores[0].to_string(), Align::Right);
        p.text_aligned(x + panel_w - theme::PAD - 90.0, y + 24.0, theme::H2, theme::TEXT_DIM, "-", Align::Right);
        p.text_aligned(x + panel_w - theme::PAD - 130.0, y + 24.0, theme::H2, theme::VANGUARD,
                       &mi.team_scores[1].to_string(), Align::Right);
    }

    let mut ty = y + 96.0;
    let cols = [0.0f32, 0.44, 0.55, 0.65, 0.75, 0.87];
    let head = ["PLAYER", "SCORE", "K", "D", "A", "PING"];
    for (i, label) in head.iter().enumerate() {
        let cx = x + theme::PAD + panel_w * cols[i];
        let align = if i == 0 { Align::Left } else { Align::Right };
        let ax = if i == 0 { cx } else { cx + 70.0 };
        p.text_aligned(ax, ty, theme::SMALL, theme::TEXT_DIM, label, align);
    }
    ty += 24.0;
    p.rule(x + theme::PAD, ty, panel_w - theme::PAD * 2.0);
    ty += 8.0;

    let mut rows: Vec<(u8, &crate::net::client::PlayerInfo)> = client.roster.iter()
        .enumerate()
        .filter(|(_, r)| r.present)
        .map(|(i, r)| (i as u8, r))
        .collect();
    rows.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.1.deaths.cmp(&b.1.deaths)));

    let teams: &[Team] = if mode.is_team_game() { &[Team::Phantom, Team::Vanguard] } else { &[Team::None] };
    for team in teams {
        if mode.is_team_game() {
            p.text(x + theme::PAD, ty, theme::BODY, theme::team_color(*team), team.name());
            ty += 26.0;
        }
        let mut index = 0;
        for (slot, r) in rows.iter() {
            if mode.is_team_game() && r.team != *team { continue; }
            if !mode.is_team_game() && r.team == Team::Spectator { continue; }
            let is_me = *slot == client.slot;
            p.row_background(x + theme::PAD, ty - 3.0, panel_w - theme::PAD * 2.0, 26.0, index, is_me);
            let name_color = if is_me { theme::ACCENT } else { theme::TEXT };
            let label = if r.is_bot { format!("{}  [BOT]", r.name) } else { r.name.clone() };
            p.text(x + theme::PAD + 8.0, ty, theme::BODY, name_color, &label);
            let vals = [
                r.score.to_string(),
                r.kills.to_string(),
                r.deaths.to_string(),
                r.assists.to_string(),
                if r.is_bot { "--".to_string() } else { r.ping.to_string() },
            ];
            for (i, v) in vals.iter().enumerate() {
                let cx = x + theme::PAD + panel_w * cols[i + 1] + 70.0;
                p.text_aligned(cx, ty, theme::BODY, theme::TEXT_DIM, v, Align::Right);
            }
            ty += 26.0;
            index += 1;
        }
        ty += 14.0;
    }

    let footer = format!(
        "PING {} MS    LOSS {:.1}%    SNAPSHOTS {:.0}/S",
        client.ping_ms(), client.loss() * 100.0, client.snapshot_hz
    );
    p.text_aligned(x + panel_w - theme::PAD, y + panel_h - 30.0, theme::SMALL, theme::TEXT_FAINT, &footer, Align::Right);
    let _ = now;
}

/// Colour for a team's marker, exported for the world view.
pub fn team_marker_color(team: Team) -> Color { theme::team_color(team) }

/// Who is speaking, bottom-left above the vitals.
///
/// Voice with no indication of who is talking is a disembodied noise; with a
/// name against it, it is a callout. The bar tracks the level the mixer is
/// actually producing, so it also says plainly whether someone's microphone is
/// working.
fn draw_voice(p: &mut Painter, f: &HudFrame, _w: f32, h: f32, s: f32) {
    let mut y = h - 150.0 * s;
    if f.transmitting {
        p.text_shadow(24.0 * s, y, theme::SMALL * s, theme::ACCENT, "TRANSMITTING");
        y -= 20.0 * s;
    }
    for (name, level) in f.voices.iter().take(4) {
        if *level < 0.004 { continue; }
        let bar = (level * 6.0).clamp(0.08, 1.0);
        p.rect(24.0 * s, y - 9.0 * s, 4.0 * s, 11.0 * s, theme::PANEL_DEEP);
        p.rect(24.0 * s, y + 2.0 * s - 11.0 * s * bar, 4.0 * s, 11.0 * s * bar, theme::GOOD);
        p.text_shadow(34.0 * s, y, theme::SMALL * s, theme::TEXT_BRIGHT, name);
        y -= 20.0 * s;
    }
}
