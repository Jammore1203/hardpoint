//! Developer diagnostics.
//!
//! These are not shipped UI; they are the tools that keep the map library
//! honest. `--audit` runs every map through validation, and `--nav` prints a
//! slice of the navigation graph as text, which is how connectivity bugs get
//! found without a level editor.

pub mod png;

use crate::maps::{MapId, ALL_MAPS};
use glam::Vec3;

pub fn audit() -> i32 {
    let mut total = 0usize;
    let start = std::time::Instant::now();
    println!("{:<11} {:>8} {:>7} {:>7} {:>6}  {}", "MAP", "BRUSHES", "DECOR", "NAV", "BUILD", "ISSUES");
    for id in ALL_MAPS {
        let t = std::time::Instant::now();
        let m = id.build();
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        let issues = m.validate();
        total += issues.len();
        let repair = if m.repairs == (0, 0) { String::new() }
                     else { format!("  [repaired {} spawn(s), {} pickup(s)]", m.repairs.0, m.repairs.1) };
        println!("{:<11} {:>8} {:>7} {:>7} {:>5.1}ms  {}{}",
                 m.name(), m.brushes.len(), m.decor.len(), m.nav.walkable_count(), ms,
                 if issues.is_empty() { "clean".to_string() } else { format!("{} PROBLEM(S)", issues.len()) }, repair);
        for i in &issues { println!("      - {}", i); }
    }
    println!("\n{} issue(s) across {} maps in {:.1} ms",
             total, ALL_MAPS.len(), start.elapsed().as_secs_f32() * 1000.0);
    if total == 0 { 0 } else { 1 }
}

/// Prints a horizontal slice of the navigation graph. Each cell shows the
/// connected-component letter of the node nearest the requested height, so
/// disconnected pockets are immediately visible.
pub fn nav_dump(name: &str, height: f32) -> i32 {
    let id = match ALL_MAPS.iter().find(|m| m.name().eq_ignore_ascii_case(name)) {
        Some(i) => *i,
        None => {
            eprintln!("unknown map '{}'. known: {}", name,
                      ALL_MAPS.iter().map(|m| m.name()).collect::<Vec<_>>().join(", "));
            return 1;
        }
    };
    let m: crate::maps::MapData = id.build();
    let (comp, sizes) = m.nav.components();
    let mut ranked: Vec<(usize, u32)> = sizes.iter().copied().enumerate().collect();
    ranked.sort_unstable_by_key(|(_, s)| std::cmp::Reverse(*s));
    println!("{} @ y={:.1}   nodes={} components={}", m.name(), height, m.nav.walkable_count(), sizes.len());
    print!("  sizes:");
    for (i, s) in ranked.iter().take(10) { print!(" {}={}", label(*i as u32), s); }
    println!();

    // Bucket nodes into cells, keeping whichever is closest to the slice.
    use std::collections::HashMap;
    let mut cells: HashMap<(i32, i32), (f32, u32)> = HashMap::new();
    for (i, n) in m.nav.nodes.iter().enumerate() {
        let key = (n.pos.x.floor() as i32, n.pos.z.floor() as i32);
        let d = (n.pos.y - height).abs();
        let e = cells.entry(key).or_insert((f32::MAX, 0));
        if d < e.0 { *e = (d, i as u32); }
    }

    let x0 = m.bounds.min.x.floor() as i32;
    let x1 = m.bounds.max.x.ceil() as i32;
    let z0 = m.bounds.min.z.floor() as i32;
    let z1 = m.bounds.max.z.ceil() as i32;
    for cz in z0..z1 {
        let mut line = String::with_capacity((x1 - x0) as usize);
        for cx in x0..x1 {
            match cells.get(&(cx, cz)) {
                Some((d, i)) if *d < 1.2 => line.push(label(comp[*i as usize])),
                Some(_) => line.push(':'),
                None => line.push('.'),
            }
        }
        println!("{:>5} {}", cz, line);
    }
    println!("  '.' = no navigation in column, ':' = navigation only at another height");

    // Where the gameplay markers land, so stranded objectives are obvious.
    for o in m.domination.iter().chain(m.bomb_sites.iter()) {
        report_marker(&m, &comp, o.label, o.pos);
    }
    for (i, p) in m.pickups.iter().enumerate() {
        report_marker(&m, &comp, Box::leak(format!("pickup{}", i).into_boxed_str()), p.pos);
    }
    0
}

