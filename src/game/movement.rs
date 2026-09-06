//! Player movement.
//!
//! This is the single most important piece of code in the game for how it
//! feels, and the single most important piece for whether multiplayer works:
//! the server and every client run this exact function on the exact same
//! inputs, so it must be deterministic, allocation-free and free of any
//! dependence on frame rate or wall-clock time.

use crate::core::clampf;
use crate::game::types::{Buttons, InputCmd, Stance};
use crate::maps::brush::{CollisionWorld, TraceMask};
use crate::maps::nav;
use crate::math::{clip_velocity, depenetrate, move_basis, Aabb};
use glam::Vec3;

/// Movement tuning. Deliberately faster and more forgiving than a modern
/// tactical shooter: the era's shooters were arcade games and the maps here
/// are built around covering ground quickly.
pub mod tune {
    /// Half-width of the player collision box.
    pub const RADIUS: f32 = 0.34;
    /// Ground speed with no modifiers, metres per second.
    pub const WALK_SPEED: f32 = 5.5;
    /// Speed while sprinting.
    pub const SPRINT_SPEED: f32 = 8.3;
    /// Ground acceleration. High enough that direction changes are instant.
    pub const ACCEL: f32 = 95.0;
    /// Air acceleration; low, but non-zero so you can steer a jump.
    pub const AIR_ACCEL: f32 = 26.0;
    /// Maximum speed the air accelerate step will add toward the wish
    /// direction. This is what makes strafing in the air feel controllable
    /// without turning into a movement exploit.
    pub const AIR_CAP: f32 = 2.4;
    /// Ground friction, as a rate.
    pub const FRICTION: f32 = 9.0;
    /// Below this speed friction stops the player outright.
    pub const STOP_SPEED: f32 = 1.6;
    pub const GRAVITY: f32 = 21.0;
    /// Initial upward velocity of a jump: about 1.35 m of clearance.
    pub const JUMP_SPEED: f32 = 5.4;
    /// Terminal fall speed, so a long drop is survivable and predictable.
    pub const MAX_FALL: f32 = 42.0;
    /// Tallest ledge that can be walked up without jumping.
    pub const STEP_HEIGHT: f32 = 0.60;
    /// How fast the collision box grows and shrinks between stances.
    pub const STANCE_RATE: f32 = 5.0;
    /// Time to reach full sprint speed, and to drop out of it.
    pub const SPRINT_RATE: f32 = 6.0;
    /// Impact speed above which a landing hurts, and the speed at which it
    /// kills outright.
    pub const FALL_SAFE: f32 = 15.0;
    pub const FALL_LETHAL: f32 = 34.0;
    /// Distance travelled between footsteps at walking pace.
    pub const STRIDE: f32 = 2.05;
    /// Speed you must exceed for a landing to count as "hard".
    pub const LAND_HARD: f32 = 8.0;
    /// Vertical speed applied when stepping off a ledge, to avoid float.
    pub const LADDER_SPEED: f32 = 3.4;
    /// How fast an airborne player can turn the direction they are already
    /// travelling in, in radians per second.
    ///
    /// Quake-style air acceleration alone gives almost no control over a short
    /// hop: holding forward while already at walking pace adds nothing at all
    /// (the wish speed is below the current speed), so the only thing a jump
    /// responds to is a sideways press, and then only by `AIR_CAP`. Over the
    /// half second a jump lasts that is a few centimetres of steering, which
    /// is why jumping felt like being fired out of a tube. This turns the
    /// velocity toward the wish direction instead of adding to it, so a player
    /// can aim a jump without any of it becoming free speed.
    pub const AIR_STEER: f32 = 3.6;
    /// Speed below which air steering does nothing, to avoid spinning a
    /// player who is essentially falling straight down.
    pub const AIR_STEER_MIN: f32 = 0.8;
    /// Fastest an updraft will carry a player upward. Without a ceiling on it
    /// a tall shaft fires people through the roof of the level.
    pub const MAX_LIFT: f32 = 11.0;
}

/// The part of a player's state that movement owns. This is exactly what the
/// client predicts and what the server reconciles, so it is kept small.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MoveState {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Stance the player is transitioning toward.
    pub stance: Stance,
    /// Current collision height; lerps toward `stance.height()`.
    pub height: f32,
    pub grounded: bool,
    /// 0 = not sprinting, 1 = fully sprinting. Blended so speed changes read.
    pub sprint_t: f32,
    /// 0 = hip fire, 1 = fully aimed.
    pub ads_t: f32,
    /// Brush index the player is standing on, for footstep materials.
    pub ground_brush: u32,
    /// Distance accumulator driving footsteps.
    pub stride: f32,
    /// Set while the jump button is held so a held jump does not auto-bunny-hop.
    pub jump_latched: bool,
    /// Downward speed at the moment of the last landing.
    pub land_speed: f32,
}

impl Default for MoveState {
    fn default() -> Self {
        MoveState {
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            stance: Stance::Stand,
            height: Stance::Stand.height(),
            grounded: false,
            sprint_t: 0.0,
            ads_t: 0.0,
            ground_brush: u32::MAX,
            stride: 0.0,
            jump_latched: false,
            land_speed: 0.0,
        }
    }
}

impl MoveState {
    #[inline]
    pub fn body(&self) -> Aabb {
        Aabb::from_base(self.pos, tune::RADIUS, self.height)
    }

