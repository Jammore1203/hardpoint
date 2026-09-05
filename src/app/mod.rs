//! The application: window, main loop, and the screen state machine.
//!
//! One structure owns everything the game needs and one function advances it.
//! The screens are plain functions that draw and return what happened, and the
//! game itself is one more screen, so there is a single place where input is
//! read, a single place where the frame is drawn, and no hidden state
//! anywhere else.

pub mod effects;
pub mod game;
pub mod hud;
pub mod screens;
pub mod world_view;

use crate::assets::meshgen::{self, MapMesh};
use crate::audio::{AudioEngine, SoundBank};
use crate::core::{FrameClock, RateLimiter};
use crate::game::loadout::Loadout;
use crate::input::{Action, InputState};
use crate::game::types::MAX_PLAYERS;
use crate::maps::{MapData, MapId};
use crate::modes::ModeId;
use crate::net::client::{Client, ClientState};
use crate::net::discovery::Browser;
use crate::net::protocol::DEFAULT_PORT;
use crate::net::server::{Server, ServerConfig};
use crate::progression::{MatchSummary, Progression};
use crate::render::{Camera, Renderer};
use crate::settings::Settings;
use crate::ui::widgets::UiSound;
use effects::Effects;
use hud::Hud;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};
use world_view::{ClientWorld, ViewModel};

/// Which screen is in front of the player.
#[derive(Clone, Debug, PartialEq)]
pub enum Screen {
    Splash,
    MainMenu,
    Multiplayer,
    Browser,
    HostSetup,
    DirectConnect,
    Lobby,
    Loadout,
    Settings,
    Controls,
    Career,
    Connecting,
    Loading,
    InGame,
    Paused,
    Results,
}

impl Screen {
    fn id(&self) -> u8 {
        match self {
            Screen::Splash => 0, Screen::MainMenu => 1, Screen::Multiplayer => 2,
            Screen::Browser => 3, Screen::HostSetup => 4, Screen::DirectConnect => 5,
            Screen::Lobby => 6, Screen::Loadout => 7, Screen::Settings => 8,
            Screen::Controls => 9, Screen::Career => 10, Screen::Connecting => 11,
            Screen::Loading => 12, Screen::InGame => 13, Screen::Paused => 14,
            Screen::Results => 15,
        }
    }
    /// Screens that draw the world behind them.
    fn over_world(&self) -> bool {
        matches!(self, Screen::InGame | Screen::Paused | Screen::Results | Screen::Loadout)
    }
}

/// A server running on a background thread.
struct HostedServer {
    shutdown: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    pub port: u16,
}

impl HostedServer {
    fn start(cfg: ServerConfig) -> Result<HostedServer, String> {
        let mut server = Server::bind(cfg).map_err(|e| format!("could not open a server socket: {e}"))?;
        let port = server.port();
        let shutdown = Arc::new(AtomicBool::new(false));
        let flag = shutdown.clone();
        let join = std::thread::Builder::new()
            .name("hardpoint-server".into())
            .spawn(move || {
                // The server runs on its own clock so the host's frame rate
                // cannot affect anybody else's match.
                let mut clock = FrameClock::new();
                let mut limiter = RateLimiter::new(120.0);
                let mut score_timer = 0.0f32;
                while !flag.load(Ordering::Relaxed) {
                    clock.tick();
                    server.update(clock.dt);
                    score_timer += clock.dt;
                    if score_timer > 0.5 {
                        score_timer = 0.0;
                        server.broadcast_scores();
                    }
                    limiter.wait();
                }
            })
            .map_err(|e| format!("could not start the server thread: {e}"))?;
        Ok(HostedServer { shutdown, join: Some(join), port })
    }
}

impl Drop for HostedServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Options chosen on the host setup screen.
#[derive(Clone)]
pub struct HostOptions {
    pub name: String,
    pub map: MapId,
    pub mode: ModeId,
    pub bots: u8,
    pub difficulty: u8,
    pub max_players: u8,
    pub friendly_fire: bool,
    pub score_limit: u16,
    pub time_limit: u16,
    pub password: String,
    pub rotate_maps: bool,
}

impl Default for HostOptions {
    fn default() -> Self {
        HostOptions {
            name: "HARDPOINT SERVER".into(),
            map: MapId::Ironveil,
            mode: ModeId::TeamDeathmatch,
            bots: 8,
            difficulty: 1,
            max_players: 12,
            friendly_fire: false,
            score_limit: ModeId::TeamDeathmatch.default_score_limit(),
            time_limit: ModeId::TeamDeathmatch.default_time_limit(),
            password: String::new(),
            rotate_maps: true,
        }
    }
}

