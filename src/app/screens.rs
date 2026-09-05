//! Every screen the player sees.
//!
//! Screens are functions that draw themselves and push `Intent`s describing
//! what the player asked for. The intents are applied after the drawing
//! borrow ends, which keeps each screen a straightforward top-to-bottom
//! description of a page rather than a tangle of callbacks.

use super::{App, HostOptions, Screen};
use crate::game::loadout::{ClassId, Loadout, ALL_CLASSES, ALL_PERKS, LETHAL_EQUIPMENT, TACTICAL_EQUIPMENT};
use crate::game::types::Team;
use crate::game::weapons::{WeaponClass, WeaponId, ALL_WEAPONS};
use crate::input::{Action, Binding, ALL_ACTIONS};
use crate::maps::ALL_MAPS;
use crate::modes::{ModeId, Phase, ALL_MODES};
use crate::progression::{challenges, rank_name};
use crate::ui::draw::Align;
use crate::ui::theme::{self, Color};
use crate::ui::widgets::{Nav, Ui, UiSound};

/// Something the player asked for, applied once the drawing borrow is over.
enum Intent {
    Goto(Screen),
    Push(Screen),
    Back,
    Quit,
    StartHost,
    Connect(String, String),
    RefreshBrowser,
    ApplySettings,
    SendLoadout,
    ToggleReady,
    StartMatch,
    LeaveMatch,
    ChangeTeam(Team),
    Sound(UiSound),
    Rebind(Action),
    ResetBindings,
    QuickPlay,
    Scroll(i32),
}

pub fn draw(app: &mut App, dt: f32, now: f64) {
    let nav = build_nav(app);
    let mut intents: Vec<Intent> = Vec::new();
    let screen = app.screen.clone();
    let focus_in = app.focus;

    // The world-facing screens draw the heads-up display first so menus sit
    // on top of it.
    if matches!(screen, Screen::InGame | Screen::Paused) {
        draw_hud(app, now);
    }

    let new_focus = {
        let App {
            renderer, settings, progression, client, browser, host, hud,
            connect_address, connect_password, loadout, rebinding, scroll,
            loading_message, summary, last_level, ..
        } = app;
        let mut ui = renderer.widgets(nav, focus_in);

        match screen {
            Screen::Splash => { splash(&mut ui); }
            Screen::MainMenu => main_menu(&mut ui, settings, progression, &mut intents),
            Screen::Multiplayer => multiplayer(&mut ui, &mut intents),
            Screen::Browser => browser_screen(&mut ui, browser, settings, *scroll, &mut intents),
            Screen::HostSetup => host_setup(&mut ui, host, settings, &mut intents),
            Screen::DirectConnect => direct_connect(&mut ui, connect_address, connect_password, &mut intents),
            Screen::Connecting => connecting(&mut ui, client.as_ref(), &mut intents),
            Screen::Loading => loading(&mut ui, loading_message),
            Screen::Lobby => lobby(&mut ui, client.as_ref(), host, settings, &mut intents),
            Screen::Loadout => loadout_screen(&mut ui, loadout, progression, &mut intents),
            Screen::Settings => settings_screen(&mut ui, settings, *scroll, &mut intents),
            Screen::Controls => controls_screen(&mut ui, settings, *rebinding, *scroll, &mut intents),
            Screen::Career => career_screen(&mut ui, progression, *scroll, &mut intents),
            Screen::Paused => paused(&mut ui, &mut intents),
            Screen::Results => results(&mut ui, client.as_ref(), summary, progression, *last_level, &mut intents),
            Screen::InGame => {
                if hud.scoreboard_open {
                    if let Some(c) = client.as_ref() {
                        let mode = c.match_info.mode;
                        super::hud::draw_scoreboard(&mut ui.p, c, mode, now, 90.0, 110.0);
                    }
                }
            }
        }

        if let Some(s) = ui.sound { intents.push(Intent::Sound(s)); }
        ui.finish()
    };
    app.focus = new_focus;

    draw_status(app, now);
    if app.show_perf { draw_perf(app, dt); }

    for intent in intents {
        apply(app, intent);
    }
}

fn build_nav(app: &App) -> Nav {
    let b = &app.settings.bindings;
    let typing = app.text_target.is_some();
    let scale = app.renderer.window_size().1 as f32 / theme::DESIGN_HEIGHT;
    Nav {
        up: !typing && (app.input.key_pressed(winit::keyboard::KeyCode::ArrowUp)
            || app.input.key_pressed(winit::keyboard::KeyCode::KeyW)),
        down: !typing && (app.input.key_pressed(winit::keyboard::KeyCode::ArrowDown)
            || app.input.key_pressed(winit::keyboard::KeyCode::KeyS)),
        left: !typing && (app.input.key_pressed(winit::keyboard::KeyCode::ArrowLeft)
            || app.input.key_pressed(winit::keyboard::KeyCode::KeyA)),
        right: !typing && (app.input.key_pressed(winit::keyboard::KeyCode::ArrowRight)
            || app.input.key_pressed(winit::keyboard::KeyCode::KeyD)),
        accept: app.input.enter || (!typing && app.input.key_pressed(winit::keyboard::KeyCode::Space)),
        back: app.input.escape || app.input.pressed(b, Action::Pause),
        page_up: app.input.key_pressed(winit::keyboard::KeyCode::PageUp),
        page_down: app.input.key_pressed(winit::keyboard::KeyCode::PageDown),
        mouse: (app.input.cursor.0 / scale, app.input.cursor.1 / scale),
        mouse_moved: app.input.cursor_moved,
        click: app.input.mouse_pressed(winit::event::MouseButton::Left),
        wheel: app.input.wheel(),
        typed: app.input.typed.clone(),
        backspace: app.input.backspace,
    }
}

fn apply(app: &mut App, intent: Intent) {
    match intent {
        Intent::Goto(s) => app.goto(s),
        Intent::Push(s) => app.push_screen(s),
        Intent::Back => app.back(),
        Intent::Quit => app.shutdown_and_quit(),
        Intent::StartHost => {
            app.settings.server_name = app.host.name.clone();
            app.settings.mark_dirty();
            app.start_host();
        }
        Intent::Connect(addr, pass) => {
            let a = addr.clone();
            app.connect_to(&a, &pass);
        }
        Intent::RefreshBrowser => {
            let now = app.now();
            app.browser.refresh(now);
            for f in app.settings.favourites.clone() {
                app.browser.probe(&f, now);
            }
            let favs = app.settings.favourites.clone();
            app.browser.mark_favourites(&favs);
        }
        Intent::ApplySettings => app.apply_settings(),
        Intent::SendLoadout => {
            let l = app.loadout;
            if let Some(c) = &mut app.client { c.set_loadout(&l); }
        }
        Intent::ToggleReady => {
            let ready = !app.local_ready();
            if let Some(c) = &mut app.client {
                c.send_message(&crate::net::protocol::ClientMsg::Ready { ready });
            }
        }
        Intent::StartMatch => {
            if let Some(c) = &mut app.client {
                c.send_message(&crate::net::protocol::ClientMsg::HostStart);
            }
            app.goto(Screen::InGame);
        }
        Intent::LeaveMatch => app.leave_match(),
        Intent::ChangeTeam(t) => {
            if let Some(c) = &mut app.client {
                c.send_message(&crate::net::protocol::ClientMsg::ChangeTeam { team: t });
            }
        }
        Intent::Sound(s) => app.play_ui(s),
        Intent::Rebind(a) => app.rebinding = Some(a),
        Intent::ResetBindings => {
            app.settings.bindings.reset();
            app.settings.mark_dirty();
        }
        Intent::QuickPlay => {
            app.host = HostOptions {
                name: app.settings.server_name.clone(),
                ..HostOptions::default()
            };
            app.start_host();
        }
        Intent::Scroll(d) => {
            app.scroll = (app.scroll as i32 + d).max(0) as usize;
        }
    }
}

// ================================================================== screens