    /// Eye position, derived from the current blended height so the camera
    /// moves smoothly through a crouch without any client-only state.
    #[inline]
    pub fn eye(&self) -> Vec3 {
        self.pos + Vec3::Y * (self.height * 0.910)
    }

    #[inline]
    pub fn horizontal_speed(&self) -> f32 {
        (self.vel.x * self.vel.x + self.vel.z * self.vel.z).sqrt()
    }

    /// Speed as a fraction of the current maximum, used for weapon spread.
    #[inline]
    pub fn speed_fraction(&self, max: f32) -> f32 {
        if max <= 0.01 { 0.0 } else { (self.horizontal_speed() / max).clamp(0.0, 1.0) }
    }
}

/// Per-move modifiers coming from the player's weapon and condition.
#[derive(Copy, Clone, Debug)]
pub struct MoveMods {
    /// Weapon movement multiplier.
    pub weapon_scale: f32,
    /// Weapon movement multiplier while aimed.
    pub ads_scale: f32,
    /// Perk or pickup multiplier.
    pub perk_scale: f32,
    /// True when the player may not sprint (firing, reloading, dead).
    pub block_sprint: bool,
    /// True when aiming down sights is being held.
    pub want_ads: bool,
    /// Seconds to reach full aim for the held weapon.
    pub ads_time: f32,
}

impl Default for MoveMods {
    fn default() -> Self {
        MoveMods {
            weapon_scale: 1.0, ads_scale: 0.55, perk_scale: 1.0,
            block_sprint: false, want_ads: false, ads_time: 0.24,
        }
    }
}

/// Things that happened during a move, for audio, effects and damage.
#[derive(Copy, Clone, Debug, Default)]
pub struct MoveEvents {
    /// Impact speed if the player landed this tick.
    pub landed: Option<f32>,
    pub footstep: bool,
    /// Brush the footstep or landing happened on.
    pub surface_brush: u32,
    pub jumped: bool,
    /// True if the player was pushed out of geometry, which should never
    /// happen in normal play and is worth counting.
    pub depenetrated: bool,
    /// Fall damage to apply, already scaled.
    pub fall_damage: f32,
    /// Set on the tick a player came through a warp. Everything that smooths
    /// a player's position between frames has to be told, or they are drawn
    /// sliding across the level at the speed of light.
    pub warped: bool,
}

