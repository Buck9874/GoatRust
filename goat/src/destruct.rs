//! GGApexDestructibleActor: APEX destructibles (vases, plates, fences, greenhouse glass, the gas
//! station roof...). The intact object is chunk 0 of its ApexDestructibleAsset (upk::apex) with
//! static collision (the leaf chunks' hulls); when it fractures, the leaf chunks become physics
//! bodies. GGKActors with an mApexActor archetype turn into that destructible when damaged
//! (GGKActor.ConvertToApex).
//!
//! Fracture rules (GGApexDestructibleActor.ShouldFracture):
//! - damage of a valid type (Default__: GGDamageTypeAbility) >= the asset's DamageThreshold
//!   (headbutt mDamage 200 vs thresholds of 1);
//! - GGDamageTypeCollision with |momentum| >= mTriggerFractureMomentum (2000), from the goat only
//!   while faster than mSprintSpeed (momentum = goat Mass 45 x hit velocity), from anything else
//!   (props: mass x velocity) on momentum alone.

use crate::convert;
use crate::goat::Goat;
use crate::physics::{HitKind, PhysBody, Physics};
use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;
use rapier3d::prelude as rp;
use std::collections::HashMap;
use upk::level::Level;
use upk::registry::{ObjRef, Registry};

/// Default__GGApexDestructibleActor.mTriggerFractureMomentum
const TRIGGER_FRACTURE_MOMENTUM: f32 = 2000.0;
/// Density of debris chunks (rapier, kg/m^3).
const DEBRIS_DENSITY: f32 = 250.0;

struct ChunkGfx {
    /// (mesh, material) per submesh, vertices in asset space (metres, Bevy axes, unscaled).
    meshes: Vec<(Handle<Mesh>, Handle<StandardMaterial>)>,
    verts: Vec<Vec3>,
}

struct ApexAsset {
    name: String,
    chunks: Vec<ChunkGfx>,
    leaves: Vec<usize>,
    damage_threshold: f32,
    /// GGApexDestructibleActor.GetActorName / GetScore (DefaultPhysMaterial's property).
    display: String,
    score: i32,
    fracture_sound: Option<String>,
}

pub struct ApexActor {
    pub name: String,
    export: usize,
    asset: usize,
    xf: Transform,
    entity: Entity,
    colliders: Vec<rp::ColliderHandle>,
    pub fractured: bool,
    trigger_momentum: f32,
}

#[derive(Resource, Default)]
pub struct Destructibles {
    pub actors: Vec<ApexActor>,
    assets: Vec<ApexAsset>,
    by_collider: HashMap<rp::ColliderHandle, usize>,
    /// GGKActor bodies with an mApexActor archetype -> (asset, trigger momentum).
    kactors: HashMap<rp::RigidBodyHandle, (usize, f32)>,
    /// Decoded fracture sounds, moved into the sound table once it exists.
    sounds: HashMap<String, Vec<Vec<u8>>>,
    /// Fractures so far (tests).
    pub fractures: u32,
    /// Debris bodies spawned so far (tests).
    pub debris: u32,
    pub last_fractured: Option<String>,
    rng: u32,
}

impl Destructibles {
    fn rand(&mut self) -> f32 {
        let mut x = self.rng.max(1);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Intact destructibles (tests).
    pub fn intact(&self) -> impl Iterator<Item = (usize, &ApexActor)> {
        self.actors.iter().enumerate().filter(|(_, a)| !a.fractured)
    }

    /// Centre of an actor's intact chunk in world space.
    pub fn centre(&self, i: usize) -> Vec3 {
        let a = &self.actors[i];
        let v = &self.assets[a.asset].chunks[0].verts;
        let c = v.iter().copied().sum::<Vec3>() / v.len().max(1) as f32;
        a.xf.transform_point(c)
    }

    /// A KActor that turns into a destructible when damaged.
    pub fn is_breakable_kactor(&self, h: rp::RigidBodyHandle) -> bool {
        self.kactors.contains_key(&h)
    }

    pub fn asset_name(&self, i: usize) -> &str {
        &self.assets[self.actors[i].asset].name
    }
}

/// The ApexDestructibleAsset of a GGApexDestructibleActor (placed or archetype).
fn asset_of(reg: &Registry, actor: ObjRef) -> Option<ObjRef> {
    let ps = reg.merged_props(actor);
    let (pk, comp) = match upk::registry::find(&ps, "StaticDestructibleComponent") {
        Some((pk, upk::props::Value::Object(o))) => (pk, *o),
        _ => return None,
    };
    let c = reg.resolve(pk, comp)?;
    let cps = reg.merged_props(c);
    match upk::registry::find(&cps, "Asset") {
        Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o),
        _ => None,
    }
}