pub struct App {
    pub window: Arc<Window>,
    pub renderer: Renderer,
    pub audio: AudioEngine,
    pub settings: Settings,
    pub progression: Progression,
    pub input: InputState,
    clock: FrameClock,
    limiter: RateLimiter,

    pub screen: Screen,
    screen_stack: Vec<Screen>,
    pub focus: usize,
    focus_memory: [usize; 16],
    pub status: Option<(String, f64)>,
    pub scroll: usize,
    /// Set while the controls screen is waiting for a key.
    pub rebinding: Option<Action>,
    pub text_target: Option<TextTarget>,

    pub client: Option<Client>,
    server: Option<HostedServer>,
    pub browser: Browser,
    pub host: HostOptions,
    pub connect_address: String,
    pub connect_password: String,

    pub map: Option<MapData>,
    loaded_map: Option<MapId>,
    pub world: ClientWorld,
    pub effects: Effects,
    pub hud: Hud,
    pub viewmodel: ViewModel,
    pub loadout: Loadout,

    camera: Camera,
    yaw: f32,
    pitch: f32,
    recoil: crate::game::movement::RecoilState,
    shake: f32,
    shake_seed: f32,
    damage_flash: f32,
    flash_blind: f32,
    mouse_captured: bool,
    pub(crate) toggle_ads_state: bool,
    pub(crate) toggle_crouch_state: bool,

    bank_rx: Option<Receiver<SoundBank>>,
    splash_time: f32,
    loading_message: String,
    match_start: f64,
    summary: MatchSummary,
    results_shown: bool,
    last_level: u8,
    pub show_perf: bool,
    quit: bool,

    // ------------------------------------------------------- developer aids
    /// Frame counter, used only by the screenshot script.
    frames: u64,
    /// `seconds -> file` screenshots requested through the environment.
    shots: Vec<(f64, String)>,
    script_step: usize,
    pending_shot: Option<String>,
    autoplay: bool,
    script_last: f64,
    visits: Vec<(f64, Screen)>,
    exit_at: f64,
    /// Frame times collected for --bench style reporting.
    bench: Option<Vec<f32>>,
    /// Per-shooter shot counter, so only every third round draws a tracer.
    tracer_countdown: [u8; MAX_PLAYERS],
    /// Set when the session was launched pointing at an external server.
    pub joined_remote: bool,
}

/// Where typed characters currently go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextTarget {
    PlayerName,
    ServerName,
    ConnectAddress,
    ConnectPassword,
    Chat,
}

impl App {
    pub fn new(window: Arc<Window>, settings: Settings, progression: Progression) -> Result<App, String> {
        let render_settings = settings.render_settings();
        let texture_size = settings.texture_quality.texture_size();
        let renderer = Renderer::new(window.clone(), render_settings, settings.vsync, texture_size)?;

        let audio = AudioEngine::new();
        audio.set_volumes(
            settings.master_volume,
            settings.sfx_volume,
            settings.music_volume,
            settings.voice_volume,
        );

        // The sound bank takes a few hundred milliseconds to synthesise, so it
        // is built on a worker while the splash is on screen.
        let sample_rate = audio.sample_rate;
        let (tx, bank_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("hardpoint-audio-bank".into())
            .spawn(move || {
                let bank = SoundBank::generate(sample_rate);
                let _ = tx.send(bank);
            })
            .ok();

        let mut limiter = RateLimiter::new(settings.fps_limit.max(1) as f32);
        if settings.fps_limit == 0 { limiter.set_hz(0.0); }

        let mut host = HostOptions {
            name: settings.server_name.clone(),
            ..HostOptions::default()
        };
        host.bots = 8;

        let level = progression.level;
        let mut loadout = Loadout::default();
        loadout.sanitize(level);

        Ok(App {
            window,
            renderer,
            audio,
            settings,
            progression,
            input: InputState::new(),
            clock: FrameClock::new(),
            limiter,
            screen: Screen::Splash,
            screen_stack: Vec::new(),
            focus: 0,
            focus_memory: [0; 16],
            status: None,
            scroll: 0,
            rebinding: None,
            text_target: None,
            client: None,
            server: None,
            browser: Browser::new().map_err(|e| format!("could not open a network socket: {e}"))?,
            host,
            connect_address: String::new(),
            connect_password: String::new(),
            map: None,
            loaded_map: None,
            world: ClientWorld::new(),
            effects: Effects::new(),
            hud: Hud::new(),
            viewmodel: ViewModel::default(),
            loadout,
            camera: Camera::default(),
            yaw: 0.0,
            pitch: 0.0,
            recoil: crate::game::movement::RecoilState::default(),
            shake: 0.0,
            shake_seed: 0.0,
            damage_flash: 0.0,
            flash_blind: 0.0,
            mouse_captured: false,
            toggle_ads_state: false,
            toggle_crouch_state: false,
            bank_rx: Some(bank_rx),
            splash_time: 0.0,
            loading_message: String::new(),
            match_start: 0.0,
            summary: MatchSummary::default(),
            results_shown: false,
            last_level: level,
            show_perf: false,
            quit: false,
            frames: 0,
            shots: parse_shots(),
            script_step: 0,
            pending_shot: None,
            autoplay: std::env::var_os("HARDPOINT_AUTOPLAY").is_some(),
            script_last: 0.0,
            visits: parse_visits(),
            exit_at: std::env::var("HARDPOINT_EXIT_AT").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0),
            bench: std::env::var_os("HARDPOINT_BENCH").map(|_| Vec::with_capacity(1 << 16)),
            tracer_countdown: [0; MAX_PLAYERS],
            joined_remote: false,
        })
    }