/// Advances one player by one command. Pure: identical inputs give identical
/// outputs on the server and on every client.
pub fn move_player(
    st: &mut MoveState,
    cmd: &InputCmd,
    mods: &MoveMods,
    world: &CollisionWorld,
    dt: f32,
) -> MoveEvents {
    let mut ev = MoveEvents::default();
    if dt <= 0.0 { return ev; }

    st.yaw = cmd.yaw;
    st.pitch = cmd.pitch;

    // ---------------------------------------------------------- stance
    let wants_prone = cmd.held(Buttons::PRONE);
    let wants_crouch = cmd.held(Buttons::CROUCH);
    let target = if wants_prone { Stance::Prone } else if wants_crouch { Stance::Crouch } else { Stance::Stand };

    // Standing up requires the room to do it; otherwise we stay low.
    let target = if target.height() > st.stance.height() && !has_headroom(st, world, target.height()) {
        st.stance
    } else {
        target
    };
    st.stance = target;
    let goal_h = target.height();
    if (st.height - goal_h).abs() > 0.001 {
        let rate = tune::STANCE_RATE * dt * 1.78;
        st.height += clampf(goal_h - st.height, -rate, rate);
    } else {
        st.height = goal_h;
    }

    // ------------------------------------------------------------- aim
    let ads_rate = if mods.ads_time > 0.01 { dt / mods.ads_time } else { 1.0 };
    st.ads_t = if mods.want_ads {
        (st.ads_t + ads_rate).min(1.0)
    } else {
        (st.ads_t - ads_rate * 1.4).max(0.0)
    };

    // ---------------------------------------------------------- sprint
    let forward_input = cmd.forward();
    let wants_sprint = cmd.held(Buttons::SPRINT)
        && forward_input > 0.25
        && !mods.block_sprint
        && !mods.want_ads
        && st.stance == Stance::Stand
        && st.ads_t < 0.05;
    let sprint_delta = tune::SPRINT_RATE * dt;
    st.sprint_t = if wants_sprint {
        (st.sprint_t + sprint_delta).min(1.0)
    } else {
        (st.sprint_t - sprint_delta * 1.8).max(0.0)
    };

    // ------------------------------------------------------------- field
    // Gravity is a property of where the player is standing, not a constant.
    let field = world.field_at(st.pos + Vec3::Y * st.height * 0.5);

    // ----------------------------------------------------- target speed
    let base = tune::WALK_SPEED + (tune::SPRINT_SPEED - tune::WALK_SPEED) * st.sprint_t;
    let ads_scale = 1.0 + (mods.ads_scale - 1.0) * st.ads_t;
    let max_speed = base
        * st.stance.speed_mult()
        * mods.weapon_scale
        * mods.perk_scale
        * ads_scale;

    // ------------------------------------------------------- wish direction
    let (fwd, right) = move_basis(st.yaw);
    let mut wish = fwd * forward_input + right * cmd.strafe();
    let wish_len = wish.length();
    if wish_len > 1.0 { wish /= wish_len; }
    let wish_speed = max_speed * wish_len.min(1.0);

    // -------------------------------------------------------------- jump
    let jump_pressed = cmd.held(Buttons::JUMP);
    if !jump_pressed { st.jump_latched = false; }
    if jump_pressed && !st.jump_latched && st.grounded && st.stance == Stance::Stand {
        st.vel.y = tune::JUMP_SPEED;
        st.grounded = false;
        st.jump_latched = true;
        ev.jumped = true;
    }

    // ----------------------------------------------------------- friction
    if st.grounded {
        let speed = st.horizontal_speed();
        if speed > 0.0 {
            let control = speed.max(tune::STOP_SPEED);
            let drop = control * tune::FRICTION * dt;
            let scale = ((speed - drop) / speed).max(0.0);
            st.vel.x *= scale;
            st.vel.z *= scale;
        }
    }

    // ------------------------------------------------------- acceleration
    if st.grounded {
        accelerate(&mut st.vel, wish, wish_speed, tune::ACCEL, dt);
    } else {
        // Airborne: cap how much speed can be added toward the wish direction.
        let capped = wish_speed.min(tune::AIR_CAP);
        accelerate(&mut st.vel, wish, capped, tune::AIR_ACCEL, dt);
        air_steer(&mut st.vel, wish, dt);
        st.vel.y -= tune::GRAVITY * field.gravity * dt;
        // Terminal velocity scales with the local gravity, so a low-gravity
        // shaft is a drift rather than a slow fall that suddenly is not.
        let terminal = tune::MAX_FALL * field.gravity.max(0.15);
        if st.vel.y < -terminal { st.vel.y = -terminal; }
    }

    // An updraft acts on a player standing in it as well as one already off
    // the floor, or there would be no way into one from the ground. The
    // ground probe lets go on its own once the rise passes its threshold.
    if field.lift != 0.0 {
        st.vel.y += field.lift * dt;
        if st.vel.y > tune::MAX_LIFT { st.vel.y = tune::MAX_LIFT; }
    }

    // ---------------------------------------------------------- integrate
    let was_grounded = st.grounded;
    let fall_speed = -st.vel.y;
    let start = st.pos;
    let travelled = slide_move(st, world, dt);
    // Resolve again after moving. A sweep can leave a hair of overlap when a
    // mover slides into a corner, and letting that persist into the next tick
    // is how players end up inside walls.
    ev.depenetrated = resolve_penetration(st, world);

    // Ride a ramp that has risen underneath us.
    //
    // Ramps are height fields, not boxes: the horizontal sweep passes through
    // them and the ground probe only ever looks down. Walking *up* one, the
    // surface climbed out of the probe window within a few frames and the
    // player fell through the ramp onto whatever was below it. Boxes and
    // stairs are handled by the step-up path; this is the ramp equivalent.
    if was_grounded && st.vel.y <= 0.1 {
        if let Some((h, brush)) = world.ramp_surface(st.pos, tune::RADIUS, tune::STEP_HEIGHT) {
            let rise = h - st.pos.y;
            if rise > 0.0 && rise <= tune::STEP_HEIGHT {
                let lifted = Vec3::new(st.pos.x, h, st.pos.z);
                let body = Aabb::from_base(lifted, tune::RADIUS - STEP_SKIN, st.height);
                if !world.box_blocked(&body, TraceMask::Solid) {
                    st.pos.y = h;
                    st.vel.y = 0.0;
                    st.ground_brush = brush;
                }
            }
        }
    }

    // Ground probe. Doing this after the move keeps `grounded` consistent with
    // the position we actually ended at.
    let (mut grounded, mut ground_brush) = probe_ground(st, world);

    // Step down off a ledge no taller than one we could step up.
    //
    // Without this, walking *down* a staircase is a series of small falls: the
    // player leaves each tread, becomes airborne for a few frames, loses
    // ground friction and footstep cadence, and lands on the next one. It
    // reads as bouncing down the stairs, and it is why they were as awkward
    // going down as they were going up. Snapping onto the surface below keeps
    // the descent continuous, and the height limit is the same `STEP_HEIGHT`
    // that governs the climb, so the two stay symmetrical.
    //
    // It runs only when the player has actually left the floor they were on.
    // Applying it to a grounded mover as well pulls players who are partway up
    // a step back down onto the tread below, which cost the navigation graph
    // three hundred rising links and doubled the number of staircases the
    // route walker wedged on.
    if was_grounded && !grounded && !ev.jumped {
        snap_to_ground(st, world);
        let probed = probe_ground(st, world);
        grounded = probed.0;
        ground_brush = probed.1;
    }
    st.grounded = grounded;
    st.ground_brush = ground_brush;
    if grounded && st.vel.y < 0.0 { st.vel.y = 0.0; }

    if grounded && !was_grounded {
        st.land_speed = fall_speed;
        ev.landed = Some(fall_speed);
        ev.surface_brush = ground_brush;
        if fall_speed > tune::FALL_SAFE {
            let t = (fall_speed - tune::FALL_SAFE) / (tune::FALL_LETHAL - tune::FALL_SAFE);
            ev.fall_damage = (t.clamp(0.0, 1.0) * 100.0).min(100.0);
        }
    }

    // ----------------------------------------------------------- footsteps
    if st.grounded && travelled > 0.0 {
        // Faster movement means a shorter stride, so footstep cadence tracks
        // speed the way a listener expects.
        st.stride += travelled;
        let stride_len = tune::STRIDE * (0.55 + 0.45 * st.stance.speed_mult());
        if st.stride >= stride_len {
            st.stride -= stride_len;
            ev.footstep = true;
            ev.surface_brush = ground_brush;
        }
    } else if !st.grounded {
        st.stride = tune::STRIDE * 0.5;
    }

    // ------------------------------------------------------------- warps
    //
    // Taken last, on the position the player actually finished the tick at,
    // so that arriving is never entangled with the step-up or ground probe
    // that got them there. Speed and heading are carried through untouched.
    //
    // A warp is authored so that its exit is not inside another mouth - the
    // map audit checks it - which is what stops a player ping-ponging.
    if let Some(w) = world.warp_at(st.pos) {
        st.pos = w.landing(st.vel);
        resolve_penetration(st, world);
        let (g, gb) = probe_ground(st, world);
        st.grounded = g;
        st.ground_brush = gb;
        if g && st.vel.y < 0.0 { st.vel.y = 0.0; }
        ev.warped = true;
        // Not a landing, however far the exit is above the floor: the fall
        // that would have hurt happened somewhere the player is no longer.
        ev.landed = None;
        ev.fall_damage = 0.0;
    }

    // Numerical hygiene: never let a NaN escape into the network stream.
    if !st.pos.is_finite() { st.pos = start; st.vel = Vec3::ZERO; }
    if !st.vel.is_finite() { st.vel = Vec3::ZERO; }

    ev
}

