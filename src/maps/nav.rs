//! Navigation.
//!
//! A navigation grid is baked once when a map is built: the playspace is
//! sampled on a one-metre lattice, every standable surface in each column
//! becomes a node, and nodes are linked to their eight neighbours when a
//! walking character could actually make the transition. That gives multi-level
//! navigation (catwalks over floors) without the cost of building a real
//! navmesh, and A* over it costs a few microseconds.
//!
//! The bake also records, per node, which directions are blocked at chest
//! height. Bots use that to pick cover without any additional raycasting.

use super::brush::{CollisionWorld, TraceMask};
use crate::math::Aabb;
use glam::{Vec2, Vec3};

pub const CELL: f32 = 1.0;
/// Maximum height difference a bot will walk up without jumping.
pub const STEP_UP: f32 = 0.55;
/// Maximum drop a bot will take voluntarily.
pub const MAX_DROP: f32 = 3.2;
/// Steepest average climb allowed across a single graph link, as rise over
/// run. Node spacing is a metre, so a 45-degree staircase gains a full metre
/// between adjacent nodes; judging that by `STEP_UP` alone would declare every
/// staircase impassable. Each individual riser is still checked against
/// `STEP_UP` by the walk simulation.
pub const MAX_CLIMB_SLOPE: f32 = 1.45;
/// The navigation agent must be at least as wide as the player, plus a margin.
///
/// It used to be a hair narrower, which meant navigation happily placed nodes
/// a centimetre from a wall. Bots pathed through them, the stair-climb test
/// walked them, and a player following the same line scraped the wall and
/// stuck. Navigation has to be a subset of where the player actually fits.
const AGENT_RADIUS: f32 = crate::game::movement::tune::RADIUS + 0.02;
/// How far above a surface an agent's collision box is considered to start.
/// It must exceed the thickness of any decorative overlay slab a map lays on
/// top of a floor (road markings, ice, paving), otherwise the box of a node in
/// one cell clips the overlay in the cell next door and the node is discarded.
pub const GROUND_CLEARANCE: f32 = 0.10;
/// Step height measured from the agent's feet rather than from the surface.
pub const STEP_ALLOWANCE: f32 = STEP_UP - GROUND_CLEARANCE;
const AGENT_HEIGHT: f32 = 1.80;
/// Surfaces closer together than this in one column collapse to one node.
const LAYER_EPS: f32 = 0.35;
const MAX_LAYERS: usize = 6;

#[derive(Copy, Clone)]
pub struct NavNode {
    pub pos: Vec3,
    /// Bitmask of the eight compass directions that are blocked at chest
    /// height. A high popcount means good cover.
    pub cover: u8,
    /// Distance to the nearest node that has cover, in metres, capped. Used by
    /// bots to retreat towards safety.
    pub cover_dist: f32,
}

#[derive(Copy, Clone)]
pub struct NavLink {
    pub to: u32,
    pub cost: f32,
}

/// Positions tried within a cell when placing a node, centre first. Offsets
/// stay under half a cell so a nudged node still belongs to its own cell.
const NUDGES: [(f32, f32); 5] = [(0.0, 0.0), (0.30, 0.0), (-0.30, 0.0), (0.0, 0.30), (0.0, -0.30)];

/// Eight-way neighbour offsets, cardinals first so paths prefer straight runs.
const DIRS: [(i32, i32); 8] = [
    (1, 0), (-1, 0), (0, 1), (0, -1),
    (1, 1), (1, -1), (-1, 1), (-1, -1),
];

pub struct NavGrid {
    origin: Vec2,
    nx: i32,
    nz: i32,
    pub nodes: Vec<NavNode>,
    /// CSR: cell index -> node indices.
    cell_start: Vec<u32>,
    cell_nodes: Vec<u32>,
    /// CSR: node index -> links.
    link_start: Vec<u32>,
    links: Vec<NavLink>,
}

impl NavGrid {
    pub fn empty() -> NavGrid {
        NavGrid {
            origin: Vec2::ZERO, nx: 0, nz: 0,
            nodes: Vec::new(), cell_start: vec![0], cell_nodes: Vec::new(),
            link_start: vec![0], links: Vec::new(),
        }
    }