fn report_marker(m: &crate::maps::MapData, comp: &[u32], label_text: &str, p: Vec3) {
    match m.nav.nearest(p) {
        Some(n) => {
            let np = m.nav.node(n).pos;
            println!("  {:<9} {:>7.1},{:>6.1},{:>7.1} -> node {:?} comp {} dist {:.2}",
                     label_text, p.x, p.y, p.z, np, label(comp[n as usize]), (np - p).length());
        }
        None => println!("  {:<9} {:>7.1},{:>6.1},{:>7.1} -> NO NAVIGATION", label_text, p.x, p.y, p.z),
    }
}

/// Explains, for one world column, exactly what the navigation bake saw:
/// every candidate surface, and for each one whether an agent could stand
/// there. This is the tool of last resort when a region is mysteriously empty.
pub fn probe(name: &str, x: f32, z: f32) -> i32 {
    use crate::maps::brush::BrushFlags;
    use crate::maps::nav::{GROUND_CLEARANCE, STEP_ALLOWANCE};
    let id = match ALL_MAPS.iter().find(|m| m.name().eq_ignore_ascii_case(name)) {
        Some(i) => *i,
        None => { eprintln!("unknown map '{}'", name); return 1; }
    };
    let m = id.build();
    let w = &m.collision;
    println!("{} column at x={:.2} z={:.2}", m.name(), x, z);
    let column = crate::math::Aabb::new(
        Vec3::new(x - 0.05, m.bounds.min.y, z - 0.05),
        Vec3::new(x + 0.05, m.bounds.max.y, z + 0.05));
    let mut hits: Vec<(f32, u32)> = Vec::new();
    w.grid.query_aabb(&column, |bi| {
        let b = &w.brushes[bi as usize];
        if x < b.aabb.min.x || x > b.aabb.max.x || z < b.aabb.min.z || z > b.aabb.max.z { return; }
        if hits.iter().any(|(_, i)| *i == bi) { return; }
        hits.push((b.surface_height(x, z), bi));
    });
    hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    for (h, bi) in &hits {
        let b = &w.brushes[*bi as usize];
        println!("  brush {:4} top={:7.3} y=[{:7.3},{:7.3}] solid={} nonav={} nodraw={} mat={:?}",
                 bi, h, b.aabb.min.y, b.aabb.max.y, b.is_solid(),
                 b.flags.contains(BrushFlags::NONAV), b.flags.contains(BrushFlags::NODRAW), b.mat);
    }
    println!("  -- standability --");
    for (h, _) in &hits {
        let feet = Vec3::new(x, h + GROUND_CLEARANCE, z);
        let ok = w.standable(feet, 0.36, 1.80, STEP_ALLOWANCE);
        // Name the first obstruction so the reason is actionable.
        let mut blockers: Vec<u32> = Vec::new();
        if !ok {
            let body = crate::math::Aabb::from_base(feet, 0.36, 1.80);
            w.grid.query_aabb(&body, |bi| {
                let b = &w.brushes[bi as usize];
                if !b.is_solid() || matches!(b.kind, crate::maps::brush::BrushKind::Ramp(_)) { return; }
                if b.aabb.max.y <= feet.y + STEP_ALLOWANCE { return; }
                if b.aabb.overlaps(&body) && !blockers.contains(&bi) { blockers.push(bi); }
            });
        }
        if blockers.is_empty() {
            println!("  stand at {:7.3} (feet {:.3}, step ceiling {:.3}): {}",
                     h, feet.y, feet.y + STEP_ALLOWANCE, if ok { "OK" } else { "BLOCKED but no blocker found" });
        } else {
            println!("  stand at {:7.3} (feet {:.3}, step ceiling {:.3}): blocked by {} brush(es)",
                     h, feet.y, feet.y + STEP_ALLOWANCE, blockers.len());
            for bi in blockers.iter().take(4) {
                let b = &w.brushes[*bi as usize];
                println!("        brush {:4} {:?} x=[{:.2},{:.2}] y=[{:.2},{:.2}] z=[{:.2},{:.2}]",
                         bi, b.mat, b.aabb.min.x, b.aabb.max.x, b.aabb.min.y, b.aabb.max.y, b.aabb.min.z, b.aabb.max.z);
            }
        }
    }
    let (comp, sizes) = m.nav.components();
    println!("  -- navigation in this column --");
    let mut found = 0;
    for (i, n) in m.nav.nodes.iter().enumerate() {
        if (n.pos.x - x).abs() < 1.0 && (n.pos.z - z).abs() < 1.0 {
            println!("     node {:5} at {:?} links {} component {} (size {})",
                     i, n.pos, m.nav.links_of(i as u32).len(), label(comp[i]), sizes[comp[i] as usize]);
            found += 1;
        }
    }
    if found == 0 { println!("     none within a metre"); }
    0
}

