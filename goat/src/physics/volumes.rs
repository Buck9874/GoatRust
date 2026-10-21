//! Volumes from the level's brushes. Implemented: GGTrampolineVolume (trampolines, mattresses,
//! fans, bouncy castles) for the goat, its ragdoll and loose props.

use crate::convert::{self, S};
use crate::goat::Goat;
use crate::physics::Physics;
use crate::ragdoll::ActiveRagdoll;
use bevy::prelude::*;
use rapier3d::prelude as rp;
use std::collections::{HashMap, HashSet};

/// One GGTrampolineVolume and its world-space hulls.
pub struct Trampoline {
    pub name: String,
    /// mDisplayName ("Trampoline", "Mattress", "Fan", "Bouncy Castle").
    pub display: String,
    /// mBounceNormal = vector(mBounceDirection), Bevy space.
    pub normal: Vec3,
    /// uu/s
    pub bounce_factor: f32,
    pub max_bounce: f32,
    pub hulls: Vec<(rp::Pose, rp::SharedShape)>,
    /// mBounceSound (cue path, also its key in the sound table).
    pub sound: Option<String>,
    /// A child of GGTrampolineVolumeTrampolineAbstract (real trampolines; GGTrickBounce pairs).
    pub is_trampoline: bool,
}

impl Trampoline {
    /// Is a point inside (or within `margin` of) the volume?
    pub fn contains(&self, p: Vec3, margin: f32) -> bool {
        let pt = convert::to_rapier(p);
        self.hulls.iter().any(|(pose, s)| {
            let local = pose.inverse_transform_point(pt);
            s.distance_to_local_point(local, true) <= margin
        })
    }

    /// Centre of the top of the first hull (tests place things above it).
    pub fn top(&self) -> Vec3 {
        let (pose, s) = &self.hulls[0];
        let aabb = s.compute_aabb(pose);
        let c = convert::from_rapier((aabb.mins + aabb.maxs) * 0.5);
        Vec3::new(c.x, aabb.maxs.y, c.z)
    }

    /// GGTrampolineVolume.PostTouch: keep the motion along the surface, reflect the motion into
    /// it, adding mBounceFactor up to mMaxBounceVelocity, and scatter it in a PI / 20.3 cone.
    /// Velocities in m/s.
    pub fn bounce(&self, v: Vec3, rnd: (f32, f32)) -> Vec3 {
        let n = self.normal;
        let projected = n * v.dot(n);
        let parallel = v - projected;
        let perp = -projected;
        let perp = perp.normalize_or(n) * (perp.length() + self.bounce_factor * S).min(self.max_bounce * S);
        let new_v = parallel + perp;
        // VRandCone(newVelocity, PI / 20.3) * |newVelocity|
        let len = new_v.length();
        let dir = new_v.normalize_or(n);
        let half = std::f32::consts::PI / 20.3;
        let side = dir.any_orthonormal_vector();
        let up = dir.cross(side);
        let ang = rnd.0 * std::f32::consts::TAU;
        let tilt = (rnd.1 * half).sin();
        (dir + (side * ang.cos() + up * ang.sin()) * tilt).normalize() * len
    }
}

/// The other volume kinds that change how things move inside them.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// BlockingVolume: a static collider (added at load).
    Blocking,
    /// PhysicsVolume / GravityVolume: GravityZ (uu/s^2, replaces the world's while inside),
    /// ZoneVelocity (m/s, moves pawns like a current), bEnabled (GGDynamicGravityVolume starts off).
    /// Priority (the highest containing volume is the pawn's PhysicsVolume), RigidBodyDamping
    /// (drag on rigid bodies inside: -velocity x damping x mass).
    Physics { gravity_z: Option<f32>, zone_velocity: Vec3, enabled: bool, priority: i32, damping: f32 },
    /// LadderVolume: WallDir (Bevy space), the way the goat faces to climb.
    Ladder { wall_dir: Vec3 },
    /// GGWaterVolume / GGCoinVolume (WaterVolume): pawns touching it are SetIsInWater.
    Water,
    /// PostProcessVolume: Priority (its Settings: look.rs, by the volume's name).
    PostProcess { priority: i32 },
}

pub struct Volume {
    pub name: String,
    pub kind: Kind,
    pub hulls: Vec<(rp::Pose, rp::SharedShape)>,
}

impl Volume {
    pub fn contains(&self, p: Vec3, margin: f32) -> bool {
        let pt = convert::to_rapier(p);
        self.hulls.iter().any(|(pose, s)| s.distance_to_local_point(pose.inverse_transform_point(pt), true) <= margin)
    }
}