fn splash(ui: &mut Ui) {
    let w = ui.p.design_width();
    let h = ui.p.design_height();
    ui.p.rect(0.0, 0.0, w, h, theme::BG);
    ui.p.noise_band(0.0, h * 0.53, w, 26.0, 7, theme::ACCENT);
    ui.p.text_aligned(w * 0.5, h * 0.40, theme::H1, theme::TEXT_BRIGHT, "HARDPOINT", Align::Center);
    ui.p.text_aligned(w * 0.5, h * 0.40 + theme::H1 + 8.0, theme::H3, theme::ACCENT, "OPERATION IRONVEIL", Align::Center);
    ui.p.text_aligned(w * 0.5, h * 0.62, theme::SMALL, theme::TEXT_DIM, "GENERATING ASSETS", Align::Center);
}

fn menu_frame(ui: &mut Ui, title: &str, subtitle: &str) -> (f32, f32, f32) {
    let w = ui.p.design_width();
    let h = ui.p.design_height();
    ui.p.rect(0.0, 0.0, w, h, theme::BG);
    // A wide dark band behind the title, with a few noise ticks for texture.
    ui.p.rect(0.0, 60.0, w, 96.0, theme::PANEL_DEEP);
    ui.p.rect(0.0, 60.0, w, 2.0, theme::BORDER_DIM);
    ui.p.rect(0.0, 154.0, w, 2.0, theme::BORDER_DIM);
    ui.p.noise_band(0.0, 60.0, w, 96.0, 3, theme::BORDER);
    let fw = 1240.0f32.min(w - 120.0);
    let x = (w * 0.5 - fw * 0.5).max(60.0);
    ui.p.text(x, 76.0, theme::H2, theme::TEXT_BRIGHT, title);
    ui.p.text(x, 76.0 + theme::H2 + 8.0, theme::SMALL, theme::ACCENT, subtitle);
    (x, 200.0, fw)
}

fn main_menu(ui: &mut Ui, settings: &crate::settings::Settings, prog: &crate::progression::Progression, out: &mut Vec<Intent>) {
    let w = ui.p.design_width();
    let h = ui.p.design_height();
    ui.p.rect(0.0, 0.0, w, h, theme::BG);

    // Title block.
    let tx = w * 0.5 - 460.0;
    ui.p.text(tx, 120.0, theme::H1, theme::TEXT_BRIGHT, "HARDPOINT");
    ui.p.text(tx, 120.0 + theme::H1 + 4.0, theme::H3, theme::ACCENT, "OPERATION IRONVEIL");
    ui.p.rect(tx, 120.0 + theme::H1 + theme::H3 + 18.0, 420.0, 3.0, theme::ACCENT);

    let mut y = 320.0;
    let bw = 540.0;
    if ui.button(tx, y, bw, "QUICK MATCH", "BOTS, INSTANTLY", true) { out.push(Intent::QuickPlay); }
    y += 54.0;
    if ui.button(tx, y, bw, "MULTIPLAYER", "", true) { out.push(Intent::Push(Screen::Multiplayer)); }
    y += 54.0;
    if ui.button(tx, y, bw, "LOADOUT", "", true) { out.push(Intent::Push(Screen::Loadout)); }
    y += 54.0;
    if ui.button(tx, y, bw, "CAREER", "", true) { out.push(Intent::Push(Screen::Career)); }
    y += 54.0;
    if ui.button(tx, y, bw, "SETTINGS", "", true) { out.push(Intent::Push(Screen::Settings)); }
    y += 54.0;
    if ui.button(tx, y, bw, "QUIT", "", true) { out.push(Intent::Quit); }

    // Player card, clear of the menu column's hint text.
    let px = tx + bw + 60.0;
    let py = 320.0;
    ui.p.panel(px, py, 340.0, 200.0);
    ui.p.text(px + 18.0, py + 18.0, theme::SMALL, theme::TEXT_DIM, "OPERATOR");
    ui.p.text(px + 18.0, py + 40.0, theme::H3, theme::TEXT_BRIGHT, &settings.player_name);
    ui.p.text(px + 18.0, py + 78.0, theme::BODY, theme::ACCENT, &format!("RANK {}  {}", prog.level, rank_name(prog.level)));
    let (into, need) = prog.level_progress();
    ui.p.bar(px + 18.0, py + 112.0, 304.0, 10.0, into as f32 / need.max(1) as f32, theme::ACCENT, theme::PANEL_DEEP);
    ui.p.text(px + 18.0, py + 130.0, theme::TINY, theme::TEXT_DIM, &format!("{} / {} XP", into, need));
    ui.p.text(px + 18.0, py + 158.0, theme::SMALL, theme::TEXT_DIM,
              &format!("{} KILLS   {:.2} K/D", prog.career.kills, prog.career.kd()));

    ui.p.text_aligned(w - 24.0, h - 34.0, theme::TINY, theme::TEXT_FAINT,
                      "ARROW KEYS OR MOUSE   ENTER TO SELECT   ESC TO GO BACK", Align::Right);
}

fn multiplayer(ui: &mut Ui, out: &mut Vec<Intent>) {
    let (x, mut y, w) = menu_frame(ui, "MULTIPLAYER", "TWELVE PLAYERS, NO WAITING");
    if ui.button(x, y, w, "HOST A GAME", "CREATE A LOBBY", true) { out.push(Intent::Push(Screen::HostSetup)); }
    y += 54.0;
    if ui.button(x, y, w, "SERVER BROWSER", "FIND GAMES ON THIS NETWORK", true) {
        out.push(Intent::Push(Screen::Browser));
        out.push(Intent::RefreshBrowser);
    }
    y += 54.0;
    if ui.button(x, y, w, "DIRECT CONNECT", "ENTER AN ADDRESS", true) { out.push(Intent::Push(Screen::DirectConnect)); }
    y += 74.0;
    if ui.button(x, y, w, "BACK", "", true) { out.push(Intent::Back); }
    if ui.nav.back { out.push(Intent::Back); }
}

fn browser_screen(
    ui: &mut Ui,
    browser: &crate::net::discovery::Browser,
    settings: &crate::settings::Settings,
    scroll: usize,
    out: &mut Vec<Intent>,
) {
    let (x, y, w) = menu_frame(ui, "SERVER BROWSER", "LOCAL NETWORK AND TRACKED SERVERS");
    let list = browser.sorted();

    // Column headings.
    let cols = [0.0f32, 0.40, 0.56, 0.68, 0.82];
    let heads = ["SERVER", "MAP", "MODE", "PLAYERS", "PING"];
    for (i, hd) in heads.iter().enumerate() {
        ui.p.text(x + w * cols[i], y, theme::SMALL, theme::TEXT_DIM, hd);
    }
    ui.p.text_aligned(x + w - 10.0, y, theme::SMALL, theme::TEXT_DIM, "STATUS", Align::Right);
    ui.p.rule(x, y + 22.0, w);

    let row_h = 32.0;
    let visible = 11usize;
    let start = scroll.min(list.len().saturating_sub(1));
    let mut ry = y + 32.0;

    if list.is_empty() {
        ui.p.text(x, ry + 16.0, theme::BODY, theme::TEXT_DIM,
                  if browser.refreshing { "SEARCHING..." } else { "NO SERVERS FOUND. HOST ONE, OR CONNECT DIRECTLY." });
        ui.skip();
    } else {
        for (i, e) in list.iter().enumerate().skip(start).take(visible) {
            let (_, activated) = ui.list_row(x, ry, w, row_h, i);
            let name_color = if e.favourite { theme::ACCENT } else { theme::TEXT };
            ui.p.text(x + 10.0, ry + 7.0, theme::BODY, name_color, &e.info.name);
            ui.p.text(x + w * cols[1], ry + 7.0, theme::BODY, theme::TEXT_DIM, e.info.map.name());
            ui.p.text(x + w * cols[2], ry + 7.0, theme::BODY, theme::TEXT_DIM, ModeId::from_u8(e.info.mode).short());
            ui.p.text(x + w * cols[3], ry + 7.0, theme::BODY, theme::TEXT, &e.slots_text());
            let ping_color = if e.ping_ms < 60 { theme::GOOD } else if e.ping_ms < 140 { theme::ACCENT } else { theme::WARN };
            ui.p.text(x + w * cols[4], ry + 7.0, theme::BODY, ping_color, &format!("{}", e.ping_ms));
            ui.p.text_aligned(x + w - 10.0, ry + 7.0, theme::SMALL, theme::TEXT_DIM, e.status(), Align::Right);
            if e.info.passworded {
                ui.p.text(x + w * cols[1] - 22.0, ry + 7.0, theme::BODY, theme::WARN, "*");
            }
            if activated {
                out.push(Intent::Connect(e.addr.to_string(), String::new()));
            }
            ry += row_h;
        }
    }

    if ui.nav.wheel > 0.0 { out.push(Intent::Scroll(-1)); }
    if ui.nav.wheel < 0.0 && list.len() > visible { out.push(Intent::Scroll(1)); }

    let by = y + 32.0 + visible as f32 * row_h + 24.0;
    // Sized from the frame: "DIRECT CONNECT" overflowed a fixed 240.
    let bw = (w - 3.0 * 16.0) * 0.25;
    if ui.small_button(x, by, bw, 44.0, "REFRESH", true) { out.push(Intent::RefreshBrowser); }
    if ui.small_button(x + bw + 16.0, by, bw, 44.0, "DIRECT CONNECT", true) { out.push(Intent::Push(Screen::DirectConnect)); }
    if ui.small_button(x + (bw + 16.0) * 2.0, by, bw, 44.0, "HOST", true) { out.push(Intent::Push(Screen::HostSetup)); }
    if ui.small_button(x + (bw + 16.0) * 3.0, by, bw, 44.0, "BACK", true) { out.push(Intent::Back); }
    if ui.nav.back { out.push(Intent::Back); }

    if !settings.favourites.is_empty() {
        ui.p.text(x, by + 56.0, theme::SMALL, theme::TEXT_FAINT,
                  &format!("RECENT: {}", settings.favourites.join("   ")));
    }
}