    pub fn should_quit(&self) -> bool { self.quit }

    // ------------------------------------------------------------ helpers

    pub fn now(&self) -> f64 { self.clock.now_secs() }
    pub fn dt(&self) -> f32 { self.clock.dt }
    pub fn fps(&self) -> f32 { self.clock.fps }

    pub fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some((text.into(), self.clock.now_secs()));
    }

    pub fn push_screen(&mut self, screen: Screen) {
        self.focus_memory[self.screen.id() as usize % 16] = self.focus;
        self.screen_stack.push(self.screen.clone());
        self.goto(screen);
    }

    pub fn goto(&mut self, screen: Screen) {
        self.focus_memory[self.screen.id() as usize % 16] = self.focus;
        self.screen = screen;
        self.focus = self.focus_memory[self.screen.id() as usize % 16];
        self.scroll = 0;
        self.text_target = None;
        self.rebinding = None;
        // The browser is only useful populated, so entering it always searches
        // rather than relying on whichever route brought the player here.
        if matches!(self.screen, Screen::Browser) {
            let now = self.clock.now_secs();
            self.browser.refresh(now);
        }
    }

    pub fn back(&mut self) {
        let target = self.screen_stack.pop().unwrap_or(Screen::MainMenu);
        self.goto(target);
        self.play_ui(UiSound::Back);
    }

    pub fn play_ui(&mut self, s: UiSound) {
        let Some(bank) = self.audio.bank.clone() else { return };
        let clip = match s {
            UiSound::Move => bank.ui_move.clone(),
            UiSound::Select => bank.ui_select.clone(),
            UiSound::Back => bank.ui_back.clone(),
            UiSound::Error => bank.ui_error.clone(),
        };
        self.audio.play_ui(clip, 0.55, 1.0);
    }

    pub fn capture_mouse(&mut self, on: bool) {
        if self.mouse_captured == on { return; }
        self.mouse_captured = on;
        self.input.captured = on;
        if on {
            let _ = self.window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| self.window.set_cursor_grab(CursorGrabMode::Confined));
            self.window.set_cursor_visible(false);
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
            self.window.set_cursor_visible(true);
        }
    }

    pub fn apply_settings(&mut self) {
        let rs = self.settings.render_settings();
        self.renderer.apply_settings(rs, self.settings.vsync);
        self.audio.set_volumes(
            self.settings.master_volume,
            self.settings.sfx_volume,
            self.settings.music_volume,
            self.settings.voice_volume,
        );
        if self.settings.fps_limit == 0 {
            self.limiter.set_hz(0.0);
        } else {
            self.limiter.set_hz(self.settings.fps_limit as f32);
        }
        self.effects.density = rs.particles;
        let fullscreen = if self.settings.fullscreen {
            Some(Fullscreen::Borderless(None))
        } else {
            None
        };
        self.window.set_fullscreen(fullscreen);
        self.settings.mark_dirty();
    }

    // -------------------------------------------------------------- hosting

    pub fn start_host(&mut self) {
        self.stop_networking();
        let rotation = if self.host.rotate_maps {
            crate::maps::ALL_MAPS.to_vec()
        } else {
            vec![self.host.map]
        };
        let cfg = ServerConfig {
            name: self.host.name.clone(),
            port: DEFAULT_PORT,
            max_players: self.host.max_players,
            bot_count: self.host.bots,
            map: self.host.map,
            mode: self.host.mode,
            password: self.host.password.clone(),
            friendly_fire: self.host.friendly_fire,
            snapshot_hz: 22.0,
            dedicated: false,
            rotation,
            lobby_countdown: 25.0,
            bot_difficulty: self.host.difficulty,
        };
        match HostedServer::start(cfg) {
            Ok(h) => {
                let port = h.port;
                self.server = Some(h);
                self.connect_to(&format!("127.0.0.1:{}", port), "");
                self.set_status(format!("HOSTING ON PORT {}", port));
            }
            Err(e) => {
                self.set_status(e);
                self.play_ui(UiSound::Error);
            }
        }
    }

    pub fn connect_to(&mut self, address: &str, password: &str) {
        let mut client = match Client::new() {
            Ok(c) => c,
            Err(e) => {
                self.set_status(format!("NETWORK ERROR: {e}"));
                return;
            }
        };
        match client.connect(address, password, self.clock.now_secs()) {
            Ok(()) => {
                self.client = Some(client);
                self.goto(Screen::Connecting);
            }
            Err(e) => {
                self.set_status(e);
                self.play_ui(UiSound::Error);
            }
        }
    }

    pub fn stop_networking(&mut self) {
        if let Some(c) = &mut self.client { c.disconnect(); }
        self.client = None;
        self.server = None;
        self.map = None;
        self.loaded_map = None;
        self.renderer.clear_map();
        self.effects.clear();
        self.hud.reset();
        self.audio.stop_all_world_sound();
    }

    /// Builds a map and uploads its geometry. Blocking, behind a loading
    /// screen; on this hardware it takes a few tens of milliseconds.
    pub fn load_map(&mut self, id: MapId) {
        let t0 = std::time::Instant::now();
        let map = id.build();
        let mesh: MapMesh = meshgen::build_map_mesh(&map, self.settings.mesh_bake_quality());
        self.renderer.upload_map(&mesh);
        self.world.reset(&map);
        self.effects.clear();
        self.effects.set_weather(map.env.weather, Vec3::ZERO);
        self.audio.play_ambience(map.env.ambience);
        self.audio.play_music(map.env.track);
        self.loading_message = format!(
            "{} - {} triangles in {:.0} ms",
            id.name(), mesh.triangle_count, t0.elapsed().as_secs_f32() * 1000.0
        );
        self.map = Some(map);
        self.loaded_map = Some(id);
    }

    // ---------------------------------------------------------------- frame

    pub fn frame(&mut self) {
        self.clock.tick();
        let dt = self.clock.dt;
        let now = self.clock.now_secs();
        self.frames += 1;

        self.run_script();
        self.pump_network(now);
        self.update(dt, now);
        self.draw(dt, now);
        self.write_pending_shot();

        self.input.end_frame();
        self.limiter.wait();
        if self.bench.is_none() && self.exit_at > 0.0 { /* score still reported */ }
        if let Some(b) = &mut self.bench {
            // Only frames of actual gameplay: menus are not the thing being
            // measured, and the first second is load and warm-up.
            if matches!(self.screen, Screen::InGame) && now > 8.0 {
                b.push(self.clock.raw_dt);
            }
        }
        if self.exit_at > 0.0 && now >= self.exit_at {
            self.report_bench();
            self.quit = true;
        }
    }

    /// Drives the scripted screenshot run. Does nothing unless the relevant
    /// environment variables are set.
    fn run_script(&mut self) {
        let now = self.clock.now_secs();
        // Screenshots are scheduled in seconds, not frames: with vsync off on
        // an unmapped window the frame rate is meaningless.
        if self.pending_shot.is_none() {
            if let Some(i) = self.shots.iter().position(|(t, _)| now >= *t) {
                let (_, path) = self.shots.remove(i);
                self.renderer.capture_request = true;
                self.pending_shot = Some(path);
            }
        }
        if !self.autoplay { return; }
        let prev_now = std::mem::replace(&mut self.script_last, now);
        let _ = prev_now;
        // A client launched with --connect must not also host: it is already
        // joining someone else's server.
        let steps: &[(f64, u8)] = if self.joined_remote || std::env::var_os("HARDPOINT_NOHOST").is_some() {
            if self.joined_remote { &[(4.5, 2)] } else { &[] }
        } else {
            &[(2.0, 0), (4.5, 1), (5.2, 2)]
        };
        while self.script_step < steps.len() && now >= steps[self.script_step].0 {
            match steps[self.script_step].1 {
                0 => {
                    self.host = HostOptions {
                        name: "SCREENSHOT SERVER".into(),
                        bots: std::env::var("HARDPOINT_BOTS").ok()
                            .and_then(|v| v.parse().ok()).unwrap_or(9),
                        difficulty: std::env::var("HARDPOINT_BOTSKILL").ok()
                            .and_then(|v| v.parse().ok()).unwrap_or(HostOptions::default().difficulty),
                        map: std::env::var("HARDPOINT_MAP").ok()
                            .and_then(|v| crate::maps::ALL_MAPS.iter()
                                .find(|m| m.name().eq_ignore_ascii_case(&v)).copied())
                            .unwrap_or(HostOptions::default().map),
                        ..HostOptions::default()
                    };
                    self.start_host();
                }
                1 => {
                    if let Some(c) = &mut self.client {
                        c.send_message(&crate::net::protocol::ClientMsg::HostStart);
                    }
                }
                _ => self.goto(Screen::InGame),
            }
            self.script_step += 1;
        }

        while let Some(i) = self.visits.iter().position(|(t, _)| now >= *t) {
            let (_, screen) = self.visits.remove(i);
            self.goto(screen);
        }

        // Once in the match, drive the player around so captures are not all
        // the same frozen viewpoint. Purely a capture aid; it feeds the same
        // input state a human would.
        if matches!(self.screen, Screen::InGame) {
            let t = now - 6.0;
            if t > 0.0 {
                use winit::keyboard::KeyCode;
                self.input.set_key(KeyCode::KeyW, (t % 7.0) < 5.0);
                self.input.set_key(KeyCode::KeyD, (t % 13.0) < 3.5);
                self.input.set_key(KeyCode::Space, (t % 9.0) < 0.1);
                let engaged = self.nearest_enemy_angles().is_some();
                // Fire when there is something to shoot, so the harness
                // produces real hits, kills and progression.
                self.input.set_mouse(winit::event::MouseButton::Left,
                                     if engaged { (t % 0.9) < 0.55 } else { (t % 2.6) < 0.30 });
                // Aim down sights when engaged, as a player would: hip fire is
                // a close-range tool and testing with it measures nothing.
                self.input.set_mouse(winit::event::MouseButton::Right, engaged);
                if engaged {
                    self.input.set_key(winit::keyboard::KeyCode::KeyW, false);
                    self.input.set_key(winit::keyboard::KeyCode::KeyD, false);
                    self.input.set_key(winit::keyboard::KeyCode::Space, false);
                }
                // Aim at the nearest enemy when there is one, so the harness
                // exercises hit feedback, kills and progression rather than
                // firing at the sky; otherwise sweep to cover ground.
                if let Some((yaw, pitch)) = self.nearest_enemy_angles() {
                    let step = (now - prev_now).clamp(0.0, 0.05) as f32;
                    let k = (step * 9.0).min(1.0);
                    self.yaw += crate::core::angle_delta(self.yaw, yaw) * k;
                    self.pitch += (pitch - self.pitch) * k;
                } else {
                    // Scaled by frame time: the capture window runs uncapped,
                    // so a per-frame constant would spin the view into the floor.
                    let step = (now - prev_now).clamp(0.0, 0.05) as f32;
                    self.input.add_motion((t * 0.35).sin() as f32 * 260.0 * step, 0.0);
                }
            }
        }
    }

    /// View angles onto the closest visible enemy, for the capture harness.
    fn nearest_enemy_angles(&self) -> Option<(f32, f32)> {
        let client = self.client.as_ref()?;
        if !client.local.alive { return None; }
        let map = self.map.as_ref()?;
        let eye = client.local.eye();
        let mine = client.my_team();
        let mut best: Option<(f32, Vec3)> = None;
        for (i, p) in client.players.iter().enumerate() {
            if i as u8 == client.slot || !p.present { continue; }
            if p.snap.flags.contains(crate::game::types::PFlags::DEAD) { continue; }
            if mine != crate::game::types::Team::None && p.team == mine { continue; }
            let target = p.render_pos + Vec3::Y * 1.2;
            let d = (target - eye).length();
            if d > 60.0 { continue; }
            if best.is_some_and(|(bd, _)| d >= bd) { continue; }
            let dir = (target - eye) / d.max(0.001);
            if map.collision.trace_ray(eye, dir, d, crate::maps::brush::TraceMask::Solid).hit {
                continue;
            }
            best = Some((d, target));
        }
        let (_, target) = best?;
        let (yaw, pitch) = crate::math::angles_from_dir((target - eye).normalize_or_zero());
        Some((yaw, pitch))
    }

    /// Prints frame-time percentiles. Averages hide hitching; the low
    /// percentiles are what a player actually feels.
    fn report_bench(&mut self) {
        // The local player's tally against the bots: the only honest measure
        // of how hard the AI actually is, as opposed to how hard bots are on
        // each other, which is symmetric and says nothing.
        if let Some(c) = &self.client {
            if let Some(me) = c.player_info(c.slot) {
                println!("[score] you {}-{} ({:.2} K/D)  bots: {}",
                         me.kills, me.deaths,
                         me.kills as f32 / me.deaths.max(1) as f32,
                         c.roster.iter().filter(|r| r.present && r.is_bot).count());
            }
        }
        let Some(mut b) = self.bench.take() else { return };
        if b.len() < 32 { return; }
        let n = b.len();
        let total: f64 = b.iter().map(|v| *v as f64).sum();
        b.sort_by(|a, c| a.partial_cmp(c).unwrap());
        let pct = |p: f64| b[((n as f64 * p) as usize).min(n - 1)];
        println!(
            "[bench] {} frames  avg {:.1} fps  median {:.1}  1% low {:.1}  0.1% low {:.1}",
            n,
            n as f64 / total,
            1.0 / pct(0.50) as f64,
            1.0 / pct(0.99) as f64,
            1.0 / pct(0.999) as f64,
        );
    }

    fn write_pending_shot(&mut self) {
        let Some(path) = self.pending_shot.take() else { return };
        let Some((w, h, pixels)) = self.renderer.captured.take() else {
            // The capture was requested but did not arrive; try again shortly.
            self.pending_shot = Some(path);
            return;
        };
        let png = crate::devtools::png::encode_rgba(w, h, &pixels);
        match std::fs::write(&path, png) {
            Ok(()) => println!("[screenshot] {} ({}x{})", path, w, h),
            Err(e) => eprintln!("[screenshot] could not write {}: {}", path, e),
        }
    }

    fn pump_network(&mut self, now: f64) {
        self.browser.update(now);
        if let Some(client) = &mut self.client {
            client.update(now);
        }
        // A map change from the server drops us into the loading screen.
        let pending = self.client.as_mut().and_then(|c| c.map_change.take());
        if let Some((map, _mode)) = pending {
            if self.loaded_map != Some(map) {
                self.loading_message = format!("LOADING {}", map.name());
                self.goto(Screen::Loading);
                self.load_map(map);
                if matches!(self.screen, Screen::Loading) {
                    self.goto(Screen::Lobby);
                }
            }
        }
    }

    fn update(&mut self, dt: f32, now: f64) {
        // Collect the sound bank once it is ready.
        if let Some(rx) = &self.bank_rx {
            if let Ok(bank) = rx.try_recv() {
                self.audio.set_bank(Arc::new(bank));
                self.bank_rx = None;
            }
        }

        if let Some((_, t)) = self.status {
            if now - t > 4.0 { self.status = None; }
        }

        self.hud.update(dt, now);
        self.damage_flash = (self.damage_flash - dt * 1.6).max(0.0);
        self.flash_blind = (self.flash_blind - dt * 0.9).max(0.0);
        self.shake = (self.shake - dt * 2.4).max(0.0);
        self.shake_seed += dt * 37.0;

        let screen = self.screen.clone();
        match screen {
            Screen::Splash => {
                self.splash_time += dt;
                if self.splash_time > 1.4 || self.input.pressed(&self.settings.bindings, Action::Pause) {
                    self.goto(Screen::MainMenu);
                    self.audio.play_music(crate::maps::MusicTrack::Menu);
                }
            }
            Screen::Connecting => self.update_connecting(now),
            Screen::InGame => self.update_game(dt, now),
            Screen::Lobby | Screen::Paused | Screen::Results | Screen::Loadout
            | Screen::Settings | Screen::Controls | Screen::Career => {
                // These can all sit on top of a live match, so they keep the
                // connection fed as well as the lobby list and match state.
                self.tick_connected_menu(dt, now);
            }
            _ => {
                self.capture_mouse(false);
            }
        }

        // Leaving a match for any reason returns the player to the menu with
        // an explanation rather than a black screen.
        if let Some(c) = &self.client {
            if let ClientState::Failed(reason) = &c.state {
                let reason = reason.clone();
                self.stop_networking();
                self.set_status(reason);
                self.goto(Screen::MainMenu);
                self.audio.play_music(crate::maps::MusicTrack::Menu);
            }
        }
    }

    fn update_connecting(&mut self, now: f64) {
        if self.client.is_none() {
            self.goto(Screen::MainMenu);
            return;
        }
        let live = self.client.as_ref().map(|c| c.state.is_live()).unwrap_or(false);
        if !live { return; }

        let name = self.settings.player_name.clone();
        let level = self.progression.level;
        let loadout = self.loadout;
        let (map, address) = {
            let client = self.client.as_mut().unwrap();
            client.hello(&name, level, &loadout);
            (client.match_info.map, client.address_text.clone())
        };

        if self.loaded_map != Some(map) {
            self.loading_message = format!("LOADING {}", map.name());
            self.goto(Screen::Loading);
            self.load_map(map);
        }
        self.goto(Screen::Lobby);
        self.settings.remember_server(&address);
        self.match_start = now;
    }

    pub fn leave_match(&mut self) {
        self.finish_match_stats();
        self.stop_networking();
        self.goto(Screen::MainMenu);
        self.screen_stack.clear();
        self.audio.play_music(crate::maps::MusicTrack::Menu);
    }

    fn finish_match_stats(&mut self) {
        if self.results_shown { return; }
        let Some(client) = &self.client else { return };
        let Some(me) = client.player_info(client.slot) else { return };
        let mi = &client.match_info;
        let won = mi.winner != crate::game::types::Team::None && mi.winner == client.my_team();
        self.summary = MatchSummary {
            won,
            mvp: mi.mvp == client.slot,
            kills: me.kills as u32,
            deaths: me.deaths as u32,
            assists: me.assists as u32,
            score: me.score,
            captures: 0,
            plants: 0,
            defuses: 0,
            best_streak: 0,
            duration_secs: (self.clock.now_secs() - self.match_start).max(0.0) as u32,
            distance: 0.0,
        };
        let before = self.progression.level;
        let (_xp, _done) = self.progression.finish_match(&self.summary);
        self.last_level = before;
        self.progression.save_if_dirty();
        self.results_shown = true;
    }

    fn draw(&mut self, dt: f32, now: f64) {
        self.renderer.begin();

        let over_world = self.screen.over_world() && self.map.is_some();
        if over_world {
            self.build_world_frame(dt, now);
        }

        screens::draw(self, dt, now);

        let env = self.map.as_ref().map(|m| m.env).unwrap_or_default();
        let camera = self.camera;
        let flash = self.flash_blind.min(1.0);
        let damage = self.damage_flash;
        if let Err(e) = self.renderer.render(&camera, &env, now as f32, flash, damage) {
            match e {
                wgpu::SurfaceError::OutOfMemory => {
                    eprintln!("[graphics] out of memory; exiting");
                    self.quit = true;
                }
                _ => self.renderer.gpu.reconfigure(),
            }
        }
    }

    fn build_world_frame(&mut self, dt: f32, now: f64) {
        let Some(map) = &self.map else { return };
        let Some(client) = &self.client else { return };

        // Camera: eye position from prediction, angles from local input, with
        // recoil and shake layered on for feel only.
        let eye = client.view_position();
        let shake = if self.shake > 0.001 {
            let s = self.shake * self.settings.screen_shake;
            Vec3::new(
                (self.shake_seed * 1.7).sin() * s * 0.06,
                (self.shake_seed * 2.3).cos() * s * 0.06,
                0.0,
            )
        } else {
            Vec3::ZERO
        };

        let fov = self.settings.fov.to_radians();
        let ads = client.local.mv.ads_t;
        let def = client.local.def();
        let fov_scale = 1.0 + (def.ads_fov_scale - 1.0) * ads;

        self.camera = Camera {
            position: eye + shake,
            yaw: self.yaw + self.recoil.yaw_kick,
            pitch: (self.pitch + self.recoil.pitch_kick).clamp(-1.53, 1.53),
            roll: 0.0,
            fov_y: fov * fov_scale,
            near: 0.045,
            far: 500.0,
        };

        let shadows = self.settings.render_settings().shadows;
        world_view::draw_players(&mut self.renderer, client, map, now as f32, shadows);
        world_view::draw_entities(&mut self.renderer, &self.world, now as f32);
        self.effects.submit(&mut self.renderer);

        // The viewmodel is only drawn while actually playing.
        if matches!(self.screen, Screen::InGame) && client.local.alive {
            let speed = client.local.mv.horizontal_speed();
            let weapon = client.local.weapon().id;
            let camera = self.camera;
            self.viewmodel.submit(&mut self.renderer, &camera, weapon, speed, self.settings.view_bob);
        }
        let _ = dt;
    }

    /// Aspect-corrected view-projection for HUD projection maths.
    pub fn camera_view_proj(&self, aspect: f32) -> glam::Mat4 {
        self.camera.view_proj(aspect)
    }

    pub fn clock_fps_low(&self) -> f32 { self.clock.fps_low }

    pub fn local_ready(&self) -> bool {
        self.client.as_ref()
            .and_then(|c| c.player_info(c.slot))
            .map(|p| p.ready)
            .unwrap_or(false)
    }

    pub fn shutdown_and_quit(&mut self) {
        self.shutdown();
        self.quit = true;
    }

    // ------------------------------------------------------------ shutdown

    pub fn shutdown(&mut self) {
        self.finish_match_stats();
        self.stop_networking();
        self.settings.save_if_dirty();
        self.progression.save_if_dirty();
    }
}