fn label(c: u32) -> char {
    const ALPHABET: &[u8] = b"#ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    ALPHABET[(c as usize) % ALPHABET.len()] as char
}

/// Prints one map's build statistics; used when tuning geometry budgets.
pub fn map_stats(id: MapId) {
    let m = id.build();
    let s = m.stats();
    println!("{}: {} brushes, {} decor, {} spawns, {} nav nodes, extent {:.0}x{:.0}x{:.0}",
             m.name(), s.brushes, s.decor, s.spawns, s.nav_nodes, s.extent.x, s.extent.y, s.extent.z);
}

/// Headless simulation soak test.
///
/// Drives a full world with random but plausible input for every player and
/// checks the invariants that matter: nobody falls out of the map, nobody ends
/// up inside geometry, and a tick stays comfortably inside its budget.
pub fn sim_test(map_name: &str, seconds: f32, players: usize) -> i32 {
    use crate::core::Rng;
    use crate::game::sim::{World, TICK_DT};
    use crate::game::types::{Buttons, InputCmd, Team};
    use crate::maps::brush::TraceMask;

    let id = match ALL_MAPS.iter().find(|m| m.name().eq_ignore_ascii_case(map_name)) {
        Some(i) => *i,
        None => { eprintln!("unknown map '{}'", map_name); return 1; }
    };

    let build_start = std::time::Instant::now();
    let mut world = World::new(id, 0xC0FFEE);
    let build_ms = build_start.elapsed().as_secs_f32() * 1000.0;

    let n = players.min(crate::game::types::MAX_PLAYERS);
    for i in 0..n {
        let p = &mut world.players[i];
        p.in_use = true;
        p.is_bot = true;
        p.name = format!("SIM{:02}", i);
        p.team = if i % 2 == 0 { Team::Phantom } else { Team::Vanguard };
    }
    for i in 0..n { world.respawn_player(i as u8, true); }

    let mut rng = Rng::seeded(7);
    let mut cmds: Vec<InputCmd> = (0..n).map(|i| InputCmd {
        seq: 0, dt_ms: (TICK_DT * 1000.0) as u8,
        move_f: 127, move_r: 0,
        yaw: rng.range(0.0, 6.28), pitch: 0.0,
        buttons: Buttons::empty(), weapon: 0xFF,
    }).collect();
    let _ = &cmds;

    let ticks = (seconds / TICK_DT) as u32;
    let mut worst_tick = 0.0f32;
    let mut total = 0.0f64;
    let mut escapes = 0u32;
    let mut stuck = 0u32;
    let mut shots = 0u64;
    let mut kills = 0u64;

    for t in 0..ticks {
        let start = std::time::Instant::now();
        for i in 0..n {
            let c = &mut cmds[i];
            c.seq = c.seq.wrapping_add(1);
            // Wander, jump, crouch, shoot: enough variety to exercise every
            // branch of the mover and the weapon machine.
            c.yaw += rng.range(-0.09, 0.09);
            c.pitch = (c.pitch + rng.range(-0.05, 0.05)).clamp(-1.2, 1.2);
            c.move_f = if rng.chance(0.86) { 127 } else { -80 };
            c.move_r = (rng.signed() * 90.0) as i8;
            c.buttons = Buttons::empty();
            if rng.chance(0.30) { c.buttons.insert(Buttons::SPRINT); }
            if rng.chance(0.02) { c.buttons.insert(Buttons::JUMP); }
            if rng.chance(0.08) { c.buttons.insert(Buttons::CROUCH); }
            if rng.chance(0.20) { c.buttons.insert(Buttons::FIRE); }
            if rng.chance(0.05) { c.buttons.insert(Buttons::ADS); }
            if rng.chance(0.004) { c.buttons.insert(Buttons::LETHAL); }
            c.sanitize();
            world.run_command(i as u8, c);
        }
        world.step(TICK_DT);

        // Respawn anyone who died so the test keeps exercising the world.
        for i in 0..n {
            if !world.players[i].alive && world.time >= world.players[i].respawn_at {
                world.respawn_player(i as u8, false);
            }
        }

        let ms = start.elapsed().as_secs_f32() * 1000.0;
        worst_tick = worst_tick.max(ms);
        total += ms as f64;

        for i in 0..n {
            let p = &world.players[i];
            if !p.alive { continue; }
            if !world.map.bounds.expanded_uniform(6.0).contains_point(p.mv.pos) { escapes += 1; }
            // Only geometry too tall to step onto counts as being stuck;
            // standing on a staircase legitimately overlaps the next tread.
            let feet = p.mv.pos + glam::Vec3::Y * 0.06;
            let body = crate::math::Aabb::from_base(feet, 0.32, p.mv.height - 0.12);
            let mut trapped = false;
            world.map.collision.grid.query_aabb(&body, |bi| {
                if trapped { return; }
                let b = &world.map.collision.brushes[bi as usize];
                if !b.is_solid() { return; }
                if b.aabb.max.y <= feet.y + crate::game::movement::tune::STEP_HEIGHT { return; }
                if b.sweep_box().overlaps(&body) { trapped = true; }
            });
            if trapped {
                stuck += 1;
                if stuck <= 6 {
                    let mut who = None;
                    world.map.collision.grid.query_aabb(&body, |bi| {
                        if who.is_some() { return; }
                        let b = &world.map.collision.brushes[bi as usize];
                        if b.is_solid() && b.aabb.max.y > feet.y + crate::game::movement::tune::STEP_HEIGHT
                            && b.sweep_box().overlaps(&body) { who = Some(bi); }
                    });
                    if let Some(bi) = who {
                        let b = &world.map.collision.brushes[bi as usize];
                        println!("  stuck: player {} at {:?} grounded={} inside brush {} {:?} y=[{:.2},{:.2}] x=[{:.2},{:.2}] z=[{:.2},{:.2}]",
                                 i, p.mv.pos, p.mv.grounded, bi, b.mat, b.aabb.min.y, b.aabb.max.y,
                                 b.aabb.min.x, b.aabb.max.x, b.aabb.min.z, b.aabb.max.z);
                    }
                }
            }
        }
        for e in world.events.iter() {
            match e {
                crate::game::events::GameEvent::Shot { .. } => shots += 1,
                crate::game::events::GameEvent::Kill { .. } => kills += 1,
                _ => {}
            }
        }
        world.events.clear();
        let _ = t;
    }

    let avg = total / ticks as f64;
    println!("{} | {} players | {:.0}s simulated in {} ticks", id.name(), n, seconds, ticks);
    println!("  map build      {:.1} ms", build_ms);
    println!("  tick average   {:.3} ms   ({:.0}% of a 16.6 ms budget)", avg, avg / 16.666 * 100.0);
    println!("  tick worst     {:.3} ms", worst_tick);
    println!("  shots fired    {}", shots);
    println!("  kills          {}", kills);
    println!("  escapes        {}", escapes);
    println!("  stuck samples  {}", stuck);

    if escapes > 0 || stuck > ticks / 20 {
        println!("  FAILED: players are leaving the map or clipping into geometry");
        return 1;
    }
    println!("  OK");
    0
}