fn host_setup(ui: &mut Ui, host: &mut HostOptions, settings: &mut crate::settings::Settings, out: &mut Vec<Intent>) {
    let (x, mut y, w) = menu_frame(ui, "HOST A GAME", "YOU WILL BE THE HOST");
    let step = 44.0;

    if ui.text_field(x, y, w, "SERVER NAME", &mut host.name, 28) {
        settings.server_name = host.name.clone();
        settings.mark_dirty();
    }
    y += step;

    let map_index = ALL_MAPS.iter().position(|m| *m == host.map).unwrap_or(0);
    let d = ui.option(x, y, w, "MAP", host.map.name(), true);
    if d != 0 {
        let n = ALL_MAPS.len() as i32;
        let i = (map_index as i32 + d).rem_euclid(n) as usize;
        host.map = ALL_MAPS[i];
    }
    y += step;

    let mode_index = ALL_MODES.iter().position(|m| *m == host.mode).unwrap_or(0);
    let d = ui.option(x, y, w, "GAME MODE", host.mode.name(), true);
    if d != 0 {
        let n = ALL_MODES.len() as i32;
        let i = (mode_index as i32 + d).rem_euclid(n) as usize;
        host.mode = ALL_MODES[i];
        host.score_limit = host.mode.default_score_limit();
        host.time_limit = host.mode.default_time_limit();
    }
    y += step;

    let mut bots = host.bots as f32;
    if ui.slider(x, y, w, "BOTS", &mut bots, 0.0, 15.0, 1.0, "{}") { host.bots = bots as u8; }
    y += step;

    const DIFF: [&str; 4] = ["RECRUIT", "REGULAR", "VETERAN", "ELITE"];
    let d = ui.option(x, y, w, "BOT SKILL", DIFF[host.difficulty.min(3) as usize], true);
    if d != 0 { host.difficulty = ((host.difficulty as i32 + d).rem_euclid(4)) as u8; }
    y += step;

    let mut players = host.max_players as f32;
    if ui.slider(x, y, w, "MAX PLAYERS", &mut players, 2.0, 16.0, 1.0, "{}") { host.max_players = players as u8; }
    y += step;

    let mut score = host.score_limit as f32;
    if ui.slider(x, y, w, "SCORE LIMIT", &mut score, 1.0, 250.0, 1.0, "{}") { host.score_limit = score as u16; }
    y += step;

    let mut time = host.time_limit as f32 / 60.0;
    if ui.slider(x, y, w, "TIME LIMIT", &mut time, 1.0, 30.0, 1.0, "{} MIN") { host.time_limit = (time * 60.0) as u16; }
    y += step;

    ui.toggle(x, y, w, "FRIENDLY FIRE", &mut host.friendly_fire);
    y += step;
    ui.toggle(x, y, w, "ROTATE MAPS", &mut host.rotate_maps);
    y += step;

    ui.text_field(x, y, w, "PASSWORD", &mut host.password, 20);
    y += step + 18.0;

    if ui.button(x, y, w, "START SERVER", "", true) { out.push(Intent::StartHost); }
    y += 54.0;
    if ui.button(x, y, w, "BACK", "", true) { out.push(Intent::Back); }
    if ui.nav.back { out.push(Intent::Back); }

    // Map blurb, so the choice means something.
    let px = x + w + 30.0;
    if px + 300.0 < ui.p.design_width() {
        ui.p.panel(px, 200.0, 300.0, 220.0);
        ui.p.text(px + 16.0, 216.0, theme::SMALL, theme::ACCENT, host.map.theme());
        ui.p.text(px + 16.0, 240.0, theme::TINY, theme::TEXT_DIM, host.map.size_class().label());
        ui.p.text_wrapped(px + 16.0, 268.0, 268.0, theme::SMALL, theme::TEXT, host.map.blurb());
        ui.p.text_wrapped(px + 16.0, 380.0, 268.0, theme::SMALL, theme::TEXT_DIM, host.mode.blurb());
    }
}

fn direct_connect(ui: &mut Ui, address: &mut String, password: &mut String, out: &mut Vec<Intent>) {
    let (x, mut y, w) = menu_frame(ui, "DIRECT CONNECT", "ADDRESS OR ADDRESS:PORT");
    ui.text_field(x, y, w, "ADDRESS", address, 48);
    y += 48.0;
    ui.text_field(x, y, w, "PASSWORD", password, 24);
    y += 68.0;
    let ok = !address.trim().is_empty();
    if ui.button(x, y, w, "CONNECT", if ok { "" } else { "ENTER AN ADDRESS" }, ok) {
        out.push(Intent::Connect(address.clone(), password.clone()));
    }
    y += 54.0;
    if ui.button(x, y, w, "BACK", "", true) { out.push(Intent::Back); }
    if ui.nav.back { out.push(Intent::Back); }
    ui.p.text(x, y + 76.0, theme::SMALL, theme::TEXT_FAINT,
              "EXAMPLES:  192.168.0.20    10.0.0.5:27016    27015");
}

fn connecting(ui: &mut Ui, client: Option<&crate::net::client::Client>, out: &mut Vec<Intent>) {
    let (x, y, w) = menu_frame(ui, "CONNECTING", "");
    let state = client.map(|c| c.state.clone());
    let text = match state {
        Some(crate::net::client::ClientState::Connecting) => "SENDING REQUEST",
        Some(crate::net::client::ClientState::Challenged) => "AUTHENTICATING",
        Some(crate::net::client::ClientState::Joining) => "JOINING",
        Some(crate::net::client::ClientState::Playing) => "CONNECTED",
        _ => "NO CONNECTION",
    };
    ui.p.text(x, y, theme::H3, theme::TEXT, text);
    if let Some(c) = client {
        ui.p.text(x, y + 42.0, theme::BODY, theme::TEXT_DIM, &format!("{}", c.server));
    }
    if ui.button(x, y + 100.0, w, "CANCEL", "", true) {
        out.push(Intent::LeaveMatch);
    }
    if ui.nav.back { out.push(Intent::LeaveMatch); }
}

fn loading(ui: &mut Ui, message: &str) {
    let w = ui.p.design_width();
    let h = ui.p.design_height();
    ui.p.rect(0.0, 0.0, w, h, theme::BG);
    ui.p.text_aligned(w * 0.5, h * 0.46, theme::H2, theme::TEXT_BRIGHT, "LOADING", Align::Center);
    ui.p.text_aligned(w * 0.5, h * 0.46 + theme::H2 + 12.0, theme::BODY, theme::TEXT_DIM, message, Align::Center);
}