use glam::Vec3;

/// Parses `HARDPOINT_SHOTS` as a comma-separated list of `frame:path`.
fn parse_shots() -> Vec<(f64, String)> {
    let Ok(spec) = std::env::var("HARDPOINT_SHOTS") else { return Vec::new() };
    spec.split(',')
        .filter_map(|item| {
            let (f, path) = item.split_once(':')?;
            Some((f.trim().parse().ok()?, path.trim().to_string()))
        })
        .collect()
}

/// Parses `HARDPOINT_VISIT` as a comma-separated list of `seconds:ScreenName`,
/// which drives the capture harness through screens that are otherwise several
/// clicks deep.
fn parse_visits() -> Vec<(f64, Screen)> {
    let Ok(spec) = std::env::var("HARDPOINT_VISIT") else { return Vec::new() };
    spec.split(',')
        .filter_map(|item| {
            let (t, name) = item.split_once(':')?;
            let screen = match name.trim() {
                "MainMenu" => Screen::MainMenu,
                "Multiplayer" => Screen::Multiplayer,
                "Browser" => Screen::Browser,
                "HostSetup" => Screen::HostSetup,
                "DirectConnect" => Screen::DirectConnect,
                "Lobby" => Screen::Lobby,
                "Loadout" => Screen::Loadout,
                "Settings" => Screen::Settings,
                "Controls" => Screen::Controls,
                "Career" => Screen::Career,
                "InGame" => Screen::InGame,
                "Paused" => Screen::Paused,
                "Results" => Screen::Results,
                other => { eprintln!("[visit] unknown screen {}", other); return None; }
            };
            Some((t.trim().parse().ok()?, screen))
        })
        .collect()
}

