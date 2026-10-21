//! rapier3d world stepping and transform sync (stands in for UE3's PhysX).

use crate::convert;

pub mod volumes;
use bevy::prelude::*;
use rapier3d::prelude as rp;

/// Unreal DefaultGravityZ from Engine/Config (-750 uu/s^2).
pub const GRAVITY_UU: f32 = -750.0;

/// Lightest a woken prop may be (kg).
pub const MIN_PROP_MASS: f32 = 2.0;

#[derive(Resource)]
pub struct Physics {
    /// Static colliders of matinee movers (InterpActor family) by lower-case actor name, with
    /// their placement at load (uvm::matinee::movers moves them).
    pub movers: std::collections::HashMap<String, Vec<(rp::ColliderHandle, Vec3, Quat)>>,
    pub world: rp::PhysicsWorld,
    pub accumulator: f32,
    /// Time dilation (slow motion toggle).
    pub time_scale: f32,
    /// KActors that have not been disturbed yet. Like UE3 KActors (bWakeOnLevelStart=false)
    /// they stay frozen as fixed bodies until something touches them.
    pub dormant: std::collections::HashSet<rp::RigidBodyHandle>,
    /// Actor names of props (debugging / logs).
    pub names: std::collections::HashMap<rp::RigidBodyHandle, String>,
    /// Character capsules -> their entity (NPCs), for hit tests.
    pub owners: std::collections::HashMap<rp::ColliderHandle, Entity>,
    /// GGPhysicalMaterialProperty score + in-game name of props.
    pub prop_info: std::collections::HashMap<rp::RigidBodyHandle, (i32, String)>,
    /// Props whose physical material has mShouldTripGoat.
    pub trip: std::collections::HashSet<rp::RigidBodyHandle>,
    /// Distinct surfaces (physical materials) of the level.
    pub surfaces: Vec<Surface>,
    /// Collider -> surface index (static meshes and props).
    pub collider_surface: std::collections::HashMap<rp::ColliderHandle, u16>,
    /// Landscape colliders: surface index per triangle (dominant paint layer).
    pub landscape_surface: std::collections::HashMap<rp::ColliderHandle, Vec<u16>>,
    /// Props that play their material's ImpactSound when they hit something.
    pub impacts: std::collections::HashMap<rp::RigidBodyHandle, PropImpact>,
    /// Damage dealt this frame (TakeDamage calls): headbutts, explosions. Read by the
    /// destructible and explosive systems, cleared at the end of the frame.
    pub hits: Vec<Hit>,
    /// Prop bodies -> their actor's export in the map and their render entity.
    pub actors: std::collections::HashMap<rp::RigidBodyHandle, (usize, Entity)>,
    /// Prop bodies -> their physical material.
    pub physmat: std::collections::HashMap<rp::RigidBodyHandle, upk::registry::ObjRef>,
    /// Things that happened to map actors, for the scripts (GGGameInfo.OnExplosion/OnFractured,
    /// Kismet); drained by the VM (capped when nothing reads them).
    pub world_events: Vec<WorldEvent>,
    /// RigidBodyCollision notifications this frame: (prop body, other collider, point) for
    /// contacts whose impulse changed the prop's velocity by more than
    /// ScriptRigidBodyCollisionThreshold (50 uu/s). Read by crate::chain, cleared in Last.
    pub contacts: Vec<(rp::RigidBodyHandle, rp::ColliderHandle, Vec3)>,
    /// Ray-cast vehicles (crate::vehicle), updated before every step.
    pub vehicles: Vec<rapier3d::control::DynamicRayCastVehicleController>,
}

#[derive(Clone, Copy, Debug)]
pub enum WorldEvent {
    /// A map actor (export index) exploded.
    Exploded { export: usize, by_goat: bool },
    /// A map actor (export index) fractured.
    Fractured { export: usize, by_goat: bool },
}

impl Physics {
    pub fn world_event(&mut self, e: WorldEvent) {
        if self.world_events.len() > 64 {
            self.world_events.remove(0);
        }
        self.world_events.push(e);
    }
}

/// UE3 damage type families that matter to the ported TakeDamage handlers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    /// GGDamageTypeAbility (headbutt, abilities).
    Ability,
    /// GGDamageTypeCollision.
    Collision,
    /// GGDamageTypeExplosiveActor (HurtRadius).
    Explosion,
}

/// One TakeDamage call against a collider.
#[derive(Clone, Debug)]
pub struct Hit {
    pub collider: rp::ColliderHandle,
    pub damage: i32,
    /// Momentum in Unreal units (kg uu/s), Bevy axes.
    pub momentum: Vec3,
    pub kind: HitKind,
    /// The goat caused it (its speed in uu/s, for the sprint checks).
    pub goat: Option<f32>,
    pub point: Vec3,
}