/// GGKActor.mApexActor of a prop's actor.
fn kactor_apex(reg: &Registry, actor_export: usize) -> Option<ObjRef> {
    let ps = reg.merged_props(ObjRef { pkg: 0, export: actor_export });
    match upk::registry::find(&ps, "mApexActor") {
        Some((pk, upk::props::Value::Object(o))) if *o != 0 => reg.resolve(pk, *o),
        _ => None,
    }
}

fn float_prop(reg: &Registry, r: ObjRef, k: &str, d: f32) -> f32 {
    match upk::registry::find(&reg.merged_props(r), k) {
        Some((_, upk::props::Value::Float(f))) => *f,
        _ => d,
    }
}

/// Materials of every destructible asset the level uses (decoded with the level's).
pub fn material_refs(reg: &Registry, level: &Level) -> Vec<ObjRef> {
    let mut assets: Vec<ObjRef> = level.apex.iter().filter_map(|a| reg.resolve(0, a.asset)).collect();
    for inst in level.instances.iter().filter(|i| i.dynamic) {
        if let Some(arch) = inst.actor_export.and_then(|e| kactor_apex(reg, e)) {
            assets.extend(asset_of(reg, arch));
        }
    }
    assets.sort();
    assets.dedup();
    let mut out = Vec::new();
    for a in assets {
        let ps = reg.merged_props(a);
        if let Some((pk, v)) = upk::registry::find(&ps, "Materials") {
            out.extend(upk::material::object_array(Some(v)).into_iter().filter_map(|m| reg.resolve(pk, m)));
        }
    }
    out
}

fn build_asset(
    reg: &Registry,
    r: ObjRef,
    meshes: &mut Assets<Mesh>,
    mat_handles: &HashMap<ObjRef, Handle<StandardMaterial>>,
    fallback: &Handle<StandardMaterial>,
    sounds: &mut HashMap<String, Vec<Vec<u8>>>,
) -> anyhow::Result<ApexAsset> {
    let d = upk::apex::parse(reg.pkg(r), r.export)?;
    let mut chunks = Vec::with_capacity(d.chunks.len());
    for ch in &d.chunks {
        let mut gfx = ChunkGfx { meshes: vec![], verts: vec![] };
        for (si, sm) in d.submeshes.iter().enumerate() {
            let tris = d.part_tris(si, ch.part);
            if tris.len() < 3 {
                continue;
            }
            let mut remap: HashMap<u32, u32> = HashMap::new();
            let (mut p, mut n, mut uv, mut idx) = (vec![], vec![], vec![], vec![]);
            for &i in tris {
                let ni = *remap.entry(i).or_insert_with(|| {
                    let v = convert::pos(sm.positions[i as usize]);
                    gfx.verts.push(v);
                    p.push(v.to_array());
                    n.push(sm.normals.get(i as usize).map(|&q| convert::dir(q).normalize_or_zero()).unwrap_or(Vec3::Y).to_array());
                    uv.push(sm.uvs.get(i as usize).copied().unwrap_or([0.0, 0.0]));
                    p.len() as u32 - 1
                });
                idx.push(ni);
            }
            let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::RENDER_WORLD);
            mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, p);
            mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, n);
            mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uv);
            mesh.insert_indices(Indices::U32(idx));
            let mat = d.materials.get(si).and_then(|&m| reg.resolve(r.pkg, m)).and_then(|m| mat_handles.get(&m)).cloned().unwrap_or(fallback.clone());
            gfx.meshes.push((meshes.add(mesh), mat));
        }
        chunks.push(gfx);
    }
    // score, name and fracture sound from the DefaultPhysMaterial's GGPhysicalMaterialProperty
    let ps = reg.merged_props(r);
    let pm = match upk::registry::find(&ps, "DefaultPhysMaterial") {
        Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o),
        _ => None,
    };
    let (score, display) = pm.and_then(|pm| upk::physasset::score_info(reg, pm)).unwrap_or((0, "Missing PhysMat".into()));
    let fracture_sound = pm.and_then(|pm| upk::physasset::phys_property(reg, pm)).and_then(|pp| match upk::registry::find(&reg.merged_props(pp), "mFractureSound") {
        Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o),
        _ => None,
    });
    let fracture_sound = fracture_sound.map(|c| {
        let key = reg.path(c);
        if !sounds.contains_key(&key) {
            let ogg: Vec<Vec<u8>> = upk::sound::cue_waves(reg, c).into_iter().filter_map(|w| upk::sound::wave(reg, w)).map(|w| w.ogg).collect();
            sounds.insert(key.clone(), ogg);
        }
        key
    });
    Ok(ApexAsset { name: d.name.clone(), leaves: d.leaf_chunks(), chunks, damage_threshold: d.damage_threshold, display, score, fracture_sound })
}