    pub fn walkable_count(&self) -> usize { self.nodes.len() }

    pub fn bake(world: &CollisionWorld, playspace: Aabb) -> NavGrid {
        let origin = Vec2::new(playspace.min.x, playspace.min.z);
        let nx = (((playspace.max.x - playspace.min.x) / CELL).ceil() as i32).clamp(1, 220);
        let nz = (((playspace.max.z - playspace.min.z) / CELL).ceil() as i32).clamp(1, 220);
        let cell_count = (nx * nz) as usize;

        let y_top = playspace.max.y;
        let y_bottom = playspace.min.y;

        let mut nodes: Vec<NavNode> = Vec::with_capacity(cell_count);
        let mut cell_start = vec![0u32; cell_count + 1];
        let mut cell_nodes: Vec<u32> = Vec::with_capacity(cell_count);

        // Column scan. Walking a probe box down the column and recording every
        // place it could stand is simple, exact for box worlds, and only runs
        // once per map.
        let mut surfaces: Vec<f32> = Vec::with_capacity(16);
        for cz in 0..nz {
            for cx in 0..nx {
                let ci = (cz * nx + cx) as usize;
                cell_start[ci] = cell_nodes.len() as u32;
                let wx = origin.x + (cx as f32 + 0.5) * CELL;
                let wz = origin.y + (cz as f32 + 0.5) * CELL;

                surfaces.clear();
                collect_surfaces(world, wx, wz, y_bottom, y_top, &mut surfaces);

                let mut placed = 0usize;
                for &h in surfaces.iter() {
                    if placed >= MAX_LAYERS { break; }
                    // Try the cell centre first, then a few offsets inside the
                    // same cell. A doorway is only a little wider than the
                    // agent, so whether a node exists in it would otherwise
                    // depend on how the lattice happens to line up with the
                    // wall - which is how interiors end up unreachable.
                    let mut chosen = None;
                    for (ox, oz) in NUDGES {
                        let feet = Vec3::new(wx + ox, h + GROUND_CLEARANCE, wz + oz);
                        if world.standable(feet, AGENT_RADIUS, AGENT_HEIGHT, STEP_ALLOWANCE) {
                            chosen = Some(feet);
                            break;
                        }
                    }
                    let Some(feet) = chosen else { continue };
                    cell_nodes.push(nodes.len() as u32);
                    nodes.push(NavNode { pos: feet, cover: 0, cover_dist: 0.0 });
                    placed += 1;
                }
            }
        }
        cell_start[cell_count] = cell_nodes.len() as u32;

        let mut grid = NavGrid {
            origin, nx, nz, nodes, cell_start, cell_nodes,
            link_start: Vec::new(), links: Vec::new(),
        };
        grid.build_links(world);
        grid.compute_cover(world);
        grid
    }

    fn build_links(&mut self, world: &CollisionWorld) {
        let n = self.nodes.len();
        let mut link_start = vec![0u32; n + 1];
        let mut links: Vec<NavLink> = Vec::with_capacity(n * 5);

        for i in 0..n {
            link_start[i] = links.len() as u32;
            let from = self.nodes[i].pos;
            let (cx, cz) = self.cell_coords(from.x, from.z);
            for (di, &(dx, dz)) in DIRS.iter().enumerate() {
                let (ncx, ncz) = (cx + dx, cz + dz);
                if ncx < 0 || ncz < 0 || ncx >= self.nx || ncz >= self.nz { continue; }
                let diagonal = di >= 4;
                // Diagonals are only allowed when both cardinals are open,
                // otherwise bots clip corners and stick on them.
                if diagonal {
                    let a = self.best_node_in_cell(cx + dx, cz, from.y);
                    let b = self.best_node_in_cell(cx, cz + dz, from.y);
                    if a.is_none() || b.is_none() { continue; }
                }
                let ci = (ncz * self.nx + ncx) as usize;
                let a = self.cell_start[ci] as usize;
                let b = self.cell_start[ci + 1] as usize;
                for &nidx in &self.cell_nodes[a..b] {
                    let to = self.nodes[nidx as usize].pos;
                    let dy = to.y - from.y;
                    let horiz = ((to.x - from.x).powi(2) + (to.z - from.z).powi(2)).sqrt();
                    if dy > STEP_UP.max(horiz * MAX_CLIMB_SLOPE) || dy < -MAX_DROP { continue; }
                    if !walkable_between(world, from, to) { continue; }
                    // Climbing and dropping both cost extra, so bots prefer
                    // flat routes when one exists.
                    let cost = horiz + dy.max(0.0) * 2.4 + (-dy).max(0.0) * 0.8;
                    links.push(NavLink { to: nidx, cost });
                }
            }
        }
        link_start[n] = links.len() as u32;
        self.link_start = link_start;
        self.links = links;
    }