/// A physical material as far as sound goes (see upk::physasset::SurfaceInfo).
#[derive(Clone, Debug, Default)]
pub struct Surface {
    /// GGPhysicalMaterialProperty.MaterialType ("Grass", "Road", ...; empty = default).
    pub kind: String,
    /// Sound key (the cue's path) of PhysicalMaterial.ImpactSound.
    pub impact: Option<String>,
    /// uu/s
    pub impact_threshold: f32,
    pub impact_refire: f32,
}

#[derive(Clone, Debug)]
pub struct PropImpact {
    pub surface: u16,
    pub last: f32,
    pub prev_vel: Vec3,
}

impl Physics {
    /// Turn a dormant prop into a simulated body. Returns true if the body is dynamic now.
    pub fn wake(&mut self, h: rp::RigidBodyHandle) -> bool {
        if self.dormant.remove(&h) {
            if let Some(rb) = self.world.bodies.get_mut(h) {
                rb.set_body_type(rp::RigidBodyType::Dynamic, true);
                rb.recompute_mass_properties_from_colliders(&self.world.colliders);
                let m = rb.mass();
                if m < MIN_PROP_MASS {
                    rb.set_additional_mass(MIN_PROP_MASS - m, true);
                }
                rb.wake_up(true);
            }
        }
        self.world.bodies.get(h).is_some_and(|b| b.is_dynamic())
    }

    /// Wake dormant props touched by moving dynamic bodies.
    fn wake_touched(&mut self) {
        if self.dormant.is_empty() {
            return;
        }
        let mut to_wake = Vec::new();
        for h in self.world.islands.active_bodies() {
            let Some(rb) = self.world.bodies.get(h) else { continue };
            if !rb.is_dynamic() || rb.linvel().length() < 0.3 && rb.angvel().length() < 0.5 {
                continue;
            }
            for &c in rb.colliders() {
                for pair in self.world.narrow_phase.contact_pairs_with(c) {
                    if !pair.has_any_active_contact() {
                        continue;
                    }
                    let other = if pair.collider1 == c { pair.collider2 } else { pair.collider1 };
                    if let Some(p) = self.world.colliders.get(other).and_then(|oc| oc.parent()) {
                        if self.dormant.contains(&p) {
                            to_wake.push(p);
                        }
                    }
                }
            }
        }
        for h in to_wake {
            self.wake(h);
        }
    }
}

impl Physics {
    /// The surface of a collider at a ray/shape-cast feature (landscape: per triangle).
    pub fn surface_at(&self, c: rp::ColliderHandle, feature: rp::FeatureId) -> Option<&Surface> {
        let idx = match (self.landscape_surface.get(&c), feature) {
            (Some(tris), rp::FeatureId::Face(f)) => *tris.get(f as usize)?,
            (Some(tris), _) => *tris.first()?,
            (None, _) => *self.collider_surface.get(&c)?,
        };
        self.surfaces.get(idx as usize)
    }

    /// GGKActorBase.RigidBodyCollision (bNotifyRigidBodyCollision, ScriptRigidBodyCollisionThreshold
    /// 50): awake scored props in contact hard enough this step.
    fn collect_contacts(&mut self) {
        const THRESHOLD: f32 = 50.0 * convert::S;
        let active: Vec<rp::RigidBodyHandle> = self.world.islands.active_bodies().collect();
        for h in active {
            if !self.prop_info.contains_key(&h) {
                continue;
            }
            let Some(rb) = self.world.bodies.get(h) else { continue };
            if !rb.is_dynamic() || rb.mass() <= 0.0 {
                continue;
            }
            let mass = rb.mass();
            for &c in rb.colliders() {
                for pair in self.world.narrow_phase.contact_pairs_with(c) {
                    if !pair.has_any_active_contact() || pair.total_impulse_magnitude() / mass < THRESHOLD {
                        continue;
                    }
                    let other = if pair.collider1 == c { pair.collider2 } else { pair.collider1 };
                    if self.contacts.iter().any(|&(b, o, _)| b == h && o == other) || self.contacts.len() > 512 {
                        continue;
                    }
                    self.contacts.push((h, other, convert::from_rapier(rb.translation())));
                }
            }
        }
    }