fn lobby(
    ui: &mut Ui,
    client: Option<&crate::net::client::Client>,
    host: &mut HostOptions,
    settings: &crate::settings::Settings,
    out: &mut Vec<Intent>,
) {
    let Some(c) = client else { return };
    let mi = &c.match_info;
    let (x, y, w) = menu_frame(ui, &mi.server_name, &format!("{}   {}", mi.map.name(), mi.mode.name()));
    let phase = Phase::from_u8(mi.phase);

    // Left: the player list, grouped by team.
    let list_w = w * 0.56;
    let panel_h = 560.0;
    ui.p.panel(x, y, list_w, panel_h);
    ui.p.text(x + 16.0, y + 14.0, theme::SMALL, theme::TEXT_DIM, "PLAYERS");
    let mut ry = y + 44.0;
    let teams: Vec<Team> = if mi.mode.is_team_game() {
        vec![Team::Phantom, Team::Vanguard]
    } else {
        vec![Team::None]
    };
    for team in teams {
        if mi.mode.is_team_game() {
            let count = c.roster.iter().filter(|r| r.present && r.team == team).count();
            ui.p.text(x + 16.0, ry, theme::BODY, theme::team_color(team), &format!("{}  ({})", team.name(), count));
            ry += 26.0;
        }
        for (slot, r) in c.roster.iter().enumerate() {
            if !r.present { continue; }
            if mi.mode.is_team_game() && r.team != team { continue; }
            let me = slot as u8 == c.slot;
            let color = if me { theme::ACCENT } else { theme::TEXT };
            let label = if r.is_bot { format!("{}  [BOT]", r.name) } else { r.name.clone() };
            ui.p.text_clipped(x + 32.0, ry, list_w - 216.0, theme::BODY, color, &label);
            ui.p.text_aligned(x + list_w - 116.0, ry, theme::SMALL, theme::TEXT_DIM,
                              &format!("LVL {}", r.level), Align::Right);
            if r.ready {
                ui.p.text_aligned(x + list_w - 16.0, ry, theme::SMALL, theme::GOOD, "READY", Align::Right);
            } else if !r.is_bot {
                ui.p.text_aligned(x + list_w - 16.0, ry, theme::SMALL, theme::TEXT_FAINT, "...", Align::Right);
            }
            ry += 24.0;
        }
        ry += 10.0;
    }

    // Right: match details and the map blurb.
    let px = x + list_w + 24.0;
    let pw = w - list_w - 24.0;
    ui.p.panel(px, y, pw, panel_h);
    ui.p.text(px + 16.0, y + 14.0, theme::SMALL, theme::TEXT_DIM, "BRIEFING");
    // Flowed, not placed at fixed offsets: blurb lengths differ per map and
    // per mode, and a fixed layout runs one paragraph into the next.
    let bx = px + 16.0;
    let bw = pw - 32.0;
    let lh = theme::SMALL * 1.42;
    let mut by2 = y + 42.0;
    ui.p.text(bx, by2, theme::H3, theme::TEXT_BRIGHT, mi.map.name());
    by2 += theme::H3 + 6.0;
    ui.p.text(bx, by2, theme::SMALL, theme::ACCENT, mi.map.theme());
    by2 += theme::SMALL + 18.0;
    by2 += ui.p.text_wrapped(bx, by2, bw, theme::SMALL, theme::TEXT, mi.map.blurb()) as f32 * lh + 14.0;
    ui.p.rule(bx, by2, bw);
    by2 += 16.0;
    ui.p.text(bx, by2, theme::BODY, theme::TEXT, mi.mode.name());
    by2 += theme::BODY + 10.0;
    by2 += ui.p.text_wrapped(bx, by2, bw, theme::SMALL, theme::TEXT_DIM, mi.mode.blurb()) as f32 * lh + 20.0;
    for line in [
        format!("SCORE LIMIT {}   TIME {} MIN", mi.score_limit, mi.time_limit / 60),
        format!("BOTS {}   FRIENDLY FIRE {}", mi.bot_count, if mi.friendly_fire { "ON" } else { "OFF" }),
        format!("PING {} MS", c.ping_ms()),
    ] {
        ui.p.text(bx, by2, theme::SMALL, theme::TEXT_DIM, &line);
        by2 += 24.0;
    }

    let by = y + panel_h + 24.0;
    let bw = (w - 48.0) * 0.25;
    let ready = c.player_info(c.slot).map(|p| p.ready).unwrap_or(false);
    if ui.small_button(x, by, bw, 44.0, if ready { "NOT READY" } else { "READY" }, true) {
        out.push(Intent::ToggleReady);
    }
    if ui.small_button(x + bw + 16.0, by, bw, 44.0, "LOADOUT", true) {
        out.push(Intent::Push(Screen::Loadout));
    }
    if mi.mode.is_team_game() {
        if ui.small_button(x + (bw + 16.0) * 2.0, by, bw, 44.0, "SWITCH TEAM", true) {
            let target = if c.my_team() == Team::Phantom { Team::Vanguard } else { Team::Phantom };
            out.push(Intent::ChangeTeam(target));
        }
    } else {
        ui.skip();
    }
    if ui.small_button(x + (bw + 16.0) * 3.0, by, bw, 44.0, "LEAVE", true) {
        out.push(Intent::LeaveMatch);
    }

    // Starting is only the host's call; everyone else waits.
    let is_host = c.slot == 0;
    let sy = by + 60.0;
    if phase == Phase::Lobby || phase == Phase::MatchOver {
        let label = if is_host { "START MATCH" } else { "WAITING FOR HOST" };
        if ui.small_button(x, sy, w, 56.0, label, is_host) {
            out.push(Intent::StartMatch);
        }
    } else {
        ui.skip();
        if ui.small_button(x, sy, w, 56.0, "DEPLOY", true) {
            out.push(Intent::Goto(Screen::InGame));
        }
    }

    let _ = (host, settings);
    if ui.nav.back { out.push(Intent::LeaveMatch); }
}

