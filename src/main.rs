//! HARDPOINT: OPERATION IRONVEIL
//!
//! A multiplayer first-person shooter in the register of the early 2000s
//! console era: fast movement, tight maps, chunky geometry, and an
//! authoritative server behind every match including the ones you host
//! yourself.

#[macro_use]
mod macros;

mod app;
mod assets;
mod audio;
mod bots;
mod core;
mod devtools;
mod game;
mod input;
mod maps;
mod math;
mod modes;
mod net;
mod progression;
mod render;
mod settings;
mod ui;

use progression::Progression;
use settings::Settings;

const USAGE: &str = "\
HARDPOINT: OPERATION IRONVEIL

  hardpoint                       play
  hardpoint --connect <address>   play, joining a server immediately
  hardpoint --server [options]    run a dedicated server with no window
  hardpoint --tracker [port]      run a server registry others can query

Developer tools:
  hardpoint --audit               validate every map
  hardpoint --nav <MAP> [height]  print a slice of a map's navigation graph
  hardpoint --probe <MAP> <x> <z> explain one column of a map
  hardpoint --simtest <MAP> [s] [n]   headless movement and collision soak
  hardpoint --botmatch <MAP> <MODE> [s] [bots] [skill]  headless match

Server options:
  --port <n>  --map <NAME>  --mode <TDM|FFA|DOM|S&D|GUN>
  --bots <n>  --skill <0-3> --players <n>  --name <text>
  --password <text>  --tracker <address>
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let first = args.first().map(|s| s.as_str()).unwrap_or("");

    let code = match first {
        "--help" | "-h" => {
            println!("{USAGE}");
            0
        }
        "--audit" => devtools::audit(),
        "--nav" => {
            let name = args.get(1).map(|s| s.as_str()).unwrap_or("IRONVEIL");
            let y = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            devtools::nav_dump(name, y)
        }
        "--probe" => {
            let name = args.get(1).map(|s| s.as_str()).unwrap_or("IRONVEIL");
            let x = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            let z = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            devtools::probe(name, x, z)
        }
        "--simtest" => {
            let name = args.get(1).map(|s| s.as_str()).unwrap_or("IRONVEIL");
            let secs = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20.0);
            let n = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(12);
            devtools::sim_test(name, secs, n)
        }
        "--botmatch" => {
            let map = args.get(1).map(|s| s.as_str()).unwrap_or("IRONVEIL");
            let mode = args.get(2).map(|s| s.as_str()).unwrap_or("TDM");
            let secs = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60.0);
            let bots = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(10);
            let diff = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(2);
            devtools::bot_match(map, mode, secs, bots, diff)
        }
        "--server" => devtools::run_dedicated(&args[1..]),
        "--tracker" => {
            let port = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(net::protocol::DEFAULT_PORT + 100);
            devtools::run_tracker(port)
        }
        _ => run_game(&args),
    };
    std::process::exit(code);
}

fn run_game(args: &[String]) -> i32 {
    let settings = Settings::load();
    let progression = Progression::load();

    let mut connect_to = None;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--connect" {
            connect_to = args.get(i + 1).cloned();
            i += 1;
        }
        i += 1;
    }

    let event_loop = match winit::event_loop::EventLoop::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("could not create an event loop: {e}");
            eprintln!("if you are on a headless machine, try `hardpoint --server` instead.");
            return 1;
        }
    };
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);

    let mut launcher = app::Launcher::new(settings, progression, connect_to);
    if let Err(e) = event_loop.run_app(&mut launcher) {
        eprintln!("the game exited with an error: {e}");
        return 1;
    }
    if let Some(err) = launcher.error() {
        eprintln!("could not start: {err}");
        return 1;
    }
    0
}