fn hull(verts: &[Vec3], scale: Vec3, centre: Vec3) -> Option<rp::SharedShape> {
    let pts: Vec<rp::Vector> = verts.iter().map(|&v| convert::to_rapier((v - centre) * scale)).collect();
    if pts.len() < 4 {
        return None;
    }
    rp::SharedShape::convex_hull(&pts)
}

/// Load the level's destructibles and the KActors that turn into one.
#[allow(clippy::too_many_arguments)]
pub fn load(
    reg: &Registry,
    level: &Level,
    kactor_bodies: &[(rp::RigidBodyHandle, usize)],
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    mat_handles: &HashMap<ObjRef, Handle<StandardMaterial>>,
    fallback: &Handle<StandardMaterial>,
    physics: &mut Physics,
) -> Destructibles {
    let t0 = std::time::Instant::now();
    let mut out = Destructibles { rng: 0x9e37_79b9, ..Default::default() };
    let mut asset_index: HashMap<ObjRef, Option<usize>> = HashMap::new();
    let mut get_asset = |out: &mut Destructibles, r: ObjRef| -> Option<usize> {
        *asset_index.entry(r).or_insert_with(|| match build_asset(reg, r, meshes, mat_handles, fallback, &mut out.sounds) {
            Ok(a) => {
                out.assets.push(a);
                Some(out.assets.len() - 1)
            }
            Err(e) => {
                warn!("apex {}: {e:#}", reg.path(r));
                None
            }
        })
    };
    for pl in &level.apex {
        let Some(ar) = reg.resolve(0, pl.asset) else { continue };
        let Some(ai) = get_asset(&mut out, ar) else { continue };
        let xf = convert::xform(&pl.xf);
        let entity = commands
            .spawn((xf, Visibility::default(), Name::new(pl.name.clone())))
            .with_children(|c| {
                for (m, mat) in &out.assets[ai].chunks[0].meshes {
                    c.spawn((Mesh3d(m.clone()), MeshMaterial3d(mat.clone())));
                }
            })
            .id();
        // static collision: the hulls of the leaf chunks (concave shapes stay concave)
        let pose = rp::Pose::from_parts(convert::to_rapier(xf.translation), convert::quat_to_rapier(xf.rotation));
        let mut colliders = Vec::new();
        for &li in &out.assets[ai].leaves {
            if let Some(s) = hull(&out.assets[ai].chunks[li].verts, xf.scale, Vec3::ZERO) {
                colliders.push(physics.world.insert_collider(rp::ColliderBuilder::new(s).position(pose).friction(0.8), None));
            }
        }
        let idx = out.actors.len();
        for &c in &colliders {
            out.by_collider.insert(c, idx);
        }
        let trigger_momentum = float_prop(reg, ObjRef { pkg: 0, export: pl.export }, "mTriggerFractureMomentum", TRIGGER_FRACTURE_MOMENTUM);
        out.actors.push(ApexActor { name: pl.name.clone(), export: pl.export, asset: ai, xf, entity, colliders, fractured: false, trigger_momentum });
    }
    let mut n_k = 0;
    for &(h, export) in kactor_bodies {
        let Some(arch) = kactor_apex(reg, export) else { continue };
        let Some(ar) = asset_of(reg, arch) else { continue };
        let Some(ai) = get_asset(&mut out, ar) else { continue };
        let m = float_prop(reg, arch, "mTriggerFractureMomentum", TRIGGER_FRACTURE_MOMENTUM);
        out.kactors.insert(h, (ai, m));
        n_k += 1;
    }
    info!("destructibles: {} placed, {n_k} breakable KActors, {} assets in {:?}", out.actors.len(), out.assets.len(), t0.elapsed());
    out
}

