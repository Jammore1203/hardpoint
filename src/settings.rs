//! Player settings: display, audio, controls and gameplay preferences.
//!
//! Everything is stored in one plain-text file that the game rewrites when a
//! setting changes. A missing or corrupt file simply produces defaults, and a
//! setting the game no longer understands is ignored rather than fatal.

use crate::core::kv::{data_dir, Kv};
use crate::input::{Binding, Bindings, ALL_ACTIONS};
use std::path::PathBuf;

/// Quality tiers used by the texture, shadow and effects settings.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Quality {
    Low = 0,
    Medium = 1,
    High = 2,
}

impl Quality {
    pub fn from_u8(v: u8) -> Quality {
        match v { 0 => Quality::Low, 2 => Quality::High, _ => Quality::Medium }
    }
    pub fn label(self) -> &'static str {
        match self { Quality::Low => "LOW", Quality::Medium => "MEDIUM", Quality::High => "HIGH" }
    }
    pub fn next(self) -> Quality {
        match self { Quality::Low => Quality::Medium, Quality::Medium => Quality::High, Quality::High => Quality::Low }
    }
    /// Texture resolution for this tier.
    ///
    /// A 64-pixel material stretched over a three-metre floor is twenty texels
    /// per metre, which is why every surface in the game used to read as a
    /// smear. These are the sizes at which a procedural material actually has
    /// somewhere to put its detail.
    pub fn texture_size(self) -> u32 {
        match self { Quality::Low => 128, Quality::Medium => 256, Quality::High => 512 }
    }
}

#[derive(Clone)]
pub struct Settings {
    // ------------------------------------------------------------- display
    pub fullscreen: bool,
    pub window_width: u32,
    pub window_height: u32,
    pub vsync: bool,
    pub fps_limit: u32,
    /// Internal render scale, 0.35 to 1.0.
    pub resolution_scale: f32,
    pub texture_quality: Quality,
    pub shadow_quality: Quality,
    pub effects_quality: Quality,
    pub view_distance: f32,
    pub antialiasing: bool,
    /// Anisotropic filtering samples: 1 turns it off and restores point
    /// magnification, which is the sharper, more period-correct look but
    /// smears any surface seen at a grazing angle.
    pub anisotropy: u8,
    pub post_processing: bool,
    pub fov: f32,

    // --------------------------------------------------------------- retro
    pub vertex_snap: f32,
    pub affine_texturing: f32,
    pub scanlines: f32,
    pub vignette: f32,
    pub film_grade: bool,

    // --------------------------------------------------------------- audio
    pub master_volume: f32,
    pub sfx_volume: f32,
    pub music_volume: f32,
    pub voice_volume: f32,

    // --------------------------------------------------------------- input
    pub sensitivity: f32,
    pub ads_sensitivity: f32,
    pub invert_y: bool,
    pub bindings: Bindings,
    pub toggle_crouch: bool,
    pub toggle_ads: bool,
    pub auto_sprint: bool,

    // ------------------------------------------------------------ gameplay
    pub player_name: String,
    pub crosshair: u8,
    pub show_fps: bool,
    pub show_damage_numbers: bool,
    pub hud_scale: f32,
    pub view_bob: f32,
    pub screen_shake: f32,

    // ------------------------------------------------------------- network
    pub last_server: String,
    pub server_name: String,
    pub favourites: Vec<String>,