    /// Marks which compass directions are blocked at chest height, then runs a
    /// multi-source BFS out from covered nodes to give every node a distance
    /// to the nearest piece of cover.
    fn compute_cover(&mut self, world: &CollisionWorld) {
        let n = self.nodes.len();
        let mut queue: Vec<u32> = Vec::with_capacity(n / 4 + 1);
        for i in 0..n {
            let p = self.nodes[i].pos + Vec3::Y * 1.05;
            let mut mask = 0u8;
            for (di, &(dx, dz)) in DIRS.iter().enumerate() {
                let d = Vec3::new(dx as f32, 0.0, dz as f32).normalize();
                if world.trace_ray(p, d, 1.4, TraceMask::Shot).hit {
                    mask |= 1 << di;
                }
            }
            self.nodes[i].cover = mask;
            self.nodes[i].cover_dist = f32::MAX;
            if mask.count_ones() >= 2 {
                self.nodes[i].cover_dist = 0.0;
                queue.push(i as u32);
            }
        }
        let mut head = 0usize;
        while head < queue.len() {
            let cur = queue[head] as usize;
            head += 1;
            let d0 = self.nodes[cur].cover_dist;
            if d0 > 24.0 { continue; }
            let a = self.link_start[cur] as usize;
            let b = self.link_start[cur + 1] as usize;
            for li in a..b {
                let l = self.links[li];
                let nd = d0 + l.cost;
                let t = l.to as usize;
                if nd + 0.001 < self.nodes[t].cover_dist {
                    self.nodes[t].cover_dist = nd;
                    queue.push(l.to);
                }
            }
        }
        for node in self.nodes.iter_mut() {
            if node.cover_dist == f32::MAX { node.cover_dist = 999.0; }
        }
    }

    #[inline]
    fn cell_coords(&self, x: f32, z: f32) -> (i32, i32) {
        (
            (((x - self.origin.x) / CELL).floor() as i32).clamp(0, self.nx.max(1) - 1),
            (((z - self.origin.y) / CELL).floor() as i32).clamp(0, self.nz.max(1) - 1),
        )
    }

    fn best_node_in_cell(&self, cx: i32, cz: i32, near_y: f32) -> Option<u32> {
        if cx < 0 || cz < 0 || cx >= self.nx || cz >= self.nz { return None; }
        let ci = (cz * self.nx + cx) as usize;
        let a = self.cell_start[ci] as usize;
        let b = self.cell_start[ci + 1] as usize;
        let mut best = None;
        let mut best_d = f32::MAX;
        for &i in &self.cell_nodes[a..b] {
            let d = (self.nodes[i as usize].pos.y - near_y).abs();
            if d < best_d { best_d = d; best = Some(i); }
        }
        best
    }

    /// Nearest navigation node to a world position, searching outward by rings
    /// so a character standing in a doorway still finds one.
    pub fn nearest(&self, p: Vec3) -> Option<u32> {
        if self.nodes.is_empty() { return None; }
        let (cx, cz) = self.cell_coords(p.x, p.z);
        let mut best: Option<u32> = None;
        let mut best_d = f32::MAX;
        for r in 0i32..6 {
            for dz in -r..=r {
                for dx in -r..=r {
                    // Only the ring, not the filled square.
                    if r > 0 && dx.abs() != r && dz.abs() != r { continue; }
                    let (gx, gz) = (cx + dx, cz + dz);
                    if gx < 0 || gz < 0 || gx >= self.nx || gz >= self.nz { continue; }
                    let ci = (gz * self.nx + gx) as usize;
                    let a = self.cell_start[ci] as usize;
                    let b = self.cell_start[ci + 1] as usize;
                    for &i in &self.cell_nodes[a..b] {
                        let np = self.nodes[i as usize].pos;
                        // Weight vertical error heavily so we do not snap to a
                        // catwalk when standing underneath it.
                        let d = (np.x - p.x).powi(2) + (np.z - p.z).powi(2) + (np.y - p.y).powi(2) * 4.0;
                        if d < best_d { best_d = d; best = Some(i); }
                    }
                }
            }
            if best.is_some() && r >= 1 { break; }
        }
        best
    }