/// What the volumes around the goat do to its movement (read by goat_move next frame).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GoatEnv {
    /// Gravity (Bevy m/s^2, Y) replacing the world's.
    pub gravity_y: Option<f32>,
    /// Current to move with (m/s).
    pub zone_velocity: Vec3,
    /// In a ladder volume: its wall direction.
    pub ladder: Option<Vec3>,
    /// Touching a water volume (GGPawn.mIsInWater).
    pub water: bool,
    /// mDistanceToGround < mIsInAirThreshold (50 uu) while in water: near the shore.
    pub near_ground: bool,
    /// GetMaterialBelowFeet: a water surface this far (uu) below the pawn's centre.
    pub water_below: Option<f32>,
}

#[derive(Resource, Default)]
pub struct Volumes {
    pub trampolines: Vec<Trampoline>,
    pub others: Vec<Volume>,
    /// Decoded Ogg data of the bounce sounds, keyed by cue path (moved into the sound table).
    pub sounds: HashMap<String, Vec<Vec<u8>>>,
    /// (trampoline, body) pairs touching last frame; a bounce happens on a new touch.
    touching: HashSet<(usize, u64)>,
    rng: u32,
    /// Bodies given a gravity/damping force last frame (rapier keeps forces until reset).
    forced: HashSet<rp::RigidBodyHandle>,
    /// Bounces so far (tests).
    pub bounces: u32,
    /// GGTrickBounce: mLastActorBelowFeet (None until the goat first lands) and mWasAirborne.
    below_feet: Option<crate::score::BelowFeet>,
    was_airborne: bool,
    /// Bounce tricks so far (tests).
    pub bounce_tricks: u32,
}

