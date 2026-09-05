//! Headless dedicated server.
//!
//! Runs the same server the game hosts internally, with no window, no
//! renderer and no audio. Useful for a permanent LAN box, and the reason the
//! networking has to be real rather than a local shortcut.

const USAGE: &str = "\
HARDPOINT DEDICATED SERVER

  hardpoint-server [options]

  --port <n>            listen port (default 27015)
  --map <NAME>          starting map
  --mode <TDM|FFA|DOM|S&D|GUN>
  --bots <n>            bots to fill with
  --skill <0-3>         bot difficulty
  --players <n>         maximum players
  --name <text>         server name shown in the browser
  --password <text>     require a password to join
  --tracker <address>   register with a tracker so it is listed off-LAN
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    std::process::exit(hardpoint::devtools::run_dedicated(&args));
}