    pub fn node(&self, i: u32) -> &NavNode { &self.nodes[i as usize] }

    pub fn links_of(&self, i: u32) -> &[NavLink] {
        let a = self.link_start[i as usize] as usize;
        let b = self.link_start[i as usize + 1] as usize;
        &self.links[a..b]
    }

    /// Labels every node with its connected component and returns
    /// `(component_of_node, sizes)`. Used by map validation to prove that
    /// spawns and objectives are actually reachable from one another, which is
    /// the one authoring mistake that is invisible until a bot walks into it.
    pub fn components(&self) -> (Vec<u32>, Vec<u32>) {
        let n = self.nodes.len();
        let mut comp = vec![u32::MAX; n];
        let mut sizes: Vec<u32> = Vec::new();
        let mut stack: Vec<u32> = Vec::with_capacity(256);
        for seed in 0..n {
            if comp[seed] != u32::MAX { continue; }
            let id = sizes.len() as u32;
            let mut count = 0u32;
            stack.clear();
            stack.push(seed as u32);
            comp[seed] = id;
            while let Some(cur) = stack.pop() {
                count += 1;
                for l in self.links_of(cur) {
                    if comp[l.to as usize] == u32::MAX {
                        comp[l.to as usize] = id;
                        stack.push(l.to);
                    }
                }
            }
            sizes.push(count);
        }
        (comp, sizes)
    }

    /// Index of the largest connected component, i.e. the main playspace.
    pub fn main_component(&self) -> (Vec<u32>, u32) {
        let (comp, sizes) = self.components();
        let best = sizes.iter().enumerate().max_by_key(|(_, s)| **s).map(|(i, _)| i as u32).unwrap_or(0);
        (comp, best)
    }

    /// Drops every node that cannot be reached on foot from one of `seeds`.
    ///
    /// Baking samples every column in the playspace, which inevitably finds
    /// standable surfaces nobody can get to: building roofs, the tops of
    /// warehouse racking, the ceiling slab itself. Keeping them would let
    /// `nearest` snap a bot onto a roof and would make the connectivity check
    /// meaningless, so they are removed once and for all here.
    pub fn prune_to_reachable(&mut self, seeds: &[Vec3]) {
        if self.nodes.is_empty() { return; }
        let (comp, sizes) = self.components();
        let mut keep = vec![false; sizes.len()];
        for s in seeds {
            if let Some(n) = self.nearest(*s) { keep[comp[n as usize] as usize] = true; }
        }
        if !keep.iter().any(|k| *k) {
            // No seed resolved; fall back to the largest region so a map with
            // badly placed spawns is still navigable rather than empty.
            if let Some((best, _)) = sizes.iter().enumerate().max_by_key(|(_, s)| **s) {
                keep[best] = true;
            }
        }

        let old_nodes = std::mem::take(&mut self.nodes);
        let old_links = std::mem::take(&mut self.links);
        let old_link_start = std::mem::take(&mut self.link_start);
        let old_cell_nodes = std::mem::take(&mut self.cell_nodes);
        let old_cell_start = std::mem::take(&mut self.cell_start);

        let mut remap = vec![u32::MAX; old_nodes.len()];
        let mut nodes = Vec::with_capacity(old_nodes.len());
        for (i, n) in old_nodes.iter().enumerate() {
            if keep[comp[i] as usize] {
                remap[i] = nodes.len() as u32;
                nodes.push(*n);
            }
        }

        // Remap is monotonic in the old index, so link starts stay ordered.
        let mut link_start = vec![0u32; nodes.len() + 1];
        let mut links: Vec<NavLink> = Vec::with_capacity(old_links.len());
        for i in 0..old_nodes.len() {
            let ni = remap[i];
            if ni == u32::MAX { continue; }
            link_start[ni as usize] = links.len() as u32;
            let a = old_link_start[i] as usize;
            let b = old_link_start[i + 1] as usize;
            for l in &old_links[a..b] {
                let t = remap[l.to as usize];
                if t != u32::MAX { links.push(NavLink { to: t, cost: l.cost }); }
            }
        }
        if let Some(last) = link_start.last_mut() { *last = links.len() as u32; }

        let cells = old_cell_start.len().saturating_sub(1);
        let mut cell_start = vec![0u32; cells + 1];
        let mut cell_nodes: Vec<u32> = Vec::with_capacity(old_cell_nodes.len());
        for ci in 0..cells {
            cell_start[ci] = cell_nodes.len() as u32;
            let a = old_cell_start[ci] as usize;
            let b = old_cell_start[ci + 1] as usize;
            for &n in &old_cell_nodes[a..b] {
                let r = remap[n as usize];
                if r != u32::MAX { cell_nodes.push(r); }
            }
        }
        if let Some(last) = cell_start.last_mut() { *last = cell_nodes.len() as u32; }

        self.nodes = nodes;
        self.links = links;
        self.link_start = link_start;
        self.cell_nodes = cell_nodes;
        self.cell_start = cell_start;
    }