/// Runs a full match of bots against each other with no clients attached.
///
/// This is the fastest way to tell whether the AI, the modes and the world
/// actually work together: if bots cannot find each other, cannot capture a
/// point or cannot finish a round, it shows up here in seconds.
pub fn bot_match(map_name: &str, mode_name: &str, seconds: f32, bots: u8, difficulty: u8) -> i32 {
    use crate::modes::{ModeId, Phase, ALL_MODES};
    use crate::net::server::{Server, ServerConfig};

    let map = match ALL_MAPS.iter().find(|m| m.name().eq_ignore_ascii_case(map_name)) {
        Some(i) => *i,
        None => { eprintln!("unknown map '{}'", map_name); return 1; }
    };
    let mode = ALL_MODES.iter()
        .find(|m| m.short().eq_ignore_ascii_case(mode_name) || m.name().eq_ignore_ascii_case(mode_name))
        .copied()
        .unwrap_or(ModeId::TeamDeathmatch);

    let cfg = ServerConfig {
        name: "BOT TEST".into(),
        port: 0,
        bot_count: bots,
        map,
        mode,
        bot_difficulty: difficulty,
        ..ServerConfig::default()
    };
    let mut server = match Server::bind(cfg) {
        Ok(s) => s,
        Err(e) => { eprintln!("bind failed: {}", e); return 1; }
    };
    server.start_match();

    let dt = 1.0 / 60.0;
    let steps = (seconds / dt) as u32;
    let start = std::time::Instant::now();
    let mut travelled = vec![0.0f32; crate::game::types::MAX_PLAYERS];
    // Scores reset when the rotation advances, so kills are accumulated from
    // per-tick deltas rather than read once at the end.
    let mut total_kills = 0u32;
    let mut total_deaths = 0u32;
    let mut prev_kills = vec![0u16; crate::game::types::MAX_PLAYERS];
    let mut prev_deaths = vec![0u16; crate::game::types::MAX_PLAYERS];
    let mut last_pos: Vec<glam::Vec3> = server.world.players.iter().map(|p| p.mv.pos).collect();
    let mut phases_seen: Vec<Phase> = Vec::new();

    for _ in 0..steps {
        server.update(dt);
        for i in 0..server.world.players.len() {
            let p = &server.world.players[i];
            if p.in_use && p.alive {
                travelled[i] += (p.mv.pos - last_pos[i]).length().min(1.0);
            }
            last_pos[i] = p.mv.pos;
            if p.score.kills > prev_kills[i] { total_kills += (p.score.kills - prev_kills[i]) as u32; }
            if p.score.deaths > prev_deaths[i] { total_deaths += (p.score.deaths - prev_deaths[i]) as u32; }
            prev_kills[i] = p.score.kills;
            prev_deaths[i] = p.score.deaths;
        }
        if phases_seen.last() != Some(&server.state.phase) {
            if phases_seen.len() < 40 {
                println!("  [t={:5.1}] -> {:?}  clock={:.1} alive P{}/V{} wins {}:{}",
                         server.world.time, server.state.phase, server.state.clock,
                         server.world.team_alive(crate::game::types::Team::Phantom),
                         server.world.team_alive(crate::game::types::Team::Vanguard),
                         server.state.round_wins[1], server.state.round_wins[2]);
            }
            phases_seen.push(server.state.phase);
        }
    }
    let wall = start.elapsed().as_secs_f32();

    let kills = total_kills;
    let deaths = total_deaths;
    let mut moved_enough = 0u32;
    println!("{} | {} | {} bots | difficulty {}", map.name(), mode.name(), bots, difficulty);
    println!("{:<14} {:>3} {:>4} {:>4} {:>6} {:>8}", "BOT", "TM", "K", "D", "SCORE", "METRES");
    for (i, p) in server.world.players.iter().enumerate() {
        if !p.in_use { continue; }
        if travelled[i] > seconds * 0.6 { moved_enough += 1; }
        println!("{:<14} {:>3} {:>4} {:>4} {:>6} {:>8.0}",
                 p.name, p.team.short(), p.score.kills, p.score.deaths, p.score.score, travelled[i]);
    }
    let active = server.world.active_players().count() as u32;
    println!();
    println!("  simulated {:.0}s of match in {:.2}s wall ({:.0}x real time)", seconds, wall, seconds / wall.max(0.001));
    println!("  phases    {:?}", phases_seen);
    println!("  kills {} deaths {}  ({:.1} kills/min/bot)", kills, deaths,
             kills as f32 / (seconds / 60.0) / active.max(1) as f32);
    println!("  bots that moved meaningfully: {}/{}", moved_enough, active);
    println!("  scores    PHANTOM {}  VANGUARD {}",
             server.state.score(crate::game::types::Team::Phantom),
             server.state.score(crate::game::types::Team::Vanguard));
    println!("  tick avg {:.3} ms  worst {:.3} ms", server.stats.avg_tick_ms, server.stats.worst_tick_ms);

    let mut bad = 0;
    if kills == 0 { println!("  FAILED: bots never killed each other"); bad = 1; }
    if moved_enough * 2 < active { println!("  FAILED: most bots barely moved"); bad = 1; }
    if bad == 0 { println!("  OK"); }
    bad
}