impl Volumes {
    fn rand(&mut self) -> f32 {
        let mut x = self.rng.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
}

/// The bounce sounds go into the shared sound table once it exists.
pub fn register_sounds(mut vols: ResMut<Volumes>, sfx: Option<ResMut<crate::audio::Sfx>>, mut audio: ResMut<Assets<AudioSource>>) {
    let Some(mut sfx) = sfx else { return };
    if vols.sounds.is_empty() {
        return;
    }
    for (key, oggs) in vols.sounds.drain() {
        let handles: Vec<Handle<AudioSource>> = oggs.into_iter().map(|b| audio.add(AudioSource { bytes: b.into() })).collect();
        sfx.cues.entry(key).or_default().extend(handles);
    }
}

fn body_key(h: rp::RigidBodyHandle) -> u64 {
    let (i, g) = h.into_raw_parts();
    ((g as u64) << 32) | i as u64
}

/// GGTrampolineVolume.Touch / PostTouch for the goat (walking goats don't bounce), its ragdoll
/// and awake props.
pub fn trampolines(
    mut vols: ResMut<Volumes>,
    mut phys: ResMut<Physics>,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut score: ResMut<crate::score::Score>,
    mut goats: Query<(&mut Goat, &Transform, Option<&ActiveRagdoll>)>,
) {
    let _prof_scope = crate::prof::scope("volumes::trampolines");
    if vols.trampolines.is_empty() {
        return;
    }
    let mut goat_bounce: Option<usize> = None;
    let mut now: HashSet<(usize, u64)> = HashSet::new();
    let mut fire: Vec<(usize, Option<String>, Vec3)> = Vec::new();
    // the goat (capsule while falling, pelvis while ragdolled)
    if let Ok((mut g, t, rd)) = goats.single_mut() {
        let (pos, vel, key) = match rd {
            Some(rd) => match phys.world.bodies.get(rd.pelvis) {
                Some(b) => (convert::from_rapier(b.translation()), convert::from_rapier(b.linvel()), body_key(rd.pelvis)),
                None => (t.translation, g.velocity, body_key(g.body)),
            },
            None => (t.translation, g.velocity, body_key(g.body)),
        };
        let falling = rd.is_some() || !g.grounded;
        for i in 0..vols.trampolines.len() {
            let tr = &vols.trampolines[i];
            let margin = crate::goat::tune::COLLISION_HEIGHT * S;
            if !tr.contains(pos, margin) {
                continue;
            }
            now.insert((i, key));
            if vols.touching.contains(&(i, key)) || !falling || vel.normalize_or_zero().dot(tr.normal) >= 0.0 {
                continue;
            }
            let rnd = (vols.rand(), vols.rand());
            let tr = &vols.trampolines[i];
            let new_v = tr.bounce(vel, rnd);
            match rd {
                Some(rd) => {
                    // SetRBLinearVelocity on the whole ragdoll
                    for &(_, h) in &rd.bodies {
                        if let Some(b) = phys.world.bodies.get_mut(h) {
                            b.set_linvel(convert::to_rapier(new_v), true);
                        }
                    }
                }
                None => {
                    g.velocity = new_v;
                    g.grounded = false;
                    // GGGoat.OnBounce: jumps and wall runs are available again
                    g.double_jump_ready = true;
                    g.multi_jump_remaining = 1;
                    g.ready_to_wall_run = true;
                }
            }
            goat_bounce = Some(i);
            fire.push((i, tr.sound.clone(), pos));
        }
        // GGTrickBounce.OnGround / OnAirborne (GGGoat.OnBounce reports the volume as the ground)
        let here = |vols: &Volumes, i: usize| {
            let t = &vols.trampolines[i];
            crate::score::BelowFeet::Volume(i, t.is_trampoline, t.display.clone())
        };
        let ground = match goat_bounce {
            Some(i) => Some(here(&vols, i)),
            // TouchingGroundVolume: standing in a bouncy volume counts as being on it
            None if g.grounded && rd.is_none() => Some(
                (0..vols.trampolines.len()).find(|&i| vols.trampolines[i].contains(pos, 0.1)).map(|i| here(&vols, i)).unwrap_or(crate::score::BelowFeet::Other),
            ),
            None => None,
        };
        match ground {
            Some(to) => {
                if vols.was_airborne {
                    vols.was_airborne = false;
                    if let Some(trick) = vols.below_feet.as_ref().and_then(|from| crate::score::bounce(from, &to)) {
                        if std::env::var_os("GOAT_TRACE").is_some() {
                            println!("    bounce trick: {} ({})", trick.name, trick.score);
                        }
                        score.report(trick);
                        vols.bounce_tricks += 1;
                    }
                }
                vols.below_feet = Some(to);
            }
            None => vols.was_airborne = true,
        }
    }
    // loose props (KActors are named in Physics::names)
    let active: Vec<rp::RigidBodyHandle> = phys.world.islands.active_bodies().collect();
    for h in active {
        if !phys.names.contains_key(&h) {
            continue;
        }
        let Some(b) = phys.world.bodies.get(h) else { continue };
        if !b.is_dynamic() {
            continue;
        }
        let pos = convert::from_rapier(b.translation());
        let vel = convert::from_rapier(b.linvel());
        let key = body_key(h);
        for i in 0..vols.trampolines.len() {
            if !vols.trampolines[i].contains(pos, 0.1) {
                continue;
            }
            now.insert((i, key));
            if vols.touching.contains(&(i, key)) || vel.normalize_or_zero().dot(vols.trampolines[i].normal) >= 0.0 {
                continue;
            }
            let rnd = (vols.rand(), vols.rand());
            let new_v = vols.trampolines[i].bounce(vel, rnd);
            if let Some(b) = phys.world.bodies.get_mut(h) {
                b.set_linvel(convert::to_rapier(new_v), true);
            }
            fire.push((i, vols.trampolines[i].sound.clone(), pos));
        }
    }
    for (i, sound, pos) in fire {
        vols.bounces += 1;
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    bounce on {} ({})", vols.trampolines[i].name, vols.trampolines[i].display);
        }
        if let Some(s) = sound {
            sounds.play(&s, Some(pos));
        }
    }
    vols.touching = now;
}