    path: PathBuf,
    dirty: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            fullscreen: false,
            window_width: 1600,
            window_height: 900,
            vsync: true,
            fps_limit: 0,
            resolution_scale: 1.0,
            texture_quality: Quality::Medium,
            shadow_quality: Quality::Medium,
            effects_quality: Quality::Medium,
            view_distance: 1.0,
            antialiasing: false,
            anisotropy: 16,
            post_processing: true,
            fov: 90.0,
            vertex_snap: 0.0,
            affine_texturing: 0.0,
            scanlines: 0.0,
            vignette: 0.22,
            film_grade: true,
            master_volume: 0.8,
            sfx_volume: 1.0,
            music_volume: 0.55,
            voice_volume: 0.9,
            sensitivity: 2.6,
            ads_sensitivity: 0.75,
            invert_y: false,
            bindings: Bindings::default(),
            toggle_crouch: false,
            toggle_ads: false,
            auto_sprint: false,
            player_name: default_name(),
            crosshair: 0,
            show_fps: false,
            show_damage_numbers: true,
            hud_scale: 1.0,
            view_bob: 1.0,
            screen_shake: 1.0,
            last_server: String::new(),
            server_name: format!("{}'S GAME", default_name()),
            favourites: Vec::new(),
            path: data_dir().join("settings.cfg"),
            dirty: false,
        }
    }
}

fn default_name() -> String {
    // The player's account name is a reasonable first guess, cleaned up.
    let raw = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "RECRUIT".to_string());
    let cleaned: String = raw.chars().filter(|c| c.is_alphanumeric()).take(16).collect();
    if cleaned.is_empty() { "RECRUIT".to_string() } else { cleaned.to_uppercase() }
}

impl Settings {
    pub fn load() -> Settings {
        let mut s = Settings::default();
        let kv = Kv::load(&s.path);

        s.fullscreen = kv.bool_or("display.fullscreen", s.fullscreen);
        s.window_width = kv.u32_or("display.width", s.window_width).clamp(640, 7680);
        s.window_height = kv.u32_or("display.height", s.window_height).clamp(480, 4320);
        s.vsync = kv.bool_or("display.vsync", s.vsync);
        s.fps_limit = kv.u32_or("display.fps_limit", s.fps_limit).min(1000);
        s.resolution_scale = kv.f32_or("display.resolution_scale", s.resolution_scale).clamp(0.35, 1.0);
        s.texture_quality = Quality::from_u8(kv.u32_or("display.texture_quality", 1) as u8);
        s.shadow_quality = Quality::from_u8(kv.u32_or("display.shadow_quality", 1) as u8);
        s.effects_quality = Quality::from_u8(kv.u32_or("display.effects_quality", 1) as u8);
        s.view_distance = kv.f32_or("display.view_distance", s.view_distance).clamp(0.4, 1.6);
        s.antialiasing = kv.bool_or("display.antialiasing", s.antialiasing);
        s.anisotropy = kv.u32_or("display.anisotropy", s.anisotropy as u32).clamp(1, 16) as u8;
        s.post_processing = kv.bool_or("display.post_processing", s.post_processing);
        s.fov = kv.f32_or("display.fov", s.fov).clamp(65.0, 120.0);

        s.vertex_snap = kv.f32_or("retro.vertex_snap", s.vertex_snap).clamp(0.0, 400.0);
        s.affine_texturing = kv.f32_or("retro.affine", s.affine_texturing).clamp(0.0, 1.0);
        s.scanlines = kv.f32_or("retro.scanlines", s.scanlines).clamp(0.0, 1.0);
        s.vignette = kv.f32_or("retro.vignette", s.vignette).clamp(0.0, 1.0);
        s.film_grade = kv.bool_or("retro.grade", s.film_grade);

        s.master_volume = kv.f32_or("audio.master", s.master_volume).clamp(0.0, 1.0);
        s.sfx_volume = kv.f32_or("audio.sfx", s.sfx_volume).clamp(0.0, 1.0);
        s.music_volume = kv.f32_or("audio.music", s.music_volume).clamp(0.0, 1.0);
        s.voice_volume = kv.f32_or("audio.voice", s.voice_volume).clamp(0.0, 1.0);

        s.sensitivity = kv.f32_or("input.sensitivity", s.sensitivity).clamp(0.1, 20.0);
        s.ads_sensitivity = kv.f32_or("input.ads_sensitivity", s.ads_sensitivity).clamp(0.1, 2.0);
        s.invert_y = kv.bool_or("input.invert_y", s.invert_y);
        s.toggle_crouch = kv.bool_or("input.toggle_crouch", s.toggle_crouch);
        s.toggle_ads = kv.bool_or("input.toggle_ads", s.toggle_ads);
        s.auto_sprint = kv.bool_or("input.auto_sprint", s.auto_sprint);
        for a in ALL_ACTIONS {
            if let Some(v) = kv.get(a.config_key()) {
                let b = Binding::parse(v);
                s.bindings.assign(a, b);
            }
        }

        s.player_name = kv.str_or("game.name", &s.player_name).to_string();
        s.crosshair = kv.u32_or("game.crosshair", 0) as u8 % 4;
        s.show_fps = kv.bool_or("game.show_fps", s.show_fps);
        s.show_damage_numbers = kv.bool_or("game.damage_numbers", s.show_damage_numbers);
        s.hud_scale = kv.f32_or("game.hud_scale", s.hud_scale).clamp(0.7, 1.4);
        s.view_bob = kv.f32_or("game.view_bob", s.view_bob).clamp(0.0, 1.5);
        s.screen_shake = kv.f32_or("game.screen_shake", s.screen_shake).clamp(0.0, 1.5);

        s.last_server = kv.str_or("net.last_server", "").to_string();
        s.server_name = kv.str_or("net.server_name", &s.server_name).to_string();
        s.favourites = kv.list("net.favourites").iter().map(|x| x.to_string()).collect();

        s
    }