/// Runs a dedicated server with no window.
pub fn run_dedicated(args: &[String]) -> i32 {
    use crate::core::{FrameClock, RateLimiter};
    use crate::modes::{ModeId, ALL_MODES};
    use crate::net::discovery::TrackerClient;
    use crate::net::server::{Server, ServerConfig};

    let mut cfg = ServerConfig {
        name: "HARDPOINT DEDICATED".into(),
        dedicated: true,
        ..ServerConfig::default()
    };
    let mut tracker: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let key = args[i].as_str();
        let value = args.get(i + 1).map(|s| s.as_str()).unwrap_or("");
        match key {
            "--port" => { cfg.port = value.parse().unwrap_or(cfg.port); i += 1; }
            "--map" => {
                if let Some(m) = ALL_MAPS.iter().find(|m| m.name().eq_ignore_ascii_case(value)) {
                    cfg.map = *m;
                }
                i += 1;
            }
            "--mode" => {
                if let Some(m) = ALL_MODES.iter().find(|m| m.short().eq_ignore_ascii_case(value) || m.name().eq_ignore_ascii_case(value)) {
                    cfg.mode = *m;
                }
                i += 1;
            }
            "--bots" => { cfg.bot_count = value.parse().unwrap_or(cfg.bot_count); i += 1; }
            "--skill" => { cfg.bot_difficulty = value.parse().unwrap_or(cfg.bot_difficulty).min(3); i += 1; }
            "--players" => { cfg.max_players = value.parse().unwrap_or(cfg.max_players); i += 1; }
            "--name" => { cfg.name = value.to_string(); i += 1; }
            "--password" => { cfg.password = value.to_string(); i += 1; }
            "--tracker" => { tracker = Some(value.to_string()); i += 1; }
            "--no-rotate" => { cfg.rotation = vec![cfg.map]; }
            other => {
                if other.starts_with("--") {
                    eprintln!("unknown option '{}'", other);
                }
            }
        }
        i += 1;
    }
    cfg.mode = ModeId::from_u8(cfg.mode as u8);

    let mut server = match Server::bind(cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not start the server: {e}");
            return 1;
        }
    };
    let mut tracker_client = tracker.as_deref().and_then(TrackerClient::new);

    println!("HARDPOINT dedicated server");
    println!("  name    {}", server.cfg.name);
    println!("  port    {}", server.port());
    println!("  map     {}", server.world.map_id.name());
    println!("  mode    {}", server.mode.id().name());
    println!("  bots    {} (skill {})", server.cfg.bot_count, server.cfg.bot_difficulty);
    println!("  players {} max", server.cfg.max_players);
    if tracker_client.is_some() { println!("  tracker {}", tracker.unwrap()); }
    println!("Press Ctrl-C to stop.");

    server.start_match();

    let mut clock = FrameClock::new();
    let mut limiter = RateLimiter::new(120.0);
    let mut score_timer = 0.0f32;
    let mut report_timer = 0.0f32;

    loop {
        clock.tick();
        server.update(clock.dt);

        score_timer += clock.dt;
        if score_timer > 0.5 {
            score_timer = 0.0;
            server.broadcast_scores();
        }

        if let Some(t) = tracker_client.as_mut() {
            let port = server.port();
            let info = server.info();
            t.update(server.now(), port, &info);
        }

        report_timer += clock.dt;
        if report_timer > 30.0 {
            report_timer = 0.0;
            let info = server.info();
            println!(
                "[{:>6.0}s] {} on {}  players {}+{} bots  phase {:?}  tick {:.2} ms",
                server.now(), server.mode.id().short(), server.world.map_id.name(),
                info.players, info.bots,
                crate::modes::Phase::from_u8(info.phase),
                server.stats.avg_tick_ms,
            );
        }

        if server.shutdown { break; }
        limiter.wait();
    }
    0
}

/// Runs a tracker: a registry servers announce themselves to and clients query.
pub fn run_tracker(port: u16) -> i32 {
    use crate::core::{FrameClock, RateLimiter};
    use crate::net::discovery::Tracker;

    let mut tracker = match Tracker::bind(port) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("could not start the tracker: {e}");
            return 1;
        }
    };
    println!("HARDPOINT tracker listening on port {}", tracker.port());
    println!("Servers register with `--tracker <this machine>:{}`.", tracker.port());

    let mut clock = FrameClock::new();
    let mut limiter = RateLimiter::new(60.0);
    let mut report = 0.0f32;
    loop {
        clock.tick();
        tracker.update(clock.now_secs());
        report += clock.dt;
        if report > 60.0 {
            report = 0.0;
            println!("[{:>6.0}s] {} server(s) registered", clock.now_secs(), tracker.count());
        }
        limiter.wait();
    }
}