/// Fracture sounds into the shared sound table.
pub fn register_sounds(mut d: ResMut<Destructibles>, sfx: Option<ResMut<crate::audio::Sfx>>, mut audio: ResMut<Assets<AudioSource>>) {
    let Some(mut sfx) = sfx else { return };
    if d.sounds.is_empty() {
        return;
    }
    for (key, oggs) in d.sounds.drain() {
        let handles: Vec<Handle<AudioSource>> = oggs.into_iter().map(|b| audio.add(AudioSource { bytes: b.into() })).collect();
        sfx.cues.entry(key).or_default().extend(handles);
    }
}

/// Break `asset` at `xf` into its leaf chunks, pushed along `push` (m/s) from `origin`.
fn spawn_debris(d: &mut Destructibles, commands: &mut Commands, phys: &mut Physics, asset: usize, xf: Transform, push: Vec3, origin: Vec3) {
    let leaves = d.assets[asset].leaves.clone();
    for li in leaves {
        let verts = &d.assets[asset].chunks[li].verts;
        if verts.is_empty() {
            continue;
        }
        let c_u = verts.iter().copied().sum::<Vec3>() / verts.len() as f32;
        let Some(shape) = hull(verts, xf.scale, c_u) else { continue };
        let pos = xf.translation + xf.rotation * (c_u * xf.scale);
        let out_dir = (pos - origin).normalize_or_zero();
        let jitter = Vec3::new(d.rand() - 0.5, d.rand() * 0.5, d.rand() - 0.5);
        let v = push + out_dir * 1.5 + jitter;
        let rb = rp::RigidBodyBuilder::dynamic()
            .pose(rp::Pose::from_parts(convert::to_rapier(pos), convert::quat_to_rapier(xf.rotation)))
            .linvel(convert::to_rapier(v))
            .angvel(convert::to_rapier(Vec3::new(d.rand() - 0.5, d.rand() - 0.5, d.rand() - 0.5) * 6.0))
            .linear_damping(0.05)
            .angular_damping(0.3)
            .ccd_enabled(true);
        let h = phys.world.insert_body(rb);
        phys.world.insert_collider(rp::ColliderBuilder::new(shape).density(DEBRIS_DENSITY).friction(0.7).restitution(0.2), Some(h));
        phys.names.insert(h, format!("{} chunk {li}", d.assets[asset].name));
        let meshes = d.assets[asset].chunks[li].meshes.clone();
        commands
            .spawn((Transform { translation: pos, rotation: xf.rotation, scale: xf.scale }, Visibility::default(), PhysBody { handle: h, scale: xf.scale }, Name::new("apex chunk")))
            .with_children(|c| {
                for (m, mat) in meshes {
                    c.spawn((Mesh3d(m), MeshMaterial3d(mat), Transform::from_translation(-c_u)));
                }
            });
        d.debris += 1;
    }
}

/// GGApexDestructibleActor.Fracture: debris, sound, score (when the goat did it).
#[allow(clippy::too_many_arguments)]
fn fracture(
    d: &mut Destructibles,
    i: usize,
    by_goat: bool,
    push: Vec3,
    origin: Vec3,
    commands: &mut Commands,
    phys: &mut Physics,
    sounds: &mut crate::audio::SoundQueue,
    score: &mut crate::score::Score,
) {
    if d.actors[i].fractured {
        return;
    }
    d.actors[i].fractured = true;
    let (asset, xf, entity) = (d.actors[i].asset, d.actors[i].xf, d.actors[i].entity);
    for c in std::mem::take(&mut d.actors[i].colliders) {
        phys.world.remove_collider(c);
        d.by_collider.remove(&c);
    }
    commands.entity(entity).despawn();
    spawn_debris(d, commands, phys, asset, xf, push, origin);
    phys.world_event(crate::physics::WorldEvent::Fractured { export: d.actors[i].export, by_goat });
    finish(d, asset, &d.actors[i].name.clone(), by_goat, xf.translation, sounds, score);
}