/// Creates the window and drives the event loop.
pub struct Launcher {
    app: Option<App>,
    settings: Option<Settings>,
    progression: Option<Progression>,
    startup_error: Option<String>,
    connect_to: Option<String>,
}

impl Launcher {
    pub fn new(settings: Settings, progression: Progression, connect_to: Option<String>) -> Launcher {
        Launcher {
            app: None,
            settings: Some(settings),
            progression: Some(progression),
            startup_error: None,
            connect_to,
        }
    }

    pub fn error(&self) -> Option<&str> { self.startup_error.as_deref() }
}

impl ApplicationHandler for Launcher {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.app.is_some() { return; }
        let Some(settings) = self.settings.take() else { return };
        let progression = self.progression.take().unwrap_or_default();

        let size = winit::dpi::PhysicalSize::new(settings.window_width, settings.window_height);
        let attrs = Window::default_attributes()
            .with_title("HARDPOINT: OPERATION IRONVEIL")
            .with_inner_size(size)
            .with_min_inner_size(winit::dpi::PhysicalSize::new(960, 540))
            .with_fullscreen(if settings.fullscreen { Some(Fullscreen::Borderless(None)) } else { None });

        let window = match el.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.startup_error = Some(format!("could not open a window: {e}"));
                el.exit();
                return;
            }
        };

        match App::new(window, settings, progression) {
            Ok(mut app) => {
                if let Some(addr) = self.connect_to.take() {
                    app.connect_address = addr.clone();
                    app.joined_remote = true;
                    app.connect_to(&addr, "");
                }
                self.app = Some(app);
            }
            Err(e) => {
                self.startup_error = Some(e);
                el.exit();
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(app) = self.app.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => {
                app.shutdown();
                el.exit();
            }
            WindowEvent::Resized(size) => {
                app.renderer.resize(size.width, size.height);
                if !app.settings.fullscreen {
                    app.settings.window_width = size.width;
                    app.settings.window_height = size.height;
                    app.settings.mark_dirty();
                }
            }
            WindowEvent::Focused(focused) => {
                if !focused && app.mouse_captured {
                    app.capture_mouse(false);
                    if app.screen == Screen::InGame { app.goto(Screen::Paused); }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    app.input.set_key(code, down);
                    if down {
                        match code {
                            KeyCode::Backspace => app.input.backspace = true,
                            KeyCode::Enter | KeyCode::NumpadEnter => app.input.enter = true,
                            KeyCode::Escape => app.input.escape = true,
                            _ => {}
                        }
                    }
                }
                if down {
                    if let Some(text) = event.text {
                        for c in text.chars() {
                            if !c.is_control() { app.input.typed.push(c); }
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                app.input.set_mouse(button, state == ElementState::Pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                app.input.add_wheel(d);
            }
            WindowEvent::CursorMoved { position, .. } => {
                app.input.cursor = (position.x as f32, position.y as f32);
                app.input.cursor_moved = true;
            }
            WindowEvent::RedrawRequested => {
                app.frame();
                if app.should_quit() {
                    app.shutdown();
                    el.exit();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: winit::event::DeviceId, event: DeviceEvent) {
        let Some(app) = self.app.as_mut() else { return };
        if let DeviceEvent::MouseMotion { delta } = event {
            if app.mouse_captured {
                app.input.add_motion(delta.0 as f32, delta.1 as f32);
            }
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(app) = self.app.as_ref() {
            app.window.request_redraw();
        }
    }
}