fn loadout_screen(
    ui: &mut Ui,
    loadout: &mut Loadout,
    prog: &crate::progression::Progression,
    out: &mut Vec<Intent>,
) {
    let (x, mut y, w) = menu_frame(ui, "LOADOUT", "APPLIES ON YOUR NEXT SPAWN");
    let step = 44.0;
    let level = prog.level;

    // Class preset.
    let ci = ALL_CLASSES.iter().position(|c| *c == loadout.class).unwrap_or(0);
    let d = ui.option(x, y, w, "CLASS", loadout.class.name(), true);
    if d != 0 {
        let i = (ci as i32 + d).rem_euclid(ALL_CLASSES.len() as i32) as usize;
        let class = ALL_CLASSES[i];
        if class == ClassId::Custom {
            loadout.class = class;
        } else {
            *loadout = class.preset();
            loadout.sanitize(level);
        }
        out.push(Intent::SendLoadout);
    }
    y += step;
    ui.p.text(x + 18.0, y - 2.0, theme::SMALL, theme::TEXT_DIM, loadout.class.blurb());
    y += 30.0;

    // Primary.
    let primaries: Vec<WeaponId> = ALL_WEAPONS.iter().copied()
        .filter(|wp| wp.def().class.is_primary())
        .collect();
    if cycle_weapon(ui, x, y, w, "PRIMARY", &mut loadout.primary, &primaries, level, prog) {
        loadout.class = ClassId::Custom;
        out.push(Intent::SendLoadout);
    }
    y += step;
    y += weapon_stats(ui, x + 18.0, y, w - 36.0, loadout.primary) + 14.0;

    let secondaries: Vec<WeaponId> = ALL_WEAPONS.iter().copied()
        .filter(|wp| wp.def().class == WeaponClass::Pistol)
        .collect();
    if cycle_weapon(ui, x, y, w, "SIDEARM", &mut loadout.secondary, &secondaries, level, prog) {
        loadout.class = ClassId::Custom;
        out.push(Intent::SendLoadout);
    }
    y += step;

    let melees: Vec<WeaponId> = ALL_WEAPONS.iter().copied()
        .filter(|wp| wp.def().class == WeaponClass::Melee)
        .collect();
    if cycle_weapon(ui, x, y, w, "MELEE", &mut loadout.melee, &melees, level, prog) {
        loadout.class = ClassId::Custom;
        out.push(Intent::SendLoadout);
    }
    y += step;

    let li = LETHAL_EQUIPMENT.iter().position(|e| *e == loadout.lethal).unwrap_or(0);
    let d = ui.option(x, y, w, "LETHAL", loadout.lethal.name(), true);
    if d != 0 {
        loadout.lethal = LETHAL_EQUIPMENT[(li as i32 + d).rem_euclid(3) as usize];
        loadout.class = ClassId::Custom;
        out.push(Intent::SendLoadout);
    }
    y += step;

    let ti = TACTICAL_EQUIPMENT.iter().position(|e| *e == loadout.tactical).unwrap_or(0);
    let d = ui.option(x, y, w, "TACTICAL", loadout.tactical.name(), true);
    if d != 0 {
        loadout.tactical = TACTICAL_EQUIPMENT[(ti as i32 + d).rem_euclid(3) as usize];
        loadout.class = ClassId::Custom;
        out.push(Intent::SendLoadout);
    }
    y += step;

    let pi = ALL_PERKS.iter().position(|p| *p == loadout.perk).unwrap_or(0);
    let d = ui.option(x, y, w, "PERK", loadout.perk.name(), true);
    if d != 0 {
        loadout.perk = ALL_PERKS[(pi as i32 + d).rem_euclid(ALL_PERKS.len() as i32) as usize];
        loadout.class = ClassId::Custom;
        out.push(Intent::SendLoadout);
    }
    y += step;
    ui.p.text(x + 18.0, y - 2.0, theme::SMALL, theme::TEXT_DIM, loadout.perk.blurb());
    y += 40.0;

    if ui.button(x, y, w, "DONE", "", true) { out.push(Intent::Back); }
    if ui.nav.back { out.push(Intent::Back); }
    y += 68.0;

    // Full numbers for the primary. The bars above give a feel; this is what
    // lets someone actually compare two rifles.
    let d = loadout.primary.def();
    let ph = 150.0;
    ui.p.panel(x, y, w, ph);
    ui.p.text(x + 16.0, y + 14.0, theme::SMALL, theme::ACCENT, "SPECIFICATIONS");
    let specs = [
        ("DAMAGE", format!("{:.0} - {:.0}", d.damage, d.damage_far)),
        ("RANGE", format!("{:.0} - {:.0} M", d.range_near, d.range_far)),
        ("RATE OF FIRE", format!("{:.0} RPM", d.rpm)),
        ("FIRE MODE", d.fire_mode.label().to_string()),
        ("MAGAZINE", format!("{} + {}", d.mag, d.reserve)),
        ("RELOAD", format!("{:.2} S / {:.2} S", d.reload_time, d.reload_empty)),
        ("HEADSHOT", format!("x{:.2}", d.headshot_mult)),
        ("PENETRATION", format!("{:.1}", d.penetration)),
        ("MOBILITY", format!("{:.0}%", d.move_scale * 100.0)),
        ("AIM TIME", format!("{:.2} S", d.ads_time)),
    ];
    let cw = (w - 32.0) / 5.0;
    for (i, (label, value)) in specs.iter().enumerate() {
        let cx = x + 16.0 + (i % 5) as f32 * cw;
        let cy = y + 48.0 + (i / 5) as f32 * 48.0;
        ui.p.text(cx, cy, theme::TINY, theme::TEXT_FAINT, label);
        ui.p.text(cx, cy + 18.0, theme::SMALL, theme::TEXT, value);
    }

    // Equipment description panel.
    let px = x + w + 30.0;
    if px + 300.0 < ui.p.design_width() {
        ui.p.panel(px, 200.0, 300.0, 300.0);
        let ex = px + 16.0;
        let ew = 268.0;
        let lh = theme::SMALL * 1.42;
        let mut ey = 216.0;
        ui.p.text(ex, ey, theme::SMALL, theme::ACCENT, "EQUIPMENT");
        ey += 30.0;
        for eq in [loadout.lethal, loadout.tactical] {
            ui.p.text(ex, ey, theme::BODY, theme::TEXT, eq.name());
            ey += theme::BODY + 6.0;
            ey += ui.p.text_wrapped(ex, ey, ew, theme::SMALL, theme::TEXT_DIM, eq.blurb()) as f32 * lh + 16.0;
        }
        ui.p.rule(ex, ey, ew);
        ey += 14.0;
        ui.p.text_wrapped(ex, ey, ew, theme::SMALL, theme::TEXT_FAINT,
                  &format!("RANK {} - {} WEAPONS UNLOCKED", level,
                           ALL_WEAPONS.iter().filter(|wp| prog.is_unlocked(**wp)).count()));
    }
}

fn cycle_weapon(
    ui: &mut Ui,
    x: f32, y: f32, w: f32,
    label: &str,
    current: &mut WeaponId,
    pool: &[WeaponId],
    level: u8,
    prog: &crate::progression::Progression,
) -> bool {
    let idx = pool.iter().position(|p| p == current).unwrap_or(0);
    let unlocked = prog.is_unlocked(*current);
    let name = if unlocked {
        current.name().to_string()
    } else {
        format!("{}  (RANK {})", current.name(), current.def().unlock_level)
    };
    let d = ui.option(x, y, w, label, &name, true);
    if d == 0 { return false; }
    // Step over anything still locked, so cycling only offers real choices.
    let n = pool.len() as i32;
    let mut i = idx as i32;
    for _ in 0..n {
        i = (i + d).rem_euclid(n);
        let candidate = pool[i as usize];
        if candidate.def().unlock_level <= level {
            *current = candidate;
            return true;
        }
    }
    false
}

/// Draws the class label, blurb and stat bars for one weapon. Returns the
/// height used, because blurbs wrap to one or two lines depending on the
/// weapon and the frame width.
fn weapon_stats(ui: &mut Ui, x: f32, y: f32, w: f32, weapon: WeaponId) -> f32 {
    let d = weapon.def();
    ui.p.text(x, y, theme::SMALL, theme::ACCENT, d.class.label());
    let mut cy = y + theme::SMALL + 8.0;
    let lines = ui.p.text_wrapped(x, cy, w, theme::SMALL, theme::TEXT_DIM, d.blurb);
    cy += lines as f32 * theme::SMALL * 1.42 + 12.0;

    // Four bars: the stats that actually decide a fight.
    let bar_w = (w - 40.0) * 0.25;
    let stats = [
        ("DAMAGE", (d.damage / 60.0).min(1.0)),
        ("RATE", (d.rpm / 1100.0).min(1.0)),
        ("RANGE", (d.range_far / 160.0).min(1.0)),
        ("CONTROL", (1.0 - (d.recoil_up / 0.05).min(1.0)).max(0.05)),
    ];
    for (i, (label, v)) in stats.iter().enumerate() {
        let bx = x + i as f32 * (bar_w + 13.0);
        ui.p.text(bx, cy, theme::TINY, theme::TEXT_FAINT, label);
        ui.p.bar(bx, cy + 16.0, bar_w, 7.0, *v, theme::ACCENT, theme::PANEL_DEEP);
    }
    cy + 27.0 - y
}