    /// KActor impact sounds: a prop whose velocity jumps by more than its material's
    /// ImpactThreshold plays ImpactSound (at most once per ImpactReFireDelay).
    fn prop_impacts(&mut self, now: f32, out: &mut Vec<(String, Option<Vec3>)>) {
        let active: Vec<rp::RigidBodyHandle> = self.world.islands.active_bodies().collect();
        for h in active {
            let Some(imp) = self.impacts.get_mut(&h) else { continue };
            let Some(rb) = self.world.bodies.get(h) else { continue };
            if !rb.is_dynamic() {
                continue;
            }
            let v = convert::from_rapier(rb.linvel());
            let dv = (v - imp.prev_vel).length() / convert::S;
            imp.prev_vel = v;
            let Some(s) = self.surfaces.get(imp.surface as usize) else { continue };
            if dv >= s.impact_threshold.max(100.0) && now - imp.last >= s.impact_refire {
                imp.last = now;
                if let Some(cue) = &s.impact {
                    out.push((cue.clone(), Some(convert::from_rapier(rb.translation()))));
                }
            }
        }
    }
}

impl Default for Physics {
    fn default() -> Self {
        let mut world = rp::PhysicsWorld::new();
        world.gravity = rp::Vector::new(0.0, GRAVITY_UU * convert::S, 0.0);
        world.integration_parameters.dt = 1.0 / 60.0;
        Self { world, accumulator: 0.0, time_scale: 1.0, dormant: Default::default(), names: Default::default(), owners: Default::default(), prop_info: Default::default(),
            trip: Default::default(),
            surfaces: Default::default(),
            collider_surface: Default::default(),
            landscape_surface: Default::default(),
            impacts: Default::default(),
            movers: Default::default(),
            hits: Default::default(),
            actors: Default::default(),
            physmat: Default::default(),
            world_events: Default::default(),
            contacts: Default::default(),
            vehicles: Default::default(),
        }
    }
}

#[derive(Component)]
pub struct PhysBody {
    pub handle: rp::RigidBodyHandle,
    pub scale: Vec3,
}

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Physics>()
            .init_resource::<volumes::Volumes>()
            .add_systems(PostUpdate, (step, sync).chain().before(TransformSystems::Propagate))
            .add_systems(Update, (volumes::register_sounds, volumes::trampolines, volumes::environment).chain().after(crate::goat::goat_move))
            .add_systems(Last, |mut phys: ResMut<Physics>| {
                phys.hits.clear();
                phys.contacts.clear();
            });
    }
}

pub fn step(time: Res<Time>, mut phys: ResMut<Physics>, mut sounds: ResMut<crate::audio::SoundQueue>, mut prof: Local<(f32, u32, f32)>) {
    let _prof_scope = crate::prof::scope("physics::step");
    let t0 = std::time::Instant::now();
    let dt = phys.world.integration_parameters.dt;
    phys.accumulator += time.delta_secs().min(0.1) * phys.time_scale;
    let mut n = 0;
    while phys.accumulator >= dt && n < 4 {
        // DefaultGravityZ, scaled by enabled mods
        phys.world.gravity = rp::Vector::new(0.0, GRAVITY_UU * convert::S * crate::mods::tweak().gravity, 0.0);
        {
            let p = &mut *phys;
            for v in &mut p.vehicles {
                let w = &mut p.world;
                let filter = rp::QueryFilter::default().exclude_rigid_body(v.chassis).exclude_sensors();
                let q = w.broad_phase.as_query_pipeline_mut(w.narrow_phase.query_dispatcher(), &mut w.bodies, &mut w.colliders, filter);
                v.update_vehicle(dt, q);
            }
        }
        phys.world.step();
        phys.wake_touched();
        let now = time.elapsed_secs();
        phys.prop_impacts(now, &mut sounds.0);
        phys.collect_contacts();
        phys.accumulator -= dt;
        n += 1;
    }
    if n == 4 {
        phys.accumulator = 0.0;
    }
    // GOAT_PROFILE=1 prints the average physics cost every 2 s
    if std::env::var_os("GOAT_PROFILE").is_some() {
        prof.0 += t0.elapsed().as_secs_f32();
        prof.1 += 1;
        prof.2 += time.delta_secs();
        if prof.2 > 2.0 {
            let awake = phys.world.bodies.iter().filter(|(_, b)| b.is_dynamic() && !b.is_sleeping()).count();
            println!("profile: physics {:.2} ms/frame, frame {:.2} ms, awake dynamic {awake}, colliders {}", prof.0 / prof.1 as f32 * 1000.0, prof.2 / prof.1 as f32 * 1000.0, phys.world.colliders.len());
            *prof = (0.0, 0, 0.0);
        }
    }
}

pub fn sync(phys: Res<Physics>, mut q: Query<(&PhysBody, &mut Transform)>) {
    let _prof_scope = crate::prof::scope("physics::sync");
    for (b, mut t) in &mut q {
        if let Some(rb) = phys.world.bodies.get(b.handle) {
            if !rb.is_dynamic() || rb.is_sleeping() {
                continue;
            }
            t.translation = convert::from_rapier(rb.translation());
            t.rotation = convert::quat_from_rapier(*rb.rotation());
            t.scale = b.scale;
        }
    }
}