    pub fn mark_dirty(&mut self) { self.dirty = true; }
    pub fn is_dirty(&self) -> bool { self.dirty }

    /// Writes the file if anything changed. Called when leaving a settings
    /// screen and on exit, never every frame.
    pub fn save_if_dirty(&mut self) {
        if !self.dirty { return; }
        self.dirty = false;
        let _ = self.save();
    }

    pub fn save(&self) -> std::io::Result<()> {
        let mut kv = Kv::new();
        kv.set_bool("display.fullscreen", self.fullscreen);
        kv.set_i32("display.width", self.window_width as i32);
        kv.set_i32("display.height", self.window_height as i32);
        kv.set_bool("display.vsync", self.vsync);
        kv.set_i32("display.fps_limit", self.fps_limit as i32);
        kv.set_f32("display.resolution_scale", self.resolution_scale);
        kv.set_i32("display.texture_quality", self.texture_quality as i32);
        kv.set_i32("display.shadow_quality", self.shadow_quality as i32);
        kv.set_i32("display.effects_quality", self.effects_quality as i32);
        kv.set_f32("display.view_distance", self.view_distance);
        kv.set_bool("display.antialiasing", self.antialiasing);
        kv.set_i32("display.anisotropy", self.anisotropy as i32);
        kv.set_bool("display.post_processing", self.post_processing);
        kv.set_f32("display.fov", self.fov);

        kv.set_f32("retro.vertex_snap", self.vertex_snap);
        kv.set_f32("retro.affine", self.affine_texturing);
        kv.set_f32("retro.scanlines", self.scanlines);
        kv.set_f32("retro.vignette", self.vignette);
        kv.set_bool("retro.grade", self.film_grade);

        kv.set_f32("audio.master", self.master_volume);
        kv.set_f32("audio.sfx", self.sfx_volume);
        kv.set_f32("audio.music", self.music_volume);
        kv.set_f32("audio.voice", self.voice_volume);

        kv.set_f32("input.sensitivity", self.sensitivity);
        kv.set_f32("input.ads_sensitivity", self.ads_sensitivity);
        kv.set_bool("input.invert_y", self.invert_y);
        kv.set_bool("input.toggle_crouch", self.toggle_crouch);
        kv.set_bool("input.toggle_ads", self.toggle_ads);
        kv.set_bool("input.auto_sprint", self.auto_sprint);
        for a in ALL_ACTIONS {
            kv.set(a.config_key(), self.bindings.get(a).name());
        }

        kv.set("game.name", &self.player_name);
        kv.set_i32("game.crosshair", self.crosshair as i32);
        kv.set_bool("game.show_fps", self.show_fps);
        kv.set_bool("game.damage_numbers", self.show_damage_numbers);
        kv.set_f32("game.hud_scale", self.hud_scale);
        kv.set_f32("game.view_bob", self.view_bob);
        kv.set_f32("game.screen_shake", self.screen_shake);

        kv.set("net.last_server", &self.last_server);
        kv.set("net.server_name", &self.server_name);
        kv.set_list("net.favourites", &self.favourites);

        kv.save(&self.path)
    }