fn settings_screen(ui: &mut Ui, s: &mut crate::settings::Settings, _scroll: usize, out: &mut Vec<Intent>) {
    let (x, top, w) = menu_frame(ui, "SETTINGS", "");
    let step = 38.0;
    let mut changed = false;

    // Two columns: the full list is thirty-odd rows and will not fit down one
    // side of a 1080-line frame without scrolling, which is worse.
    let col_w = (w - 40.0) * 0.5;
    let right_x = x + col_w + 40.0;
    let mut y = top;

    ui.p.text(x, y, theme::SMALL, theme::ACCENT, "PRESETS");
    y += 26.0;
    let pw = (col_w - 3.0 * 10.0) / 4.0;
    if ui.small_button(x, y, pw, 36.0, "LOW END", true) { s.apply_low_preset(); changed = true; }
    if ui.small_button(x + pw + 10.0, y, pw, 36.0, "BALANCED", true) { s.apply_balanced_preset(); changed = true; }
    if ui.small_button(x + (pw + 10.0) * 2.0, y, pw, 36.0, "HIGH", true) { s.apply_high_preset(); changed = true; }
    if ui.small_button(x + (pw + 10.0) * 3.0, y, pw, 36.0, "AUTHENTIC", true) { s.apply_authentic_preset(); changed = true; }
    y += 52.0;

    ui.p.text(x, y, theme::SMALL, theme::ACCENT, "DISPLAY");
    y += 26.0;
    changed |= ui.toggle(x, y, col_w, "FULLSCREEN", &mut s.fullscreen); y += step;
    changed |= ui.toggle(x, y, col_w, "VERTICAL SYNC", &mut s.vsync); y += step;
    let mut fps = s.fps_limit as f32;
    if ui.slider(x, y, col_w, "FRAME RATE LIMIT", &mut fps, 0.0, 300.0, 10.0, "{}") {
        s.fps_limit = fps as u32; changed = true;
    }
    y += step;
    let mut scale = s.resolution_scale * 100.0;
    if ui.slider(x, y, col_w, "RENDER RESOLUTION", &mut scale, 35.0, 100.0, 5.0, "{}%") {
        s.resolution_scale = scale / 100.0; changed = true;
    }
    y += step;
    let d = ui.option(x, y, col_w, "TEXTURE QUALITY", s.texture_quality.label(), true);
    if d != 0 { s.texture_quality = s.texture_quality.next(); changed = true; }
    y += step;
    let d = ui.option(x, y, col_w, "SHADOW QUALITY", s.shadow_quality.label(), true);
    if d != 0 { s.shadow_quality = s.shadow_quality.next(); changed = true; }
    y += step;
    let d = ui.option(x, y, col_w, "EFFECTS QUALITY", s.effects_quality.label(), true);
    if d != 0 { s.effects_quality = s.effects_quality.next(); changed = true; }
    y += step;
    let mut vd = s.view_distance * 100.0;
    if ui.slider(x, y, col_w, "VIEW DISTANCE", &mut vd, 40.0, 160.0, 5.0, "{}%") {
        s.view_distance = vd / 100.0; changed = true;
    }
    y += step;
    changed |= ui.toggle(x, y, col_w, "ANTI-ALIASING", &mut s.antialiasing); y += step;
    changed |= ui.toggle(x, y, col_w, "POST PROCESSING", &mut s.post_processing); y += step;
    if ui.slider(x, y, col_w, "FIELD OF VIEW", &mut s.fov, 65.0, 120.0, 1.0, "{}") { changed = true; }
    y += step + 16.0;

    ui.p.text(x, y, theme::SMALL, theme::ACCENT, "PERIOD LOOK");
    y += 26.0;
    if ui.slider(x, y, col_w, "VERTEX SNAPPING", &mut s.vertex_snap, 0.0, 400.0, 10.0, "{}") { changed = true; }
    y += step;
    if ui.slider(x, y, col_w, "AFFINE TEXTURING", &mut s.affine_texturing, 0.0, 1.0, 0.05, "{}") { changed = true; }
    y += step;
    if ui.slider(x, y, col_w, "SCANLINES", &mut s.scanlines, 0.0, 1.0, 0.05, "{}") { changed = true; }
    y += step;
    if ui.slider(x, y, col_w, "VIGNETTE", &mut s.vignette, 0.0, 1.0, 0.05, "{}") { changed = true; }

    // -- right column ------------------------------------------------------
    let mut y = top;
    ui.p.text(right_x, y, theme::SMALL, theme::ACCENT, "AUDIO");
    y += 26.0;
    if ui.slider(right_x, y, col_w, "MASTER VOLUME", &mut s.master_volume, 0.0, 1.0, 0.05, "{}") { changed = true; }
    y += step;
    if ui.slider(right_x, y, col_w, "EFFECTS", &mut s.sfx_volume, 0.0, 1.0, 0.05, "{}") { changed = true; }
    y += step;
    if ui.slider(right_x, y, col_w, "MUSIC", &mut s.music_volume, 0.0, 1.0, 0.05, "{}") { changed = true; }
    y += step;
    if ui.slider(right_x, y, col_w, "ANNOUNCER", &mut s.voice_volume, 0.0, 1.0, 0.05, "{}") { changed = true; }
    y += step + 16.0;

    ui.p.text(right_x, y, theme::SMALL, theme::ACCENT, "GAMEPLAY");
    y += 26.0;
    if ui.text_field(right_x, y, col_w, "PLAYER NAME", &mut s.player_name, 20) { changed = true; }
    y += step;
    if ui.slider(right_x, y, col_w, "MOUSE SENSITIVITY", &mut s.sensitivity, 0.2, 12.0, 0.1, "{}") { changed = true; }
    y += step;
    if ui.slider(right_x, y, col_w, "AIM SENSITIVITY", &mut s.ads_sensitivity, 0.2, 1.5, 0.05, "{}") { changed = true; }
    y += step;
    changed |= ui.toggle(right_x, y, col_w, "INVERT LOOK", &mut s.invert_y); y += step;
    changed |= ui.toggle(right_x, y, col_w, "TOGGLE CROUCH", &mut s.toggle_crouch); y += step;
    changed |= ui.toggle(right_x, y, col_w, "TOGGLE AIM", &mut s.toggle_ads); y += step;
    changed |= ui.toggle(right_x, y, col_w, "AUTOMATIC SPRINT", &mut s.auto_sprint); y += step;
    let mut ch = s.crosshair as f32;
    if ui.slider(right_x, y, col_w, "CROSSHAIR", &mut ch, 0.0, 3.0, 1.0, "{}") { s.crosshair = ch as u8; changed = true; }
    y += step;
    if ui.slider(right_x, y, col_w, "VIEW BOB", &mut s.view_bob, 0.0, 1.5, 0.05, "{}") { changed = true; }
    y += step;
    if ui.slider(right_x, y, col_w, "SCREEN SHAKE", &mut s.screen_shake, 0.0, 1.5, 0.05, "{}") { changed = true; }
    y += step;
    changed |= ui.toggle(right_x, y, col_w, "SHOW FRAME RATE", &mut s.show_fps); y += step;
    changed |= ui.toggle(right_x, y, col_w, "DAMAGE NUMBERS", &mut s.show_damage_numbers); y += step + 20.0;

    let bw = (col_w - 14.0) * 0.5;
    if ui.small_button(right_x, y, bw, 48.0, "CONTROLS", true) { out.push(Intent::Push(Screen::Controls)); }
    if ui.small_button(right_x + bw + 14.0, y, bw, 48.0, "BACK", true) { out.push(Intent::Back); }
    if ui.nav.back { out.push(Intent::Back); }

    if changed {
        s.mark_dirty();
        out.push(Intent::ApplySettings);
    }
}

fn controls_screen(
    ui: &mut Ui,
    s: &mut crate::settings::Settings,
    rebinding: Option<Action>,
    scroll: usize,
    out: &mut Vec<Intent>,
) {
    let (x, y, w) = menu_frame(ui, "CONTROLS", "SELECT AN ACTION AND PRESS A KEY");
    let step = 32.0;

    // Two columns so all twenty-six actions are on screen at once; a rebinding
    // screen you have to scroll is a rebinding screen people get lost in.
    let col_w = (w - 40.0) * 0.5;
    let per_col = ALL_ACTIONS.len().div_ceil(2);

    for (i, a) in ALL_ACTIONS.iter().enumerate() {
        let (cx, row) = if i < per_col { (x, i) } else { (x + col_w + 40.0, i - per_col) };
        let ry = y + row as f32 * step;
        let (_, activated) = ui.list_row(cx, ry, col_w, step, i);
        ui.p.text(cx + 12.0, ry + 6.0, theme::BODY, theme::TEXT, a.label());
        let binding = s.bindings.get(*a);
        let waiting = rebinding == Some(*a);
        let text = if waiting { "PRESS A KEY...".to_string() } else { binding.name() };
        let color = if waiting {
            theme::ACCENT
        } else if binding == Binding::None {
            theme::BAD
        } else {
            theme::TEXT_DIM
        };
        ui.p.text_aligned(cx + col_w - 12.0, ry + 6.0, theme::BODY, color, &text, Align::Right);
        if activated && rebinding.is_none() {
            out.push(Intent::Rebind(*a));
        }
    }

    let by = y + per_col as f32 * step + 34.0;
    let bw = (w - 20.0) * 0.5;
    if ui.small_button(x, by, bw, 46.0, "RESET TO DEFAULTS", true) { out.push(Intent::ResetBindings); }
    if ui.small_button(x + bw + 20.0, by, bw, 46.0, "BACK", true) { out.push(Intent::Back); }
    if ui.nav.back && rebinding.is_none() { out.push(Intent::Back); }
    let _ = scroll;
    let _ = Binding::None;
}