fn finish(d: &mut Destructibles, asset: usize, name: &str, by_goat: bool, at: Vec3, sounds: &mut crate::audio::SoundQueue, score: &mut crate::score::Score) {
    let a = &d.assets[asset];
    if let Some(s) = &a.fracture_sound {
        sounds.play(s, Some(at));
    }
    if by_goat {
        score.report(crate::score::fractured(&a.display, a.score));
    }
    d.fractures += 1;
    d.last_fractured = Some(name.to_string());
    if std::env::var_os("GOAT_TRACE").is_some() {
        println!("    fractured {name} ({}) by_goat={by_goat}", a.name);
    }
}

/// Damage, goat bumps and prop impacts against destructibles.
#[allow(clippy::too_many_arguments)]
pub fn collisions(
    mut d: ResMut<Destructibles>,
    mut phys: ResMut<Physics>,
    mut commands: Commands,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut score: ResMut<crate::score::Score>,
    goats: Query<(&Goat, &Transform, Option<&crate::ragdoll::ActiveRagdoll>)>,
    bodies: Query<(Entity, &PhysBody)>,
) {
    let _prof_scope = crate::prof::scope("destruct::collisions");
    if d.actors.is_empty() && d.kactors.is_empty() {
        return;
    }
    let d = &mut *d;
    // (actor or kactor body, by goat, push m/s, origin)
    enum Target {
        Actor(usize),
        KActor(rp::RigidBodyHandle),
    }
    let mut breaks: Vec<(Target, bool, Vec3, Vec3)> = Vec::new();
    if std::env::var_os("GOAT_TRACE").is_some() && !phys.hits.is_empty() {
        let apex = phys.hits.iter().filter(|h| d.by_collider.contains_key(&h.collider)).count();
        println!("    hits {} ({apex} on destructibles)", phys.hits.len());
    }
    // TakeDamage / ReportMomentumOnCollision from hits this frame
    for hit in &phys.hits {
        let momentum_ok = |trigger: f32| hit.kind == HitKind::Collision && hit.momentum.length() >= trigger && hit.goat.is_none_or(|s| s > crate::goat::tune::SPRINT_SPEED);
        if let Some(&i) = d.by_collider.get(&hit.collider) {
            let a = &d.actors[i];
            let damage_ok = hit.kind == HitKind::Ability && hit.damage as f32 >= d.assets[a.asset].damage_threshold;
            if !a.fractured && (damage_ok || momentum_ok(a.trigger_momentum)) {
                breaks.push((Target::Actor(i), hit.goat.is_some(), hit.momentum.normalize_or_zero() * 3.0, hit.point));
            }
        } else if let Some(h) = phys.world.colliders.get(hit.collider).and_then(|c| c.parent()) {
            if let Some(&(ai, trigger)) = d.kactors.get(&h) {
                // GGKActor.ConvertToApex: accumulated damage > threshold, or a hard enough hit
                if hit.damage as f32 > d.assets[ai].damage_threshold || momentum_ok(trigger) {
                    breaks.push((Target::KActor(h), hit.goat.is_some(), hit.momentum.normalize_or_zero() * 3.0, hit.point));
                }
            }
        }
    }
    // the goat bumping into them (GGPawn.Collided -> ReportMomentumOnCollision)
    if let Ok((g, t, rd)) = goats.single() {
        let (pos, vel) = match rd.and_then(|rd| phys.world.bodies.get(rd.pelvis)) {
            Some(b) => (convert::from_rapier(b.translation()), convert::from_rapier(b.linvel())),
            None => (t.translation, g.velocity),
        };
        let speed = vel.length() / convert::S;
        if speed > crate::goat::tune::SPRINT_SPEED {
            let ball = rp::SharedShape::ball(crate::goat::tune::COLLISION_RADIUS * convert::S + 0.15);
            let pose = rp::Pose::from_translation(convert::to_rapier(pos));
            let qp = phys.world.query_pipeline_with_filter(rp::QueryFilter::default().exclude_rigid_body(g.body));
            for (c, _) in qp.intersect_shape(pose, &*ball) {
                let momentum = crate::goat::tune::MASS * speed;
                if let Some(&i) = d.by_collider.get(&c) {
                    if momentum >= d.actors[i].trigger_momentum {
                        breaks.push((Target::Actor(i), true, vel * 0.5, pos));
                    }
                } else if let Some(h) = phys.world.colliders.get(c).and_then(|c| c.parent()) {
                    if d.kactors.get(&h).is_some_and(|&(_, m)| momentum >= m) {
                        breaks.push((Target::KActor(h), true, vel * 0.5, pos));
                    }
                }
            }
        }
    }
    // props (and debris) flying into intact ones: momentum = mass x relative speed
    let mut prop_hits = Vec::new();
    for (i, a) in d.actors.iter().enumerate().filter(|(_, a)| !a.fractured) {
        for &c in &a.colliders {
            for pair in phys.world.narrow_phase.contact_pairs_with(c) {
                if !pair.has_any_active_contact() {
                    continue;
                }
                let other = if pair.collider1 == c { pair.collider2 } else { pair.collider1 };
                let Some(b) = phys.world.colliders.get(other).and_then(|o| o.parent()).and_then(|h| phys.world.bodies.get(h)) else { continue };
                if !b.is_dynamic() {
                    continue;
                }
                let v = convert::from_rapier(b.linvel());
                let momentum = b.mass() * v.length() / convert::S;
                if momentum >= a.trigger_momentum {
                    prop_hits.push((i, v * 0.5, convert::from_rapier(b.translation())));
                }
            }
        }
    }
    breaks.extend(prop_hits.into_iter().map(|(i, v, p)| (Target::Actor(i), false, v, p)));

    for (target, by_goat, push, origin) in breaks {
        match target {
            Target::Actor(i) => fracture(d, i, by_goat, push, origin, &mut commands, &mut phys, &mut sounds, &mut score),
            Target::KActor(h) => {
                let Some((ai, _)) = d.kactors.remove(&h) else { continue };
                // ShutDown the KActor, spawn the archetype where it is and break it at once
                let Some(b) = phys.world.bodies.get(h) else { continue };
                let xf_body = Transform { translation: convert::from_rapier(b.translation()), rotation: convert::quat_from_rapier(*b.rotation()), scale: Vec3::ONE };
                let scale = bodies.iter().find(|(_, pb)| pb.handle == h).map(|(_, pb)| pb.scale).unwrap_or(Vec3::ONE);
                if let Some((e, _)) = bodies.iter().find(|(_, pb)| pb.handle == h) {
                    commands.entity(e).despawn();
                }
                let name = phys.names.get(&h).cloned().unwrap_or_default();
                if let Some(&(export, _)) = phys.actors.get(&h) {
                    phys.world_event(crate::physics::WorldEvent::Fractured { export, by_goat });
                }
                phys.world.remove_body_with_colliders(h, true);
                phys.names.remove(&h);
                phys.prop_info.remove(&h);
                let xf = Transform { scale, ..xf_body };
                spawn_debris(d, &mut commands, &mut phys, ai, xf, push, origin);
                finish(d, ai, &name, by_goat, xf.translation, &mut sounds, &mut score);
            }
        }
    }
}

/// Break a destructible (tests).
pub fn force_fracture(d: &mut Destructibles, i: usize, commands: &mut Commands, phys: &mut Physics, sounds: &mut crate::audio::SoundQueue, score: &mut crate::score::Score) {
    let o = d.actors[i].xf.translation;
    fracture(d, i, true, Vec3::ZERO, o, commands, phys, sounds, score);
}

pub struct DestructPlugin;

impl Plugin for DestructPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Destructibles>()
            .add_systems(Update, (register_sounds, collisions).chain().after(crate::goat::goat_move));
    }
}