    /// A random node, used to seed bot wandering when no objective applies.
    pub fn random_node(&self, rng: &mut crate::core::Rng) -> Option<u32> {
        if self.nodes.is_empty() { return None; }
        Some(rng.below(self.nodes.len() as u32))
    }
}

/// Collects standable surface heights in a world column, highest first.
fn collect_surfaces(world: &CollisionWorld, x: f32, z: f32, y_bottom: f32, y_top: f32, out: &mut Vec<f32>) {
    let column = Aabb::new(
        Vec3::new(x - 0.05, y_bottom, z - 0.05),
        Vec3::new(x + 0.05, y_top, z + 0.05),
    );
    world.grid.query_aabb(&column, |bi| {
        let b = &world.brushes[bi as usize];
        if !b.is_solid() { return; }
        if b.flags.contains(super::brush::BrushFlags::NONAV) { return; }
        if x < b.aabb.min.x || x > b.aabb.max.x || z < b.aabb.min.z || z > b.aabb.max.z { return; }
        let h = b.surface_height(x, z);
        if h < y_bottom || h > y_top { return; }
        // Surfaces within a step of each other are the same walkable layer.
        // Keep the highest: maps routinely lay a thin slab over a base floor
        // (asphalt on sand, ice on snow), and standing on the lower one would
        // put the agent inside the upper slab.
        if let Some(e) = out.iter_mut().find(|e| (**e - h).abs() < LAYER_EPS) {
            if h > *e { *e = h; }
            return;
        }
        out.push(h);
    });
    out.sort_unstable_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
}