fn career_screen(ui: &mut Ui, prog: &crate::progression::Progression, scroll: usize, out: &mut Vec<Intent>) {
    let (x, y, w) = menu_frame(ui, "CAREER", rank_name(prog.level));

    // Rank and experience.
    ui.p.panel(x, y, w, 130.0);
    ui.p.text(x + 18.0, y + 16.0, theme::H2, theme::TEXT_BRIGHT, &format!("RANK {}", prog.level));
    ui.p.text(x + 18.0, y + 16.0 + theme::H2 + 4.0, theme::BODY, theme::ACCENT, rank_name(prog.level));
    let (into, need) = prog.level_progress();
    ui.p.bar(x + 18.0, y + 96.0, w - 36.0, 12.0, into as f32 / need.max(1) as f32, theme::ACCENT, theme::PANEL_DEEP);
    ui.p.text_aligned(x + w - 18.0, y + 20.0, theme::BODY, theme::TEXT_DIM,
                      &format!("{} XP TOTAL", prog.xp), Align::Right);

    // Career statistics in two columns.
    let sy = y + 150.0;
    ui.p.panel(x, sy, w * 0.48, 250.0);
    let c = &prog.career;
    let stats: [(&str, String); 9] = [
        ("MATCHES", c.matches.to_string()),
        ("WINS", format!("{} ({:.0}%)", c.wins, c.win_rate() * 100.0)),
        ("KILLS", c.kills.to_string()),
        ("DEATHS", c.deaths.to_string()),
        ("K/D", format!("{:.2}", c.kd())),
        ("ASSISTS", c.assists.to_string()),
        ("HEADSHOTS", c.headshots.to_string()),
        ("ACCURACY", format!("{:.1}%", c.accuracy() * 100.0)),
        ("BEST STREAK", c.best_streak.to_string()),
    ];
    for (i, (label, value)) in stats.iter().enumerate() {
        let ry = sy + 20.0 + i as f32 * 25.0;
        ui.p.text(x + 18.0, ry, theme::BODY, theme::TEXT_DIM, label);
        ui.p.text_aligned(x + w * 0.48 - 18.0, ry, theme::BODY, theme::TEXT_BRIGHT, value, Align::Right);
    }

    // Challenges.
    let cx = x + w * 0.5;
    let cw = w * 0.5;
    ui.p.panel(cx, sy, cw, 250.0);
    ui.p.text(cx + 18.0, sy + 14.0, theme::SMALL, theme::ACCENT, "CHALLENGES");
    let list = challenges();
    let visible = 7usize;
    let start = scroll.min(list.len().saturating_sub(1));
    for (i, ch) in list.iter().enumerate().skip(start).take(visible) {
        let ry = sy + 42.0 + (i - start) as f32 * 28.0;
        let done = prog.challenge_done.get(i).copied().unwrap_or(false);
        let progress = prog.challenge_progress.get(i).copied().unwrap_or(0);
        let color = if done { theme::GOOD } else { theme::TEXT };
        ui.p.text(cx + 18.0, ry, theme::SMALL, color, ch.name);
        ui.p.text_aligned(cx + cw - 18.0, ry, theme::SMALL, theme::TEXT_DIM,
                          &format!("{}/{}", progress, ch.target), Align::Right);
        ui.p.bar(cx + 18.0, ry + 16.0, cw - 36.0, 4.0,
                 progress as f32 / ch.target.max(1) as f32,
                 if done { theme::GOOD } else { theme::ACCENT }, theme::PANEL_DEEP);
    }
    if ui.nav.wheel > 0.0 { out.push(Intent::Scroll(-1)); }
    if ui.nav.wheel < 0.0 && list.len() > visible { out.push(Intent::Scroll(1)); }

    // Weapon usage.
    let wy = sy + 270.0;
    ui.p.panel(x, wy, w, 190.0);
    ui.p.text(x + 18.0, wy + 14.0, theme::SMALL, theme::ACCENT, "MOST USED WEAPONS");
    let mut used: Vec<(WeaponId, u32, u32, u32)> = ALL_WEAPONS.iter()
        .map(|wp| (*wp, prog.weapon_kills[wp.index()], prog.weapon_shots[wp.index()], prog.weapon_hits[wp.index()]))
        .filter(|(_, k, s, _)| *k > 0 || *s > 0)
        .collect();
    used.sort_by(|a, b| b.1.cmp(&a.1));
    for (i, (wp, kills, shots, hits)) in used.iter().take(5).enumerate() {
        let ry = wy + 42.0 + i as f32 * 26.0;
        ui.p.text(x + 18.0, ry, theme::BODY, theme::TEXT, wp.name());
        ui.p.text_aligned(x + w * 0.6, ry, theme::BODY, theme::TEXT_DIM, &format!("{} KILLS", kills), Align::Right);
        let acc = if *shots > 0 { *hits as f32 / *shots as f32 * 100.0 } else { 0.0 };
        ui.p.text_aligned(x + w - 18.0, ry, theme::BODY, theme::TEXT_DIM, &format!("{:.0}% ACC", acc), Align::Right);
    }
    if used.is_empty() {
        ui.p.text(x + 18.0, wy + 42.0, theme::BODY, theme::TEXT_FAINT, "NO COMBAT RECORDED YET");
    }

    if ui.button(x, wy + 210.0, w, "BACK", "", true) { out.push(Intent::Back); }
    if ui.nav.back { out.push(Intent::Back); }
}

fn paused(ui: &mut Ui, out: &mut Vec<Intent>) {
    let w = ui.p.design_width();
    ui.p.dim(0.65);
    let x = w * 0.5 - 260.0;
    let mut y = 260.0;
    ui.p.panel(x - 24.0, y - 40.0, 568.0, 360.0);
    ui.p.text(x, y - 24.0, theme::H2, theme::TEXT_BRIGHT, "PAUSED");
    y += 40.0;
    if ui.button(x, y, 520.0, "RESUME", "", true) { out.push(Intent::Goto(Screen::InGame)); }
    y += 54.0;
    if ui.button(x, y, 520.0, "LOADOUT", "", true) { out.push(Intent::Push(Screen::Loadout)); }
    y += 54.0;
    if ui.button(x, y, 520.0, "SETTINGS", "", true) { out.push(Intent::Push(Screen::Settings)); }
    y += 54.0;
    if ui.button(x, y, 520.0, "LEAVE MATCH", "", true) { out.push(Intent::LeaveMatch); }
    if ui.nav.back { out.push(Intent::Goto(Screen::InGame)); }
}