/// PhysicsVolume / GravityVolume / LadderVolume effects for the goat (into Goat::env) and
/// GravityVolume gravity for awake props.
pub fn environment(mut vols: ResMut<Volumes>, mut phys: ResMut<Physics>, mut goats: Query<(&mut Goat, &Transform)>) {
    let _prof_scope = crate::prof::scope("volumes::environment");
    if vols.others.is_empty() {
        return;
    }
    if let Ok((mut g, t)) = goats.single_mut() {
        let mut env = GoatEnv::default();
        let mut best_priority = i32::MIN;
        for v in &vols.others {
            match v.kind {
                Kind::Blocking | Kind::PostProcess { .. } => {}
                Kind::Physics { gravity_z, zone_velocity, enabled, priority, .. } => {
                    if enabled && v.contains(t.translation, 0.0) {
                        // the highest Priority volume's GravityZ
                        if let Some(gz) = gravity_z {
                            if priority >= best_priority {
                                best_priority = priority;
                                env.gravity_y = Some(gz * S);
                            }
                        }
                        env.zone_velocity += zone_velocity;
                    }
                }
                Kind::Ladder { wall_dir } => {
                    if v.contains(t.translation, 0.1) {
                        env.ladder = Some(wall_dir);
                    }
                }
                Kind::Water => {
                    // Touch: the pawn's collision cylinder overlapping the water
                    if v.contains(t.translation, crate::goat::tune::COLLISION_HEIGHT * S) {
                        env.water = true;
                    } else {
                        // the trace from above the water stops at its surface
                        let trace = crate::goat::tune::COLLISION_HEIGHT * 1.5;
                        let ray = rp::Ray::new(convert::to_rapier(t.translation), rp::Vector::NEG_Y);
                        let hit = v.hulls.iter().filter_map(|(pose, s)| s.cast_ray(pose, &ray, trace * S, true)).fold(None, |a: Option<f32>, toi| Some(a.map_or(toi, |a| a.min(toi))));
                        if let Some(toi) = hit {
                            env.water_below = Some(toi / S);
                        }
                    }
                }
            }
        }
        if env.water {
            let ray = rp::Ray::new(convert::to_rapier(t.translation), rp::Vector::NEG_Y);
            let half = crate::goat::tune::COLLISION_HEIGHT * S;
            env.near_ground = phys
                .world
                .cast_ray(&ray, half + 50.0 * S, true, rp::QueryFilter::only_fixed())
                .is_some();
        }
        if env != g.env {
            if std::env::var_os("GOAT_TRACE").is_some() {
                println!("    volumes around the goat: {env:?}");
            }
            g.env = env;
        }
    }
    // props floating in zero/low gravity volumes: cancel the difference to the world's gravity
    let world_g = phys.world.gravity.y;
    // (volume, gravity, priority, RigidBodyDamping)
    let gravity_vols: Vec<(usize, Option<f32>, i32, f32)> = vols
        .others
        .iter()
        .enumerate()
        .filter_map(|(i, v)| match v.kind {
            Kind::Physics { gravity_z, enabled: true, priority, damping, .. } if gravity_z.is_some() || damping > 0.0 => Some((i, gravity_z.map(|g| g * S), priority, damping)),
            _ => None,
        })
        .collect();
    if gravity_vols.is_empty() {
        return;
    }
    let mut forced = HashSet::new();
    let active: Vec<rp::RigidBodyHandle> = phys.world.islands.active_bodies().collect();
    for h in active {
        let Some(b) = phys.world.bodies.get_mut(h) else { continue };
        if !b.is_dynamic() {
            continue;
        }
        let p = convert::from_rapier(b.translation());
        let inside = gravity_vols.iter().filter(|(i, ..)| vols.others[*i].contains(p, 0.0)).max_by_key(|(_, _, prio, _)| *prio);
        if let Some(&(_, gy, _, damping)) = inside {
            let m = b.mass();
            let v = convert::from_rapier(b.linvel());
            b.reset_forces(false);
            let mut f = -v * damping * m;
            if let Some(gy) = gy {
                f += Vec3::Y * (gy - world_g) * m;
            }
            b.add_force(convert::to_rapier(f), true);
            forced.insert(h);
        }
    }
    // out of the volume again: drop the force
    for h in vols.forced.difference(&forced) {
        if let Some(b) = phys.world.bodies.get_mut(*h) {
            b.reset_forces(false);
        }
    }
    vols.forced = forced;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounce_reflects_and_caps() {
        let t = Trampoline {
            name: "t".into(),
            display: "Trampoline".into(),
            normal: Vec3::Y,
            bounce_factor: 300.0,
            max_bounce: 2500.0,
            hulls: vec![],
            sound: None,
            is_trampoline: true,
        };
        // falling at 800 uu/s -> up at 1100 uu/s (cone scatter keeps the speed)
        let v = t.bounce(Vec3::new(0.0, -800.0 * S, 0.0), (0.0, 0.0));
        assert!((v.length() / S - 1100.0).abs() < 1.0 && v.y > 0.0, "{v}");
        // capped at mMaxBounceVelocity
        let v = t.bounce(Vec3::new(0.0, -3000.0 * S, 0.0), (0.3, 0.5));
        assert!((v.length() / S - 2500.0).abs() < 1.0, "{v}");
        // sideways motion is kept
        let v = t.bounce(Vec3::new(200.0 * S, -500.0 * S, 0.0), (0.0, 0.0));
        assert!((v.x / S - 200.0).abs() < 1.0 && (v.y / S - 800.0).abs() < 1.0, "{v}");
    }
}
