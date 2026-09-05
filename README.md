# Hardpoint: Operation Ironveil

A complete multiplayer arena shooter in the style of a 2002 console FPS, written
in Rust. Twelve original maps, five game modes, twenty-five weapons, bots that
actually play the objective, and a real authoritative server over UDP.

There is no editor, no asset pipeline and no content directory. Every texture,
mesh, sound effect, music track and glyph is generated procedurally at load
time, which is why the whole game is a few hundred kilobytes of binary and
starts in well under a second.

## Installing

On Arch and derivatives, build a real package so pacman can remove it cleanly:

```sh
makepkg -si
```

Anywhere else:

```sh
./install.sh                 # builds, self-tests, installs to /usr/local
PREFIX=/usr ./install.sh     # or somewhere else
sudo make uninstall          # to remove it again
```

That installs both binaries, the icon at six sizes, a desktop entry, two
manual pages and a systemd unit for the dedicated server. The game generates
every asset at runtime, so there is no data directory to go with them and the
binary runs from anywhere.

Requires a Rust toolchain (1.80 or newer) and a Vulkan, Metal, D3D12 or GLES
capable GPU. The only dependencies are `wgpu`, `winit`, `glam`, `bytemuck`,
`pollster` and `cpal`.

## Building without installing

```sh
make build     # release binaries in target/release
make check     # map audit, traversal test, and bot matches in three modes
```

## Playing

```sh
hardpoint                        # launch the game
hardpoint --connect <address>    # launch and join a server directly
```

From the main menu:

- **QUICK MATCH** starts a local server with bots and drops you straight in.
- **MULTIPLAYER → HOST A GAME** lets you pick the map, mode, player count, bot
  count and difficulty, then opens a lobby others can join.
- **MULTIPLAYER → SERVER BROWSER** finds servers on the local network by UDP
  broadcast, plus anything registered with a tracker.
- **MULTIPLAYER → DIRECT CONNECT** joins by address, with an optional password.

### Dedicated server

```sh
hardpoint-server --map DRYDOCK --mode DOM --bots 8
```

Installed systems get a service unit reading `/etc/hardpoint/server.conf`:

```sh
sudo systemctl enable --now hardpoint-server
```

It runs under a dynamic user with no filesystem access and a UDP socket as its
only capability.

Options: `--port`, `--map`, `--mode` (`TDM`, `FFA`, `DOM`, `S&D`, `GUN`),
`--bots`, `--skill 0-3`, `--players`, `--name`, `--password`, `--tracker`.

It runs headless, holds the authoritative simulation, cycles maps at the end of
each match, and answers LAN discovery so it shows up in the browser.

### Tracker

```sh
hardpoint --tracker
```

A tiny registry servers announce themselves to and browsers query, for when
players are not on the same broadcast domain. Servers register with
`--tracker <address>`.

## Controls

| | |
|---|---|
| Move | `W` `A` `S` `D` |
| Jump / respawn | `Space` |
| Crouch | `Left Ctrl` |
| Prone | `Z` |
| Sprint | `Left Shift` |
| Fire / aim | `Mouse 1` / `Mouse 2` |
| Reload | `R` |
| Melee | `V` |
| Lethal / tactical | `G` / `F` |
| Use, plant, defuse | `E` |
| Weapons | `1` `2` `3`, or the mouse wheel |
| Scoreboard | `Tab` |
| Chat / team chat | `T` / `Y` |
| Pause | `Esc` |
| Performance overlay | `F3` |

Everything is rebindable in Settings → Controls.

## Maps

| | | |
|---|---|---|
| **Ironveil** | Desert airfield | **Drydock** | Shipyard |
| **Stormworks** | Industrial warehouse | **Foundry** | Abandoned foundry |
| **Belvoir** | European village | **Saltbite** | Coastal fort |
| **Greenline** | Jungle relay station | **Deepwell** | Underground bunker |
| **Whiteout** | Arctic radar site | **Junction** | Rail junction |
| **Highrise** | Urban apartments | **Overpass** | Highway checkpoint |

Each is small-to-medium, built for eight to sixteen players, with team spawns,
flanking routes, verticality, a power position or two and weapon pickups.

## Modes

- **Team Deathmatch** — two teams, first to the score limit.
- **Free For All** — everyone for themselves.
- **Domination** — three capture points, score ticks with control.
- **Search & Destroy** — one life a round, plant or defend the bomb.
- **Gun Game** — a kill promotes you through twenty weapons; first to the end.

## Weapons

Twenty-five: five assault rifles, five submachine guns, three shotguns, three
sniper rifles, three light machine guns, four sidearms and two melee weapons.
Each has its own damage curve, range falloff, fire mode, cadence, recoil
pattern, spread model, reload timings, penetration and mobility cost. The
loadout screen shows all of it.

## Architecture

```
src/
  core/      ring buffer, RNG, frame clock, key-value config
  math/      AABBs, swept collision, frustum
  maps/      brush geometry, the map DSL, twelve maps, navigation graph
  game/      player state, movement, weapons, projectiles, simulation
  modes/     the five game modes behind one trait
  bots/      director and per-bot behaviour
  net/       bit packing, protocol, channels, server, client, discovery
  assets/    texture, font, mesh and character generators
  render/    wgpu device, eight pipelines, one shader file
  ui/        theme, painter, immediate-mode widgets
  app/       screens, HUD, effects, world view
  audio/     synthesis, music, mixer
```

**The server is authoritative.** Clients send input commands and nothing else.
Position, damage, kills, score, match state, respawns and unlocks are all
decided server-side, and every command is bounds-checked and rate-limited
before it is applied. Clients predict their own movement and reconcile against
the server's answer; other players are interpolated on a 100 ms delay; the
server rewinds player positions by each shooter's latency so shots land where
the shooter saw them.

Snapshots are delta-compressed against the last acknowledged state, with
per-field change masks and quantised positions, velocities and angles, and are
culled by relevance. Reliable messages ride an ordered stream on the same
socket; everything else is fire-and-forget.

## Development tools

```sh
hardpoint --audit                      # validate all twelve maps
hardpoint --nav <MAP> [height]         # ASCII dump of the navigation graph
hardpoint --probe <MAP> <x> <z>        # explain one column of a map
hardpoint --simtest <MAP> [secs] [n]   # headless movement and collision soak
hardpoint --botmatch <MAP> <MODE> ...  # headless match, many times real time
hardpoint --stairs [MAP|ALL]           # walk every route; report wedge points
hardpoint --icon <path> [size]         # write the application icon
```

`HARDPOINT_CENSUS=1` on a `--botmatch` lists every event the match produced,
which is how you tell a subsystem that is not running from one that is.

## Performance

The renderer targets low-end hardware: a four-core CPU with integrated
graphics at 720p or 900p. Geometry is brush-based with baked per-vertex
lighting and no runtime shadow work; culling is done on pre-built spatial
clusters; textures live in a single array so the whole map draws in a handful
of calls. Settings offers Low End, Balanced, High and Authentic presets, plus
a render-resolution scale that renders the world small and point-upscales it,
which is both a performance lever and the period look.

## Licence

Original work. No trademarked names, likenesses, maps, sounds or assets from
any existing game.