/// Quake-style accelerate: adds speed toward `wish` only up to `wish_speed`.
/// This is what gives the movement its responsive, slightly slidey character.
#[inline]
fn accelerate(vel: &mut Vec3, wish: Vec3, wish_speed: f32, accel: f32, dt: f32) {
    if wish_speed <= 0.0 { return; }
    let current = vel.dot(wish);
    let add = wish_speed - current;
    if add <= 0.0 { return; }
    let mut accel_speed = accel * wish_speed * dt;
    if accel_speed > add { accel_speed = add; }
    *vel += wish * accel_speed;
}

/// Turns an airborne player's horizontal velocity toward where they are
/// steering, without changing how fast they are going.
///
/// Speed is preserved exactly, so this cannot be used to build speed the way
/// air acceleration can; it only decides which way the momentum points. The
/// rotation is capped per second rather than applied as a blend, so the turn
/// rate is the same whether the player is drifting or falling at terminal
/// velocity.
#[inline]
fn air_steer(vel: &mut Vec3, wish: Vec3, dt: f32) {
    let horiz = Vec3::new(vel.x, 0.0, vel.z);
    let speed = horiz.length();
    if speed < tune::AIR_STEER_MIN { return; }
    let want = Vec3::new(wish.x, 0.0, wish.z);
    let want_len = want.length();
    if want_len < 0.01 { return; }
    let (dir, target) = (horiz / speed, want / want_len);
    let dot = dir.dot(target).clamp(-1.0, 1.0);
    let angle = dot.acos();
    let step = (tune::AIR_STEER * dt).min(angle);
    if step < 1e-5 { return; }
    // The component of the target perpendicular to the current heading. At a
    // dead 180 there is none, so a player holding straight back keeps going
    // until a sideways press gives the turn something to rotate about.
    let perp = (target - dir * dot).normalize_or_zero();
    if perp == Vec3::ZERO { return; }
    let turned = dir * step.cos() + perp * step.sin();
    vel.x = turned.x * speed;
    vel.z = turned.z * speed;
}

/// True if the player could stand up to `height` where they are.
fn has_headroom(st: &MoveState, world: &CollisionWorld, height: f32) -> bool {
    let probe = Aabb::from_base(st.pos, tune::RADIUS - 0.02, height);
    !world.box_blocked(&probe, TraceMask::Solid)
}

/// Moves the player, sliding along surfaces and stepping up small ledges.
/// Returns the horizontal distance actually covered.
fn slide_move(st: &mut MoveState, world: &CollisionWorld, dt: f32) -> f32 {
    let start = st.pos;

    // Push out of anything we are already inside. This should be rare, but a
    // stuck player is unrecoverable so it is always worth the check.
    resolve_penetration(st, world);

    let mut remaining = st.vel * dt;
    let mut planes: [Vec3; 4] = [Vec3::ZERO; 4];
    let mut plane_count = 0usize;

    for _ in 0..5 {
        if remaining.length_squared() < 1e-10 { break; }
        let body = st.body();
        let hit = world.trace_box(&body, remaining, TraceMask::Solid);
        if !hit.hit {
            st.pos += remaining;
            break;
        }

        // Move up to the contact, leaving a small gap so we never rest exactly
        // on a surface plane (which would make the next trace start inside it).
        let advance = remaining * hit.fraction;
        st.pos += advance + hit.normal * 0.002;
        remaining -= advance;

        // Try to walk over the obstruction rather than stopping dead against
        // it. Only for near-vertical surfaces: we do not want to "step" up a
        // ceiling or a floor.
        if hit.normal.y.abs() < 0.45 && try_step_up(st, world, &mut remaining) {
            continue;
        }

        // Clip velocity and the remaining motion against every plane we have
        // touched, so an inside corner stops us instead of ejecting us.
        if plane_count < planes.len() {
            planes[plane_count] = hit.normal;
            plane_count += 1;
        }
        for p in planes.iter().take(plane_count) {
            remaining = clip_velocity(remaining, *p, 1.0);
            st.vel = clip_velocity(st.vel, *p, 1.0);
        }
        if remaining.length_squared() < 1e-10 { break; }
    }

    let d = st.pos - start;
    (d.x * d.x + d.z * d.z).sqrt()
}

