//! Airborne goat: free-look spins/flips (GGPlayerInputGame + GGGoat.UpdateRotation), landing
//! rules (GGGoat.Landed / CanLandWithCurrentRotation), jump-over collection
//! (GGGoat.CollectJumpOverInfo / FlushJumpOverInfo) and wall running (Pawn PHYS_WallRun).

use crate::convert::{self, S};
use crate::goat::Goat;
use crate::physics::Physics;
use bevy::prelude::*;
use rapier3d::prelude as rp;

/// GGPlayerInputGame.mRotationSpeed (Unreal rotation units per second).
const ROTATION_SPEED_UU: f32 = 100000.0;
/// GGPlayerControllerGame mPawnRotInterpSpeed / Max / Increase.
pub const ROT_INTERP_SPEED: f32 = 2.2;
const ROT_INTERP_MAX: f32 = 5.0;
const ROT_INTERP_INCREASE: f32 = 0.01;
/// GGGoat.mAllowedGroundImpactUpDot and mRagdollLandSpeed.
const ALLOWED_GROUND_IMPACT_UP_DOT: f32 = 0.95;
pub const RAGDOLL_LAND_SPEED: f32 = 1780.0;
/// GGGoat.mMaxJumpOverDistance.
const MAX_JUMP_OVER_DISTANCE: f32 = 90.0;

/// Pawn wall-run settings (Engine Default__Pawn, GGGoat overrides).
pub mod wall {
    pub const MAX_RUNNABLE_WALL_Z: f32 = 0.8;
    pub const MIN_RUNNABLE_WALL_Z: f32 = -0.1;
    pub const MIN_WALL_RUN_Z: f32 = -100.0;
    pub const MAX_WALL_RUN_DOT: f32 = 0.25;
    pub const WALL_RUN_Z: f32 = 300.0;
    pub const WALL_RUN_BOOST_Z: f32 = 200.0;
    /// mMaxStepOverWallBump: the wall trace depth; farther than half of it loses the wall.
    pub const MAX_STEP_OVER_WALL_BUMP: f32 = 20.0;
    /// GGGoat overrides.
    pub const TRESHOLD_WALL_RUN_DOT: f32 = 0.0;
    pub const WALL_JUMP_Z: f32 = 1000.0;
    pub const WALL_RUN_SPEED: f32 = 750.0;
}

fn uu(v: f32) -> f32 {
    v * S
}

/// Something the goat was above while airborne.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Below {
    Collider(rp::ColliderHandle),
}

/// Spin (A/D) and flip (W/S) while holding free-look in the air. Returns (dyaw, dpitch) applied.
pub fn rotate_in_air(g: &mut Goat, forward: f32, right: f32, dt: f32, time_dilation: f32) -> (f32, f32) {
    // mRotationRate: aStrafe -> yaw, aBaseY -> pitch; the pawn turns by Rotation - rate.
    let yaw_rate = if right > 0.0 { -1.0 } else if right < 0.0 { 1.0 } else { 0.0 };
    let pitch_rate = if forward > 0.0 { 1.0 } else if forward < 0.0 { -1.0 } else { 0.0 };
    if yaw_rate == 0.0 && pitch_rate == 0.0 {
        g.rot_interp = ROT_INTERP_SPEED;
        return (0.0, 0.0);
    }
    // RInterpTo(Rotation, Rotation - rate, dt, speed) moves min(dt*speed, 1) of the way.
    let step = ROTATION_SPEED_UU * std::f32::consts::TAU / 65536.0 * (dt * g.rot_interp).min(1.0);
    g.rot_interp = (g.rot_interp + g.rot_interp * ROT_INTERP_INCREASE * time_dilation).min(ROT_INTERP_MAX);
    let (dy, dp) = (yaw_rate * step, pitch_rate * step);
    g.yaw += dy;
    g.pitch += dp;
    (dy, dp)
}

/// Goat orientation including air pitch.
pub fn goat_rotation(g: &Goat) -> Quat {
    Quat::from_rotation_y(g.yaw) * Quat::from_rotation_x(g.pitch)
}

/// Floor normal and what is below the goat within `max` metres of its feet.
pub fn probe_below(phys: &Physics, g: &Goat, center: Vec3, half_height: f32, max: f32) -> Option<(f32, Vec3, rp::ColliderHandle)> {
    let ray = rp::Ray::new(convert::to_rapier(center), rp::Vector::NEG_Y);
    let filter = rp::QueryFilter::default().exclude_rigid_body(g.body);
    phys.world
        .cast_ray_and_get_normal(&ray, half_height + max, true, filter)
        .map(|(h, hit)| (hit.time_of_impact - half_height, convert::from_rapier(hit.normal), h))
}