/// Can an agent walk directly from one point to another?
///
/// This walks the segment in short increments and, at each one, resolves the
/// ground the way the movement code does: step up to `STEP_UP`, fall up to
/// `MAX_DROP`, and require standing room. Testing it as a walk rather than as
/// a single box sweep is what makes staircases navigable — a sweep at a fixed
/// height is blocked by the very step it is supposed to climb — while still
/// correctly refusing ledges and walls.
fn walkable_between(world: &CollisionWorld, from: Vec3, to: Vec3) -> bool {
    let delta = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
    let dist = delta.length();
    if dist < 1e-4 {
        return (to.y - from.y).abs() <= STEP_UP;
    }
    // One sample every 0.35 m. Together with `STEP_UP` this sets the steepest
    // stair the graph will accept (about 57 degrees), which comfortably covers
    // every staircase the map DSL builds while still refusing sheer walls.
    let samples = ((dist / 0.35).ceil() as i32).clamp(2, 48);
    let mut cur = Vec3::new(from.x, from.y, from.z);
    for i in 1..=samples {
        let t = i as f32 / samples as f32;
        let x = from.x + delta.x * t;
        let z = from.z + delta.z * t;
        // The highest surface we could step onto from here.
        let probe = Vec3::new(x, cur.y + STEP_UP, z);
        let ground = match world.ground_below(probe, AGENT_RADIUS, STEP_UP + MAX_DROP) {
            Some((h, _)) => h,
            None => return false,
        };
        if ground > cur.y + STEP_UP || ground < cur.y - MAX_DROP { return false; }
        let feet = Vec3::new(x, ground + GROUND_CLEARANCE, z);
        if !world.standable(feet, AGENT_RADIUS, AGENT_HEIGHT, STEP_ALLOWANCE) { return false; }
        cur = feet;
    }
    // We must actually arrive at the destination surface, not one above or
    // below it, or the graph would claim links between stacked walkways.
    if (cur.y - to.y).abs() > 0.45 { return false; }

    // A climb is confirmed with the real mover. The stepped walk above is an
    // approximation and a generous one: it accepts anything up to about a
    // 57-degree slope, including plenty a player cannot actually climb. That
    // is how bots came to path up staircases a human wedges on. Navigation
    // claiming ground the player cannot reach is far worse than navigation
    // missing a little, so anything that rises gets checked properly.
    if to.y - from.y > 0.20 && !climbable_by_player(world, from, to) {
        return false;
    }
    true
}

/// Runs the actual player movement code from `from` to `to` and reports
/// whether it arrives. Only called for links that climb, which is a small
/// fraction of the graph, so the cost stays in the noise at map build time.
fn climbable_by_player(world: &CollisionWorld, from: Vec3, to: Vec3) -> bool {
    use crate::game::movement::{self, MoveMods, MoveState};
    use crate::game::types::{Buttons, InputCmd, Stance};

    let flat = Vec3::new(to.x - from.x, 0.0, to.z - from.z);
    let dist = flat.length();
    if dist < 1e-4 { return true; }
    let dir = flat / dist;

    let mut st = MoveState::default();
    st.pos = from + Vec3::Y * 0.02;
    st.height = Stance::Stand.height();
    st.stance = Stance::Stand;
    // A player meets a step already moving; a standing start is a harder test
    // than anything that happens in play.
    st.vel = dir * 5.0;
    st.grounded = true;

    let mods = MoveMods {
        weapon_scale: 1.0, ads_scale: 1.0, perk_scale: 1.0,
        block_sprint: false, want_ads: false, ads_time: 0.25,
    };
    let yaw = crate::math::angles_from_dir(dir).0;
    let dt = 1.0 / 60.0;
    // A second: several times what a metre of stair or ramp takes, but the
    // mover has to accelerate from the node and a ramp is climbed gradually.
    for _ in 0..60 {
        let cmd = InputCmd {
            seq: 0, dt_ms: 16, move_f: 127, move_r: 0,
            yaw, pitch: 0.0, buttons: Buttons::empty(), weapon: 0xFF,
        };
        movement::move_player(&mut st, &cmd, &mods, world, dt);
        let flat_err = Vec3::new(st.pos.x - to.x, 0.0, st.pos.z - to.z).length();
        if flat_err < 0.45 && (st.pos.y - to.y).abs() < 0.35 { return true; }
    }
    false
}

/// Reusable A* workspace. One of these per bot avoids all path allocation in
/// the steady state, which matters when a dozen bots repath under fire.
pub struct PathFinder {
    /// Visit stamps let us skip clearing the score arrays between searches.
    stamp: Vec<u32>,
    generation: u32,
    g: Vec<f32>,
    came: Vec<u32>,
    open: Vec<(f32, u32)>,
    pub path: Vec<Vec3>,
}

impl PathFinder {
    pub fn new() -> PathFinder {
        PathFinder {
            stamp: Vec::new(), generation: 0, g: Vec::new(), came: Vec::new(),
            open: Vec::with_capacity(256), path: Vec::with_capacity(64),
        }
    }

    fn ensure(&mut self, n: usize) {
        if self.stamp.len() < n {
            self.stamp.resize(n, 0);
            self.g.resize(n, 0.0);
            self.came.resize(n, u32::MAX);
        }
    }