/// How much narrower the step-up traces are than the player. Two centimetres
/// is enough to stop a wall the player is merely brushing from cancelling the
/// step, and far too small to fit through anything.
const STEP_SKIN: f32 = 0.02;

/// How far ahead the step-across trace looks, however little motion is left in
/// the frame. A shade under half a stair tread.
const STEP_PROBE: f32 = 0.12;

/// Attempts to walk up a ledge no taller than `STEP_HEIGHT`.
///
/// Lift, move across, drop back down. Accepting the step only when the mover
/// actually gains ground is what stops a player from being levitated up the
/// face of a wall they are merely leaning on.
fn try_step_up(st: &mut MoveState, world: &CollisionWorld, remaining: &mut Vec3) -> bool {
    // Stepping while rising out of a jump would let players climb walls.
    if !st.grounded && st.vel.y > 0.5 { return false; }

    let horiz = Vec3::new(remaining.x, 0.0, remaining.z);
    let horiz_len = horiz.length();
    if horiz_len < 1e-4 { return false; }

    let saved_pos = st.pos;

    // The step traces use a box a hair narrower than the player. Stairs are
    // routinely built hard against a wall with a centimetre to spare, and a
    // full-width trace then reports an instant hit on the wall it is sliding
    // along, so the step is refused and the player sticks halfway up a
    // staircase with the key held down. The final position is still validated
    // at full width below, so this cannot push anyone into geometry.
    let slim = |st: &MoveState| Aabb::from_base(st.pos, tune::RADIUS - STEP_SKIN, st.height);

    // 1. Lift, but no further than there is room for.
    let up_hit = world.trace_box(&slim(st), Vec3::Y * tune::STEP_HEIGHT, TraceMask::Solid);
    let lift = tune::STEP_HEIGHT * up_hit.fraction - 0.004;
    if lift < 0.06 { return false; }
    st.pos.y += lift;

    // 2. Move across at the raised height.
    //
    // The trace probes at least `STEP_PROBE` ahead even when barely any motion
    // is left, but the mover is only ever advanced by the motion it actually
    // had. A player walking into a riser usually arrives with a millimetre of
    // the frame's movement left over, and testing only that millimetre says
    // nothing about whether the tread is clear - so the step was refused, the
    // velocity was clipped against the riser, and the player stopped dead for
    // a frame before starting again. On a staircase that happens on every
    // tread, which is what made stairs feel like walking through treacle.
    let dir = horiz / horiz_len;
    let probe = dir * horiz_len.max(STEP_PROBE);
    let across = world.trace_box(&slim(st), probe, TraceMask::Solid);
    let advance = (probe.length() * across.fraction).min(horiz_len);
    if advance < 1e-4 {
        st.pos = saved_pos;
        return false;
    }
    let gained = dir * advance;
    st.pos += gained;

    // 3. Settle back down onto whatever is under us, leaving the same two
    // millimetres of clearance every other contact in this file leaves.
    //
    // Without it the mover comes to rest exactly flush on the tread it just
    // climbed, and the full-width validation below - which is a strict overlap
    // test - decides the destination is blocked and refuses the step.
    //
    // The surface is found with the same downward query the ground probe uses
    // rather than a swept box, and at the player's real width. A swept box has
    // to be narrower than the player to get up a staircase built against a
    // wall, and a mover that has crossed the lip of a tread by less than that
    // margin then lands beside the tread instead of on it - two centimetres
    // inside the riser, where the full-width check refuses the step. That is
    // frame-rate dependent, because the distance a mover crosses the lip by is
    // whatever was left of its motion when it reached the riser: at 60 Hz it
    // clears the margin and climbs, and at 144 it does not. Walking into a
    // staircase at an angle on a fast machine, the player slid along the
    // bottom step for as long as they held the key. A downward surface query
    // does not care how much of the box is over the tread, so the step lands
    // where the mover actually is.
    let drop = lift + 0.02;
    let feet_y = st.pos.y;
    match world.ground_below(st.pos + Vec3::Y * 0.02, tune::RADIUS, drop) {
        Some((h, _)) if h <= feet_y + 0.02 => st.pos.y = h + 0.002,
        // Nothing underneath. Walking, that means the step led off a ledge and
        // is refused; falling, the mover is allowed to clear the lip and keep
        // dropping, which is what lets a player scramble onto a low roof.
        _ => {
            if st.grounded {
                st.pos = saved_pos;
                return false;
            }
            st.pos.y = feet_y - drop;
        }
    }
    if world.box_blocked(&st.body(), TraceMask::Solid) {
        st.pos = saved_pos;
        return false;
    }

    *remaining -= gained;
    st.grounded = true;
    true
}

/// Pushes the player out of any geometry they are intersecting.
fn resolve_penetration(st: &mut MoveState, world: &CollisionWorld) -> bool {
    let mut moved = false;
    for _ in 0..3 {
        let body = Aabb::from_base(st.pos, tune::RADIUS, st.height);
        let mut push = Vec3::ZERO;
        let mut worst = 0.0f32;
        world.grid.query_aabb(&body, |bi| {
            if world.is_destroyed(bi) { return; }
            let b = &world.brushes[bi as usize];
            if !b.is_solid() { return; }
            let solid = b.sweep_box();
            if !solid.overlaps(&body) { return; }
            // A clipped brush occupies only part of its bounding box. Pushing
            // out of the whole box shoves players out of the empty corner
            // beside an angled wall, which reads as being stuck on nothing.
            if !b.footprint_overlaps(body.center(), body.half()) { return; }
            let mtv = depenetrate(&body, &solid);
            let len = mtv.length();
            if len > worst { worst = len; push = mtv; }
        });
        if worst < 1e-4 { break; }
        // Push clear with a real margin, not just out to the surface: landing
        // exactly flush leaves the next sweep unable to tell contact from
        // penetration.
        let dir = push / worst;
        st.pos += dir * (worst + 0.006);
        moved = true;
    }
    moved
}