/// CollectJumpOverInfo: remember whatever is close below while moving through the air.
pub fn collect_jump_over(phys: &Physics, g: &mut Goat, center: Vec3, half_height: f32) {
    if Vec2::new(g.velocity.x, g.velocity.z).length_squared() < uu(3.0) * uu(3.0) {
        return;
    }
    if let Some((dist, _, h)) = probe_below(phys, g, center, half_height, uu(MAX_JUMP_OVER_DISTANCE)) {
        if dist < uu(MAX_JUMP_OVER_DISTANCE) && !g.jump_over.contains(&Below::Collider(h)) {
            g.jump_over.push(Below::Collider(h));
        }
    }
}

/// Outcome of touching down.
pub struct Landing {
    pub ragdoll: bool,
    pub perfect: bool,
    pub floor: Option<rp::ColliderHandle>,
}

/// GGGoat.Landed: bad rotation, facing away from the camera at speed, or an impact faster than
/// mRagdollLandSpeed (unless it was a perfect landing) all end in a ragdoll.
pub fn land(phys: &Physics, g: &Goat, center: Vec3, half_height: f32, impact_velocity: Vec3, cam_forward: Vec3, now: f32) -> Landing {
    let probe = probe_below(phys, g, center, half_height, 0.5);
    let normal = probe.map(|p| p.1).unwrap_or(Vec3::Y);
    let up = goat_rotation(g) * Vec3::Y;
    let pitched = g.pitch.rem_euclid(std::f32::consts::TAU).min(std::f32::consts::TAU - g.pitch.rem_euclid(std::f32::consts::TAU)) > 0.01;
    let mut ragdoll = up.dot(normal) < ALLOWED_GROUND_IMPACT_UP_DOT && pitched;
    let fwd = goat_rotation(g) * -Vec3::Z;
    let fwd2 = Vec2::new(fwd.x, fwd.z).normalize_or_zero();
    let cam2 = Vec2::new(cam_forward.x, cam_forward.z).normalize_or_zero();
    let hspeed = Vec2::new(impact_velocity.x, impact_velocity.z).length();
    if fwd2.dot(cam2) < 0.5 && hspeed > uu(700.0) {
        ragdoll = true;
    }
    let mut perfect = false;
    if !ragdoll && impact_velocity.length() > uu(RAGDOLL_LAND_SPEED) {
        if g.tried_landing_at.is_some_and(|at| now - at < crate::score::PERFECT_LANDING_JUMP_TIME) {
            perfect = true;
        } else {
            ragdoll = true;
        }
    }
    if std::env::var_os("GOAT_LAND_LOG").is_some() {
        println!(
            "    land: ragdoll {ragdoll} perfect {perfect} | up.n {:.2} pitch {:.1} deg | fwd.cam {:.2} hspeed {:.0} | impact {:.0} uu/s tried {:?}",
            up.dot(normal),
            g.pitch.to_degrees(),
            fwd2.dot(cam2),
            hspeed / convert::S,
            impact_velocity.length() / convert::S,
            g.tried_landing_at.map(|at| now - at)
        );
    }
    Landing { ragdoll, perfect, floor: probe.map(|p| p.2) }
}

/// Wall-run state kept on the goat.
#[derive(Default, Clone, Copy)]
pub struct WallRun {
    pub active: bool,
    pub normal: Vec3,
    pub time: f32,
    pub scored: bool,
}

/// Try to start a wall run from a character-controller hit (falling, after a jump).
/// Native APawn wall-run check (GoatGame.exe 0xb2c810, called from physFalling's hit handling).
pub fn try_start_wall_run(g: &mut Goat, normal: Vec3) -> bool {
    if g.grounded || !g.ready_to_wall_run || g.wall.active {
        return false;
    }
    let ny = normal.y;
    if !(ny < wall::MAX_RUNNABLE_WALL_Z && ny > wall::MIN_RUNNABLE_WALL_Z) {
        return false;
    }
    if !(g.velocity.y > uu(wall::MIN_WALL_RUN_Z)) {
        return false;
    }
    // 2D dot between the pawn's facing and the wall normal: -1 = running straight into it
    let n2 = Vec3::new(normal.x, 0.0, normal.z).normalize_or_zero();
    let fwd = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
    let dot = fwd.dot(n2);
    if !(dot < wall::MAX_WALL_RUN_DOT) {
        return false;
    }
    g.ready_to_wall_run = false;
    // SetBase(None, HitNormal): Floor = the wall normal; horizontal velocity is kept
    g.wall = WallRun { active: true, normal, time: 0.0, scored: false };
    g.velocity.y = uu(wall::WALL_RUN_Z);
    if dot < wall::TRESHOLD_WALL_RUN_DOT {
        g.velocity.y = uu(wall::WALL_RUN_Z - wall::WALL_RUN_BOOST_Z * dot);
    }
    true
}