fn results(
    ui: &mut Ui,
    client: Option<&crate::net::client::Client>,
    summary: &crate::progression::MatchSummary,
    prog: &crate::progression::Progression,
    last_level: u8,
    out: &mut Vec<Intent>,
) {
    let w = ui.p.design_width();
    let h = ui.p.design_height();
    ui.p.dim(0.82);
    let Some(c) = client else { return };
    let mi = &c.match_info;

    let won = summary.won;
    let title = if mi.winner == Team::None && !won { "MATCH OVER" } else if won { "VICTORY" } else { "DEFEAT" };
    let color = if won { theme::GOOD } else if mi.winner == Team::None { theme::TEXT } else { theme::BAD };
    ui.p.text_aligned(w * 0.5, 90.0, theme::H1, color, title, Align::Center);
    if mi.mode.is_team_game() {
        ui.p.text_aligned(w * 0.5, 90.0 + theme::H1 + 8.0, theme::H3, theme::TEXT_DIM,
                          &format!("{}  {} - {}  {}", Team::Phantom.name(), mi.team_scores[0],
                                   mi.team_scores[1], Team::Vanguard.name()), Align::Center);
    }

    // The scoreboard, then the player's own tally.
    super::hud::draw_scoreboard(&mut ui.p, c, mi.mode, 0.0, 210.0, 250.0);

    let px = w * 0.5 - 300.0;
    let py = h - 220.0;
    ui.p.panel(px, py, 600.0, 130.0);
    ui.p.text(px + 18.0, py + 14.0, theme::SMALL, theme::ACCENT, "YOUR MATCH");
    let cells = [
        ("KILLS", summary.kills.to_string()),
        ("DEATHS", summary.deaths.to_string()),
        ("ASSISTS", summary.assists.to_string()),
        ("SCORE", summary.score.to_string()),
    ];
    for (i, (label, value)) in cells.iter().enumerate() {
        let cx = px + 24.0 + i as f32 * 142.0;
        ui.p.text(cx, py + 44.0, theme::TINY, theme::TEXT_DIM, label);
        ui.p.text(cx, py + 62.0, theme::H3, theme::TEXT_BRIGHT, value);
    }
    if prog.level > last_level {
        ui.p.text_aligned(px + 582.0, py + 98.0, theme::BODY, theme::ACCENT,
                          &format!("PROMOTED TO RANK {}", prog.level), Align::Right);
    } else {
        let (into, need) = prog.level_progress();
        ui.p.text_aligned(px + 582.0, py + 98.0, theme::SMALL, theme::TEXT_DIM,
                          &format!("{} / {} XP TO RANK {}", into, need, prog.level + 1), Align::Right);
    }

    let by = h - 74.0;
    if ui.small_button(w * 0.5 - 310.0, by, 300.0, 46.0, "RETURN TO LOBBY", true) {
        out.push(Intent::Goto(Screen::Lobby));
    }
    if ui.small_button(w * 0.5 + 10.0, by, 300.0, 46.0, "LEAVE MATCH", true) {
        out.push(Intent::LeaveMatch);
    }
    if ui.nav.back { out.push(Intent::Goto(Screen::Lobby)); }
}

// ============================================================== in-game HUD

fn draw_hud(app: &mut App, now: f64) {
    let Some(client) = &app.client else { return };
    let Some(map) = &app.map else { return };
    let mi = &client.match_info;

    // Objective markers the mode wants shown.
    let mut objectives: Vec<(glam::Vec3, &str, Team, bool)> = Vec::new();
    match mi.mode {
        ModeId::Domination => {
            for (i, o) in map.domination.iter().enumerate().take(3) {
                let owner = Team::from_u8(mi.hud[i] & 0x7F);
                let contested = mi.hud[i] & 0x80 != 0;
                objectives.push((o.pos, o.label, owner, contested));
            }
        }
        ModeId::SearchDestroy => {
            let planted = mi.hud[0] == 1;
            for (i, o) in map.bomb_sites.iter().enumerate().take(2) {
                let active = !planted || mi.hud[1] == i as u8;
                if active {
                    objectives.push((o.pos, o.label, Team::None, planted));
                }
            }
        }
        _ => {}
    }

    let alive = client.local.alive;
    let respawn = client.snapshots_respawn_in();
    let def = client.local.weapon().def();
    let spread = client.local.spread();
    let lethal_name = client.local.loadout.lethal.name();
    let tactical_name = client.local.loadout.tactical.name();
    let reloading = client.local.action == crate::game::player::Action::Reloading;

    let bomb_progress = if mi.mode == ModeId::SearchDestroy { mi.hud[4] as f32 / 255.0 } else { 0.0 };
    let bomb_label = if mi.hud[0] == 1 { "DEFUSING" } else { "PLANTING" };

    let aspect = {
        let (w, h) = app.renderer.internal_size();
        w as f32 / h.max(1) as f32
    };
    let view_proj = app.camera_view_proj(aspect);

    let frame = super::hud::HudFrame {
        client,
        hud: &app.hud,
        now,
        view_proj,
        spread,
        crosshair_style: app.settings.crosshair,
        show_damage_numbers: app.settings.show_damage_numbers,
        scale: app.settings.hud_scale,
        objectives: &objectives,
        alive,
        respawn_in: respawn,
        health: client.local.health,
        armor: client.local.armor,
        ammo: client.local.weapon().ammo,
        reserve: client.local.weapon().reserve,
        weapon: client.local.weapon().id,
        lethal: client.local.lethal_count,
        tactical: client.local.tactical_count,
        lethal_name,
        tactical_name,
        reloading,
        bomb_progress,
        bomb_label,
    };

    let mut p = app.renderer.painter();
    super::hud::draw(&mut p, &frame);
    let _ = def;
}

// ================================================================== overlays

fn draw_status(app: &mut App, now: f64) {
    let Some((text, t)) = app.status.clone() else { return };
    let age = (now - t) as f32;
    let alpha = if age > 3.4 { ((4.0 - age) / 0.6).clamp(0.0, 1.0) } else { 1.0 };
    let mut p = app.renderer.painter();
    let w = p.design_width();
    let h = p.design_height();
    let tw = p.measure(&text, theme::BODY) + 40.0;
    p.rect(w * 0.5 - tw * 0.5, h - 96.0, tw, 34.0, theme::with_alpha(theme::PANEL_DEEP, alpha * 0.9));
    p.rect(w * 0.5 - tw * 0.5, h - 96.0, 3.0, 34.0, theme::with_alpha(theme::ACCENT, alpha));
    p.text_aligned(w * 0.5, h - 88.0, theme::BODY, theme::with_alpha(theme::TEXT_BRIGHT, alpha), &text, Align::Center);
}

fn draw_perf(app: &mut App, _dt: f32) {
    let stats = app.renderer.stats;
    let (iw, ih) = app.renderer.internal_size();
    let (ww, wh) = app.renderer.window_size();
    let fps = app.fps();
    let low = app.clock_fps_low();
    let ping = app.client.as_ref().map(|c| c.ping_ms()).unwrap_or(0);
    let loss = app.client.as_ref().map(|c| c.loss()).unwrap_or(0.0);
    let corrections = app.client.as_ref().map(|c| c.corrections).unwrap_or(0);
    let particles = app.effects.particle_count();
    let tex_mb = app.renderer.texture_memory() as f32 / (1024.0 * 1024.0);
    let scene_mb = app.renderer.scene_memory() as f32 / (1024.0 * 1024.0);
    let adapter = app.renderer.gpu.adapter_name.clone();
    let backend = app.renderer.gpu.backend.clone();

    let lines = [
        format!("{:.0} FPS   1% LOW {:.0}", fps, low),
        format!("{}x{} -> {}x{}", iw, ih, ww, wh),
        format!("DRAWS {}   TRIS {}", stats.draw_calls, stats.triangles),
        format!("CLUSTERS {}/{}", stats.clusters_drawn, stats.clusters_total),
        format!("SPRITES {}   PARTICLES {}", stats.sprites, particles),
        format!("PING {} MS   LOSS {:.1}%   FIXES {}", ping, loss * 100.0, corrections),
        format!("TEX {:.1} MB   TARGETS {:.1} MB", tex_mb, scene_mb),
        format!("{} ({})", adapter, backend),
    ];

    let mut p = app.renderer.painter();
    let x = 16.0;
    let mut y = 16.0;
    let w = 380.0;
    p.rect(x - 8.0, y - 8.0, w, lines.len() as f32 * 20.0 + 16.0, [0.0, 0.0, 0.0, 0.55]);
    for l in lines.iter() {
        p.text(x, y, theme::SMALL, theme::TEXT, l);
        y += 20.0;
    }
}

// ---------------------------------------------------------------- helpers

/// Colour used by the results screen for a team badge.
pub fn team_badge(team: Team) -> Color { theme::team_color(team) }