/// Finds the surface under the player and whether they are standing on it.
fn probe_ground(st: &MoveState, world: &CollisionWorld) -> (bool, u32) {
    // A small downward tolerance keeps the player attached while walking over
    // the seams between adjacent brushes.
    let tolerance = if st.vel.y > 0.5 { 0.0 } else { 0.12 };
    if tolerance <= 0.0 { return (false, u32::MAX); }
    match world.ground_below(st.pos + Vec3::Y * 0.02, tune::RADIUS, tolerance + 0.06) {
        Some((h, brush)) if st.pos.y - h <= tolerance => (true, brush),
        _ => (false, u32::MAX),
    }
}

/// Snaps the player down onto the surface when they walk off a small lip,
/// so running over a kerb - or down a staircase - does not launch them into a
/// fall.
pub fn snap_to_ground(st: &mut MoveState, world: &CollisionWorld) {
    if st.vel.y > 0.1 { return; }
    let feet = st.pos + Vec3::Y * 0.02;
    let Some((h, brush)) = world.ground_below(feet, tune::RADIUS, tune::STEP_HEIGHT + 0.02) else { return };
    let drop = st.pos.y - h;
    // Already standing on it, or too far down to be a step.
    if drop <= 0.02 || drop > tune::STEP_HEIGHT { return; }
    // Never snap into something. The narrow box is the same one the step-up
    // path uses, for the same reason: a staircase built hard against a wall
    // must not refuse the last centimetre.
    let landed = Aabb::from_base(Vec3::new(st.pos.x, h, st.pos.z),
                                 tune::RADIUS - STEP_SKIN, st.height);
    if world.box_blocked(&landed, TraceMask::Solid) { return; }
    st.pos.y = h;
    st.vel.y = 0.0;
    st.ground_brush = brush;
}

/// Keeps a player inside the playable volume. The server calls this after
/// every move: a client that manages to get outside the map is teleported
/// back rather than trusted.
pub fn clamp_to_bounds(st: &mut MoveState, bounds: &Aabb) -> bool {
    let p = st.pos;
    let clamped = Vec3::new(
        clampf(p.x, bounds.min.x + 0.5, bounds.max.x - 0.5),
        clampf(p.y, bounds.min.y - 8.0, bounds.max.y + 40.0),
        clampf(p.z, bounds.min.z + 0.5, bounds.max.z - 0.5),
    );
    if clamped != p {
        st.pos = clamped;
        st.vel = Vec3::ZERO;
        return true;
    }
    false
}

/// True if the player has fallen out of the world and should be killed.
#[inline]
pub fn out_of_world(st: &MoveState, bounds: &Aabb) -> bool {
    st.pos.y < bounds.min.y - 6.0
}

/// Applies weapon recoil to a view angle pair, used identically by the client
/// (for the local view) and by bots (so their aim climbs too).
pub struct RecoilState {
    pub pitch_kick: f32,
    pub yaw_kick: f32,
    /// The portion of the kick that will not spring back.
    pub pitch_perm: f32,
    pub yaw_perm: f32,
    /// How many shots into the current burst we are. This is what makes
    /// recoil a pattern rather than a random walk.
    pub shot: u16,
    /// Seconds since the last shot, for deciding when a burst has ended.
    pub since_shot: f32,
}

impl Default for RecoilState {
    fn default() -> Self {
        RecoilState {
            pitch_kick: 0.0, yaw_kick: 0.0, pitch_perm: 0.0, yaw_perm: 0.0,
            shot: 0, since_shot: 99.0,
        }
    }
}

/// The recoil pattern for one weapon, as a function of shot number.
///
/// Deterministic, because a pattern you cannot learn is just a penalty. The
/// shape is the one every automatic weapon in this genre has: the first few
/// shots climb almost vertically while the gun is still controllable, then the
/// climb flattens and the muzzle walks sideways, reversing once so the tail of
/// a long burst is the part you have to fight. Each weapon gets its own
/// horizontal phase and bias from its identity, so two rifles with the same
/// statistics still spray differently and are worth learning separately.
pub fn recoil_pattern(def: &crate::game::weapons::WeaponDef, shot: u16) -> (f32, f32) {
    let n = shot as f32;
    // Vertical: strong at first, easing to a plateau.
    let climb = 1.0 - (-(n + 1.0) * 0.42).exp();
    let vertical = def.recoil_up * (1.35 - 0.55 * climb);

    // Horizontal: two sine terms at incommensurate rates, seeded per weapon,
    // which reads as a deliberate hand-drawn pattern rather than a wobble.
    let seed = def.id as u32;
    let phase = (seed % 7) as f32 * 0.9;
    let bias = if seed % 2 == 0 { 1.0 } else { -1.0 };
    let walk = ((n * 0.55 + phase).sin() * 0.7 + (n * 0.23 + phase * 0.5).sin() * 0.5) * bias;
    // The first two shots barely move sideways: tapping is precise, holding
    // the trigger is not.
    let horizontal = def.recoil_side * walk * (n / (n + 2.5)) * 2.2;
    (vertical, horizontal)
}