    /// A* from `start` to `goal`, writing a smoothed world-space path into
    /// `self.path`. `budget` caps expanded nodes so a bot can never stall the
    /// server tick on an unreachable goal.
    pub fn find(&mut self, grid: &NavGrid, world: &CollisionWorld, start: Vec3, goal: Vec3, budget: u32) -> bool {
        self.path.clear();
        let (s, e) = match (grid.nearest(start), grid.nearest(goal)) {
            (Some(a), Some(b)) => (a, b),
            _ => return false,
        };
        if s == e {
            self.path.push(grid.node(e).pos);
            return true;
        }
        self.ensure(grid.nodes.len());
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            // Wrapped: clear once so stale stamps cannot alias.
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.generation = 1;
        }
        let gen = self.generation;
        let goal_pos = grid.node(e).pos;

        self.open.clear();
        self.stamp[s as usize] = gen;
        self.g[s as usize] = 0.0;
        self.came[s as usize] = u32::MAX;
        self.open.push((heuristic(grid.node(s).pos, goal_pos), s));

        let mut expanded = 0u32;
        let mut found = false;
        while let Some(idx) = pop_min(&mut self.open) {
            if idx == e { found = true; break; }
            expanded += 1;
            if expanded > budget { break; }
            let g_cur = self.g[idx as usize];
            let pos = grid.node(idx).pos;
            for l in grid.links_of(idx) {
                let ti = l.to as usize;
                let ng = g_cur + l.cost;
                if self.stamp[ti] == gen && self.g[ti] <= ng { continue; }
                self.stamp[ti] = gen;
                self.g[ti] = ng;
                self.came[ti] = idx;
                let f = ng + heuristic(grid.node(l.to).pos, goal_pos);
                self.open.push((f, l.to));
            }
            let _ = pos;
        }

        if !found { return false; }

        // Reconstruct, then string-pull: skipping waypoints we can walk
        // straight past turns the lattice path into something that looks
        // deliberate rather than like a bot following graph paper.
        let mut rev: Vec<Vec3> = Vec::with_capacity(48);
        let mut cur = e;
        let mut guard = 0;
        loop {
            rev.push(grid.node(cur).pos);
            let prev = self.came[cur as usize];
            if prev == u32::MAX { break; }
            cur = prev;
            guard += 1;
            if guard > 4096 { break; }
        }
        rev.reverse();

        self.path.clear();
        let mut anchor = 0usize;
        self.path.push(rev[0]);
        while anchor < rev.len() - 1 {
            let mut furthest = anchor + 1;
            for probe in (anchor + 2)..rev.len() {
                // Only straighten across roughly level ground; smoothing over
                // a staircase would cut the corner and walk a bot into it.
                if (rev[probe].y - rev[anchor].y).abs() < STEP_UP
                    && walkable_between(world, rev[anchor], rev[probe])
                {
                    furthest = probe;
                } else {
                    break;
                }
            }
            self.path.push(rev[furthest]);
            anchor = furthest;
        }
        true
    }
}

impl Default for PathFinder {
    fn default() -> Self { Self::new() }
}

#[inline]
fn heuristic(a: Vec3, b: Vec3) -> f32 {
    // Slightly overestimating vertical distance keeps A* from wandering up and
    // down stairs looking for shortcuts that do not exist.
    let d = b - a;
    (d.x * d.x + d.z * d.z).sqrt() + d.y.abs() * 1.5
}

/// Linear-scan minimum extraction. The open set here is small (tens of
/// entries on these map sizes) so this beats a binary heap in practice and
/// needs no ordering wrapper for floats.
#[inline]
fn pop_min(open: &mut Vec<(f32, u32)>) -> Option<u32> {
    if open.is_empty() { return None; }
    let mut best = 0usize;
    for i in 1..open.len() {
        if open[i].0 < open[best].0 { best = i; }
    }
    Some(open.swap_remove(best).1)
}

// Navigation must never claim ground the player cannot stand on.
const _: () = assert!(AGENT_RADIUS >= crate::game::movement::tune::RADIUS);