    /// The renderer's view of these settings.
    pub fn render_settings(&self) -> crate::render::RenderSettings {
        crate::render::RenderSettings {
            resolution_scale: self.resolution_scale,
            msaa: self.antialiasing,
            anisotropy: self.anisotropy.clamp(1, 16),
            vertex_snap: self.vertex_snap,
            affine_texturing: self.affine_texturing,
            scanlines: self.scanlines,
            vignette: self.vignette,
            exposure: 1.0,
            saturation: if self.film_grade { 1.0 } else { 1.0 },
            view_distance: self.view_distance,
            post_processing: self.post_processing,
            texture_lod_bias: 0.0,
            shadows: self.shadow_quality != Quality::Low,
            particles: match self.effects_quality {
                Quality::Low => 0.4,
                Quality::Medium => 1.0,
                Quality::High => 1.6,
            },
        }
    }

    pub fn mesh_bake_quality(&self) -> crate::assets::meshgen::BakeQuality {
        use crate::assets::meshgen::BakeQuality;
        match self.shadow_quality {
            Quality::Low => BakeQuality::Flat,
            Quality::Medium => BakeQuality::Shadows,
            Quality::High => BakeQuality::Full,
        }
    }

    /// Everything turned down, for the weakest hardware the game targets.
    pub fn apply_low_preset(&mut self) {
        self.resolution_scale = 0.6;
        self.texture_quality = Quality::Low;
        self.shadow_quality = Quality::Low;
        self.effects_quality = Quality::Low;
        self.view_distance = 0.65;
        self.antialiasing = false;
        self.anisotropy = 2;
        self.post_processing = false;
        self.vsync = false;
        self.fps_limit = 0;
        self.dirty = true;
    }

    pub fn apply_balanced_preset(&mut self) {
        self.resolution_scale = 1.0;
        self.texture_quality = Quality::Medium;
        self.shadow_quality = Quality::Medium;
        self.effects_quality = Quality::Medium;
        self.view_distance = 1.0;
        self.antialiasing = false;
        self.anisotropy = 16;
        self.post_processing = true;
        self.dirty = true;
    }

    pub fn apply_high_preset(&mut self) {
        self.resolution_scale = 1.0;
        self.texture_quality = Quality::High;
        self.shadow_quality = Quality::High;
        self.effects_quality = Quality::High;
        self.view_distance = 1.35;
        self.antialiasing = true;
        self.anisotropy = 16;
        self.post_processing = true;
        self.dirty = true;
    }

    /// The full period treatment: low internal resolution, wobbling vertices,
    /// affine texturing and scanlines.
    pub fn apply_authentic_preset(&mut self) {
        self.resolution_scale = 0.45;
        self.texture_quality = Quality::Low;
        self.shadow_quality = Quality::Medium;
        self.effects_quality = Quality::Medium;
        self.antialiasing = false;
        self.anisotropy = 1;
        self.post_processing = true;
        self.vertex_snap = 190.0;
        self.affine_texturing = 0.85;
        self.scanlines = 0.30;
        self.vignette = 0.34;
        self.dirty = true;
    }

    pub fn remember_server(&mut self, addr: &str) {
        if addr.is_empty() { return; }
        self.last_server = addr.to_string();
        self.favourites.retain(|f| f != addr);
        self.favourites.insert(0, addr.to_string());
        self.favourites.truncate(8);
        self.dirty = true;
    }
}