impl RecoilState {
    pub fn kick(&mut self, def: &crate::game::weapons::WeaponDef, rng: &mut crate::core::Rng, ads_t: f32) {
        // Aiming cuts recoil noticeably; that is most of why aiming is worth
        // the movement penalty.
        let scale = 1.0 - 0.35 * ads_t;
        let (up, side) = recoil_pattern(def, self.shot);
        // A little noise on top, so the pattern is learnable but not a rail.
        // Enough to matter at range, not enough to hide the shape.
        let jitter = 0.10 + 0.10 * (1.0 - ads_t);
        let up = up * scale * (1.0 + rng.gaussian() * jitter * 0.5);
        let side = (side + def.recoil_side * rng.gaussian() * jitter) * scale;

        self.pitch_kick += up;
        self.yaw_kick += side;
        self.pitch_perm += up * def.recoil_keep;
        self.yaw_perm += side * def.recoil_keep * 0.5;
        self.shot = self.shot.saturating_add(1);
        self.since_shot = 0.0;
    }

    pub fn update(&mut self, def: &crate::game::weapons::WeaponDef, dt: f32) {
        // A burst ends when the trigger has been off long enough for the hands
        // to reset; the pattern then starts again from the top. Without this
        // the pattern would be a property of the magazine rather than of how
        // you are shooting.
        self.since_shot += dt;
        if self.since_shot > 0.32 { self.shot = 0; }
        let k = (-def.recoil_recover * dt).exp();
        self.pitch_kick = self.pitch_perm + (self.pitch_kick - self.pitch_perm) * k;
        self.yaw_kick = self.yaw_perm + (self.yaw_kick - self.yaw_perm) * k;
        // The permanent component decays much more slowly, so a long burst
        // leaves the muzzle high until the player resets.
        let slow = (-1.6 * dt).exp();
        self.pitch_perm *= slow;
        self.yaw_perm *= slow;
    }

    pub fn reset(&mut self) {
        self.pitch_kick = 0.0;
        self.yaw_kick = 0.0;
        self.pitch_perm = 0.0;
        self.yaw_perm = 0.0;
        self.shot = 0;
        self.since_shot = 99.0;
    }
}

/// Bots and the death camera need to know how far a player can see; sharing
/// the constant keeps AI reaction ranges tied to the map's fog.
pub const MAX_ENGAGE_RANGE: f32 = 160.0;

// Navigation assumes bots can climb `nav::STEP_UP`; the mover must be at
// least that capable or bots will walk into ledges the graph says are fine.
const _: () = assert!(tune::STEP_HEIGHT >= nav::STEP_UP);

#[cfg(test)]
mod stair_tests {
    use super::*;
    use crate::game::types::{Buttons, InputCmd, Stance};
    use crate::maps::brush::RampAxis;
    use crate::maps::build::MapBuilder;
    use crate::maps::MapId;
    use crate::assets::materials::Mat;

    /// Walks a player up a plain staircase and reports how much of their
    /// ground speed survives the climb.
    ///
    /// Every frame spent stopped against a riser is speed the player can feel
    /// going missing, and it is entirely a function of the tick rate: the
    /// faster the client runs, the less of the frame's motion is left over
    /// when it reaches the riser, and the less there is for the step-up to
    /// work with. That is why stairs felt worse the better the machine was.
    fn climb_fraction(hz: f32, angle_deg: f32) -> f32 {
        let mut b = MapBuilder::new(MapId::Junction);
        b.floor(-24.0, -8.0, 48.0, 16.0, 0.0, Mat::Concrete);
        b.stairs(-24.0, 0.0, 0.0, 48.0, 8.0, 4.0, RampAxis::PosZ, Mat::Concrete);
        b.floor(-24.0, 8.0, 48.0, 8.0, 4.0, Mat::Concrete);
        let map = b.finish();

        let mods = MoveMods {
            weapon_scale: 1.0, ads_scale: 1.0, perk_scale: 1.0,
            block_sprint: false, want_ads: false, ads_time: 0.25,
        };
        let dt = 1.0 / hz;
        let yaw = crate::math::angles_from_dir(
            Vec3::new(angle_deg.to_radians().sin(), 0.0, angle_deg.to_radians().cos())).0;

        let mut st = MoveState::default();
        st.pos = Vec3::new(0.0, 0.0, -3.0);
        st.height = Stance::Stand.height();
        st.stance = Stance::Stand;
        st.grounded = true;

        // Run up to the foot of the stairs first, so the climb is measured
        // from a mover already at full speed.
        let cmd = InputCmd { seq: 0, dt_ms: 16, move_f: 127, move_r: 0,
                             yaw, pitch: 0.0, buttons: Buttons::empty(), weapon: 0xFF };
        for _ in 0..(hz as usize) {
            move_player(&mut st, &cmd, &mods, &map.collision, dt);
            if st.pos.z > -0.2 { break; }
        }
        let flat_speed = st.horizontal_speed();
        assert!(flat_speed > 5.0, "did not reach the stairs at speed: {flat_speed}");

        let start = st.pos;
        let mut ticks = 0usize;
        while st.pos.y < 3.9 && ticks < (hz as usize * 8) {
            move_player(&mut st, &cmd, &mods, &map.collision, dt);
            ticks += 1;
            if std::env::var_os("HP_TRACE").is_some() && ticks % 20 == 0 {
                println!("  {hz}Hz/{angle_deg} t{ticks} ({:.2},{:.2},{:.2}) g{}", st.pos.x, st.pos.y, st.pos.z, st.grounded);
            }
        }
        assert!(st.pos.y >= 3.9, "never reached the top: y={:.2} after {ticks} ticks", st.pos.y);
        let travelled = (st.pos.z - start.z).abs();
        let elapsed = ticks as f32 * dt;
        (travelled / elapsed) / flat_speed
    }