/// One frame of PHYS_WallRun (native physWallRun, GoatGame.exe 0xb30060), before moving.
/// `wish` is the camera-relative input direction (the pawn's Acceleration). Gravity is applied
/// here. Returns false (and ends the run, back to PHYS_Falling) like the native exits.
pub fn update_wall_run(g: &mut Goat, dt: f32, wish: Vec3, gravity_y: f32) -> bool {
    if !g.wall.active {
        return false;
    }
    g.wall.time += dt;
    let n = g.wall.normal;
    // Acceleration (uu/s^2): input on the wall plane, horizontal part only, at AccelRate
    let accel = wish * crate::goat::tune::WALK_ACCEL;
    let mut a = accel - n * accel.dot(n);
    a.y = 0.0;
    if a.length_squared() < 1e-8 {
        // no acceleration: drop off the wall
        trace_end("no input along the wall", a.length());
        g.wall.active = false;
        return false;
    }
    // the pawn turns toward the input (the original follows the controller rotation)
    let wish2 = Vec3::new(wish.x, 0.0, wish.z);
    if wish2.length_squared() > 1e-6 {
        g.yaw = f32::atan2(-wish2.x, -wish2.z);
    }
    g.velocity += a.normalize() * uu(crate::goat::tune::WALK_ACCEL) * dt;
    g.velocity.y += gravity_y * dt;
    // stay on the wall plane
    g.velocity -= n * g.velocity.dot(n);
    // facing into the wall runs straight up; along it at up to mWallRunSpeed
    let n2 = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
    let fwd = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
    let max_h = uu(wall::WALL_RUN_SPEED) * (fwd.dot(n2) + 1.0).clamp(0.0, 1.0);
    let h = Vec3::new(g.velocity.x, 0.0, g.velocity.z);
    if h.length_squared() > max_h * max_h {
        let h = h.normalize_or_zero() * max_h;
        g.velocity.x = h.x;
        g.velocity.z = h.z;
    }
    if !(g.velocity.y > uu(wall::MIN_WALL_RUN_Z)) {
        trace_end("too slow upwards", g.velocity.y);
        g.wall.active = false;
        return false;
    }
    true
}

fn trace_end(why: &str, v: f32) {
    if std::env::var_os("GOAT_TRACE").is_some() {
        println!("    wall run ends: {why} ({v:.3})");
    }
}

/// After moving: the wall must still be within half of mMaxStepOverWallBump (its normal becomes
/// the new Floor) and the pawn must not face away from it (mMaxWallRunDot).
pub fn keep_wall(g: &mut Goat, wall_hit: Option<(f32, Vec3)>) -> bool {
    match wall_hit {
        Some((gap, normal)) if gap <= uu(wall::MAX_STEP_OVER_WALL_BUMP) * 0.5 => {
            g.wall.normal = normal;
        }
        _ => {
            trace_end("lost the wall", wall_hit.map_or(-1.0, |h| h.0));
            g.wall.active = false;
            return false;
        }
    }
    let n2 = Vec3::new(g.wall.normal.x, 0.0, g.wall.normal.z).normalize_or_zero();
    let fwd = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
    if fwd.dot(n2) > wall::MAX_WALL_RUN_DOT {
        trace_end("facing away", fwd.dot(n2));
        g.wall.active = false;
        return false;
    }
    true
}

/// Pawn.DoWallJump: leave the wall at 45 degrees with mWallJumpZ, keeping speed along it.
pub fn wall_jump(g: &mut Goat) {
    let n2 = Vec3::new(g.wall.normal.x, 0.0, g.wall.normal.z).normalize_or_zero();
    g.velocity.y = 0.0;
    g.velocity += (n2 + Vec3::Y).normalize() * uu(wall::WALL_JUMP_Z);
    g.wall.active = false;
    g.ready_to_wall_run = true;
}