    #[test]
    fn stairs_do_not_eat_the_players_speed() {
        for hz in [60.0, 144.0, 240.0] {
            for angle in [0.0, 20.0] {
                let f = climb_fraction(hz, angle);
                assert!(f > 0.80,
                        "at {hz} Hz approaching {angle} degrees off square, only {:.0}% of \
                         walking speed survives the staircase", f * 100.0);
            }
        }
    }
}

#[cfg(test)]
mod warp_tests {
    use super::*;
    use crate::game::types::{Buttons, InputCmd, Stance};
    use crate::maps::MapId;

    /// Walks a player forward along `dir` for up to `secs` and reports where
    /// they ended up, plus whether they came through a warp on the way.
    fn walk(map: &crate::maps::MapData, from: Vec3, dir: Vec3, secs: f32) -> (Vec3, u32) {
        let mods = MoveMods {
            weapon_scale: 1.0, ads_scale: 1.0, perk_scale: 1.0,
            block_sprint: false, want_ads: false, ads_time: 0.25,
        };
        let dt = 1.0 / 90.0;
        let yaw = crate::math::angles_from_dir(dir.normalize()).0;
        let mut st = MoveState::default();
        st.pos = from;
        st.height = Stance::Stand.height();
        st.stance = Stance::Stand;
        st.grounded = true;
        let cmd = InputCmd { seq: 0, dt_ms: 11, move_f: 127, move_r: 0,
                             yaw, pitch: 0.0, buttons: Buttons::empty(), weapon: 0xFF };
        let mut warps = 0u32;
        for _ in 0..((secs / dt) as usize) {
            let ev = move_player(&mut st, &cmd, &mods, &map.collision, dt);
            if ev.warped { warps += 1; }
        }
        (st.pos, warps)
    }

    /// A player who walks into one end of a warp comes out of the other, and
    /// a player who walks into it from the other side is not bounced back and
    /// forth between the two.
    ///
    /// The exit side has to follow the direction of travel. With a fixed exit
    /// point, one of the two approaches puts the player down facing the mouth
    /// they just came out of, and they ping-pong for as long as they hold the
    /// key - which is what this counts.
    #[test]
    fn a_warp_carries_a_player_through_once() {
        let map = crate::maps::library::build(MapId::Deepwell);
        assert!(!map.collision.warps.is_empty(), "Deepwell has no warps to test");

        // The pair runs north-south in the west corridor: mouths at z = -14
        // and z = 16, with four blast bulkheads between them the long way.
        let (north, warps) = walk(&map, Vec3::new(-9.0, 0.1, -17.0), Vec3::Z, 2.5);
        assert_eq!(warps, 1, "walking north through the south gate warped {warps} times");
        assert!(north.z > 14.0,
                "should have come out at the north gate, ended at {north:?}");

        let (south, warps) = walk(&map, Vec3::new(-9.0, 0.1, 18.5), Vec3::NEG_Z, 2.5);
        assert_eq!(warps, 1, "walking south through the north gate warped {warps} times");
        assert!(south.z < -11.0,
                "should have come out at the south gate, ended at {south:?}");
    }

    /// A player standing in an updraft leaves the floor, and one in a
    /// low-gravity room jumps higher than one outside it.
    #[test]
    fn a_field_changes_what_gravity_does() {
        let map = crate::maps::library::build(MapId::Deepwell);
        let inside = map.collision.field_at(Vec3::new(0.0, 1.0, 0.0));
        let outside = map.collision.field_at(Vec3::new(-20.0, 1.0, -20.0));
        assert!(inside.gravity < 0.5, "the middle room should be light");
        assert_eq!(outside.gravity, 1.0, "the rest of the deck should not be");

        let jump = |at: Vec3| -> f32 {
            let mods = MoveMods {
                weapon_scale: 1.0, ads_scale: 1.0, perk_scale: 1.0,
                block_sprint: false, want_ads: false, ads_time: 0.25,
            };
            let dt = 1.0 / 120.0;
            let mut st = MoveState::default();
            st.pos = at;
            st.height = Stance::Stand.height();
            st.stance = Stance::Stand;
            st.grounded = true;
            let mut peak = at.y;
            for i in 0..300 {
                let buttons = if i < 4 { Buttons::JUMP } else { Buttons::empty() };
                let cmd = InputCmd { seq: 0, dt_ms: 8, move_f: 0, move_r: 0,
                                     yaw: 0.0, pitch: 0.0, buttons, weapon: 0xFF };
                move_player(&mut st, &cmd, &mods, &map.collision, dt);
                peak = peak.max(st.pos.y);
            }
            peak - at.y
        };
        let light = jump(Vec3::new(0.0, 0.1, 0.0));
        let normal = jump(Vec3::new(-20.0, 0.1, -20.0));
        assert!(light > normal * 1.8,
                "a jump in the light room reached {light:.2}m against {normal:.2}m outside");
    }
}
