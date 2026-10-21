//! Explosive props: GGExplosiveActorAbstract (gas cans, barrels, fire extinguishers, gas pumps
//! via GGExplosiveActorContent) and GGExplosiveActorWreckable (cars: swap to the wreck mesh).
//!
//! ShouldExplode: damage >= mExplosionTriggerDamage (30; headbutts deal 200, explosions 500+),
//! or collision momentum >= mExplosionTriggerMomentum (1800) - from the goat only while faster
//! than 1.2 x mSprintSpeed, from a hard impact on anything else (mass independent: the hit
//! velocity). Explode: HurtRadius(mExplosionDamage, mExplosionDamageRadius, full damage,
//! mExplosiveMomentum) from the phys material's GGPhysicalMaterialProperty, its explosion sound and
//! effect; GGTrickExplosion (10) and GGTrickHurtByExplosion (each scored victim) for the combo.

use crate::convert::{self, S};
use crate::goat::Goat;
use crate::physics::{Hit, HitKind, Physics};
use bevy::prelude::*;
use rapier3d::prelude as rp;
use std::collections::HashMap;
use upk::registry::{ObjRef, Registry};

/// Default__GGExplosiveActorAbstract
const TRIGGER_DAMAGE: i32 = 30;
const TRIGGER_MOMENTUM: f32 = 1800.0;
const DEFAULT_DAMAGE: f32 = 500.0;
const DEFAULT_RADIUS: f32 = 1000.0;
const DEFAULT_MOMENTUM: f32 = 500.0;
/// Default__GGNpc Mass.
const NPC_MASS: f32 = 80.0;
/// Cap on launch speeds (m/s), like headbutts.
const MAX_LAUNCH: f32 = 30.0;

pub struct Explosive {
    pub name: String,
    export: usize,
    entity: Entity,
    damage: f32,
    /// uu
    radius: f32,
    /// kg uu/s
    momentum: f32,
    trigger_damage: i32,
    trigger_momentum: f32,
    effect: Option<String>,
    sound: Option<String>,
    /// GGExplosiveActorWreckable: becomes its wreck (mesh, material) instead of vanishing.
    wreck: Option<(Vec<(Handle<Mesh>, Handle<StandardMaterial>)>, Vec3)>,
    prev_speed: f32,
    score: i32,
}

/// The wreck meshes of wreckable explosives, built with the level (world.rs).
#[derive(Resource, Default)]
pub struct Wrecks(pub HashMap<rp::RigidBodyHandle, (Vec<(Handle<Mesh>, Handle<StandardMaterial>)>, Vec3)>);

#[derive(Resource, Default)]
pub struct Explosives {
    pub items: HashMap<rp::RigidBodyHandle, Explosive>,
    ready: bool,
    /// The scorch decal (GGPhysicalMaterialProperty.mExplosionDecalMat's texture), loaded on the
    /// first explosion.
    decal: Option<Option<Handle<Image>>>,
    /// Explosions so far (tests) and the last one's origin.
    pub exploded: u32,
    pub last: Option<(String, Vec3)>,
}

fn props_float(ps: &[(usize, upk::props::Prop)], k: &str) -> Option<f32> {
    match upk::registry::find(ps, k) {
        Some((_, upk::props::Value::Float(f))) => Some(*f),
        Some((_, upk::props::Value::Int(i))) => Some(*i as f32),
        _ => None,
    }
}

/// Find the explosive props among the level's bodies once the registry is up.
#[allow(clippy::too_many_arguments)]
fn init(
    mut ex: ResMut<Explosives>,
    phys: Res<Physics>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut wrecks: ResMut<Wrecks>,
    sfx: Option<ResMut<crate::audio::Sfx>>,
    mut audio: ResMut<Assets<AudioSource>>,
) {
    if ex.ready {
        return;
    }
    let (Some(game), Some(mut sfx)) = (game, sfx) else { return };
    if phys.actors.is_empty() {
        return;
    }
    ex.ready = true;
    let reg: &Registry = &game.0;
    let map = &reg.packages[0];
    let mut n = 0;
    for (&h, &(export, entity)) in &phys.actors {
        let class = map.class_name(export as i32 + 1);
        if !class.starts_with("GGExplosiveActor") {
            continue;
        }
        let actor = ObjRef { pkg: 0, export };
        let aps = reg.merged_props(actor);
        let mut e = Explosive {
            name: map.obj_name(export as i32 + 1),
            export,
            entity,
            damage: DEFAULT_DAMAGE,
            radius: DEFAULT_RADIUS,
            momentum: DEFAULT_MOMENTUM,
            trigger_damage: props_float(&aps, "mExplosionTriggerDamage").map(|f| f as i32).unwrap_or(TRIGGER_DAMAGE),
            trigger_momentum: props_float(&aps, "mExplosionTriggerMomentum").unwrap_or(TRIGGER_MOMENTUM),
            effect: None,
            sound: None,
            wreck: wrecks.0.remove(&h),
            prev_speed: 0.0,
            score: phys.prop_info.get(&h).map(|p| p.0).unwrap_or(0),
        };
        // PostBeginPlay: damage, radius and momentum from the phys material property
        if let Some(pp) = phys.physmat.get(&h).and_then(|&pm| upk::physasset::phys_property(reg, pm)) {
            let ps: Vec<(usize, upk::props::Prop)> = reg.merged_props(pp);
            if let Some(v) = props_float(&ps, "mExplosionDamage") {
                e.damage = v;
            }
            if let Some(v) = props_float(&ps, "mExplosionDamageRadius") {
                e.radius = v;
            }
            if let Some(v) = props_float(&ps, "mExplosiveMomentum") {
                e.momentum = v;
            }
            let obj = |k: &str| match upk::registry::find(&ps, k) {
                Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o),
                _ => None,
            };
            e.effect = obj("mExplosionEffectTemplate").map(|r| reg.path(r));
            if let Some(c) = obj("mExplosionSound") {
                let key = reg.path(c);
                if !sfx.cues.contains_key(&key) {
                    crate::audio::load_cue(reg, &mut sfx, &mut audio, &key, &key);
                }
                e.sound = Some(key);
            }
        }
        ex.items.insert(h, e);
        n += 1;
    }
    info!("explosives: {n} ({} wreckable)", ex.items.values().filter(|e| e.wreck.is_some()).count());
}

/// A pending explosion.
struct Blast {
    body: rp::RigidBodyHandle,
    by_goat: bool,
}

#[allow(clippy::too_many_arguments)]
fn triggers(
    mut ex: ResMut<Explosives>,
    mut phys: ResMut<Physics>,
    mut commands: Commands,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut score: ResMut<crate::score::Score>,
    mut goats: Query<(Entity, &mut Goat, &Transform, Option<&crate::ragdoll::ActiveRagdoll>)>,
    npcs: Query<&crate::npc::Npc>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut lib: ResMut<crate::particles::FxLibrary>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut entities: Query<&mut Transform, Without<Goat>>,
) {
    let _prof_scope = crate::prof::scope("explosive::triggers");
    let ex = &mut *ex;
    if ex.items.is_empty() {
        return;
    }
    let mut blasts: Vec<Blast> = Vec::new();
    // TakeDamage / ReportMomentumOnCollision this frame (chain reactions come through here too)
    for hit in &phys.hits {
        let Some(h) = phys.world.colliders.get(hit.collider).and_then(|c| c.parent()) else { continue };
        let Some(e) = ex.items.get(&h) else { continue };
        let on_damage = hit.damage >= e.trigger_damage;
        let on_momentum = hit.kind == HitKind::Collision
            && hit.momentum.length() >= e.trigger_momentum
            && hit.goat.is_none_or(|s| s > crate::goat::tune::SPRINT_SPEED * 1.2);
        if on_damage || on_momentum {
            blasts.push(Blast { body: h, by_goat: hit.goat.is_some() });
        }
    }
    // Collided with the world: a hard stop at >= mExplosionTriggerMomentum uu/s
    let handles: Vec<rp::RigidBodyHandle> = ex.items.keys().copied().collect();
    for h in handles {
        let Some(b) = phys.world.bodies.get(h) else { continue };
        let speed = b.linvel().length() / S;
        let e = ex.items.get_mut(&h).unwrap();
        if e.prev_speed >= e.trigger_momentum && speed < e.prev_speed * 0.6 {
            blasts.push(Blast { body: h, by_goat: false });
        }
        e.prev_speed = speed;
    }
    // the goat ramming one faster than 1.2 x sprint speed
    if let Ok((_, g, t, rd)) = goats.single() {
        let speed = g.velocity.length() / S;
        if rd.is_none() && speed > crate::goat::tune::SPRINT_SPEED * 1.2 && crate::goat::tune::MASS * speed >= TRIGGER_MOMENTUM {
            let ball = rp::SharedShape::ball(crate::goat::tune::COLLISION_RADIUS * S + 0.15);
            let qp = phys.world.query_pipeline_with_filter(rp::QueryFilter::default().exclude_rigid_body(g.body));
            for (c, _) in qp.intersect_shape(rp::Pose::from_translation(convert::to_rapier(t.translation)), &*ball) {
                if let Some(h) = phys.world.colliders.get(c).and_then(|c| c.parent()).filter(|h| ex.items.contains_key(h)) {
                    blasts.push(Blast { body: h, by_goat: true });
                }
            }
        }
    }
    let regs: Vec<std::sync::Arc<Registry>> = game.iter().map(|g| g.0.clone()).collect();
    for blast in blasts {
        let Some(e) = ex.items.remove(&blast.body) else { continue };
        let Some(b) = phys.world.bodies.get(blast.body) else { continue };
        let origin = convert::from_rapier(b.translation());
        // HurtRadius (bDoFullDamage): every actor within the radius takes the full damage, pushed
        // away from the origin with the full momentum
        let ball = rp::SharedShape::ball(e.radius * S);
        let mut victims = Vec::new();
        {
            let qp = phys.world.query_pipeline_with_filter(rp::QueryFilter::default().exclude_rigid_body(blast.body));
            for (c, _) in qp.intersect_shape(rp::Pose::from_translation(convert::to_rapier(origin)), &*ball) {
                victims.push(c);
            }
        }
        let mut pushed: Vec<rp::RigidBodyHandle> = Vec::new();
        for c in victims {
            let Some(col) = phys.world.colliders.get(c) else { continue };
            let p = convert::from_rapier(col.position().translation);
            let parent_body = col.parent();
            let dir = (p - origin + Vec3::Y * 0.3).normalize_or(Vec3::Y);
            phys.hits.push(Hit { collider: c, damage: e.damage as i32, momentum: dir * e.momentum, kind: HitKind::Explosion, goat: None, point: p });
            if let Some(&npc) = phys.owners.get(&c) {
                // GGNpc.TakeDamage: ragdoll with the momentum
                let v = (dir * (e.momentum * S / NPC_MASS)).clamp_length_max(MAX_LAUNCH);
                commands.entity(npc).insert((crate::npc::Knocked { velocity: v }, crate::fire::Ignite));
                if let Ok(n) = npcs.get(npc) {
                    score.report(hurt_trick(n.score));
                }
                continue;
            }
            let Some(parent) = parent_body else { continue };
            if pushed.contains(&parent) {
                continue;
            }
            pushed.push(parent);
            if let Ok((ge, mut g, _, rd)) = goats.single_mut() {
                let is_goat = parent == g.body || rd.is_some_and(|rd| rd.bodies.iter().any(|&(_, h)| h == parent));
                if is_goat {
                    // GGPawn.ShouldBeOnFire: explosive damage sets the goat alight
                    commands.entity(ge).insert(crate::fire::Ignite);
                    // GGGoat.TakeDamage: GGDamageTypeExplosiveActor ragdolls the victim, AddImpulse(momentum)
                    let v = (dir * (e.momentum * S / crate::goat::tune::MASS)).clamp_length_max(MAX_LAUNCH);
                    match rd {
                        Some(rd) => {
                            for &(_, h) in &rd.bodies {
                                if let Some(rb) = phys.world.bodies.get_mut(h) {
                                    rb.set_linvel(rb.linvel() + convert::to_rapier(v), true);
                                }
                            }
                        }
                        None => {
                            g.velocity = v;
                            crate::goat::start_ragdoll(&mut commands, ge, &mut g, &mut phys);
                        }
                    }
                    continue;
                }
            }
            if !phys.wake(parent) {
                continue;
            }
            if let Some(rb) = phys.world.bodies.get_mut(parent) {
                let j = (e.momentum * S).min(rb.mass() * MAX_LAUNCH);
                rb.apply_impulse(convert::to_rapier(dir * j), true);
            }
            if let Some(&(s, _)) = phys.prop_info.get(&parent) {
                score.report(hurt_trick(s));
            }
        }
        // effects
        if let Some(s) = &e.sound {
            sounds.play(s, Some(origin));
        }
        if let Some(path) = &e.effect {
            let rs: Vec<&Registry> = regs.iter().map(|r| &**r).collect();
            if let Some(fx) = lib.get(&rs, path, &mut materials, &mut images) {
                commands.spawn((Transform::from_translation(origin), Visibility::default(), crate::particles::ParticleFx::new(fx, true), Expire(6.0), Name::new("explosion")));
            }
        }
        // GGExplosionLight (UDKExplosionLight TimeShift: bright for 0.3 s, out at 0.4 s)
        commands.spawn((
            PointLight { color: Color::srgb_u8(255, 160, 60), intensity: 6.0e6, range: (e.radius * S).clamp(6.0, 25.0), shadow_maps_enabled: false, ..default() },
            Transform::from_translation(origin + Vec3::Y * 0.6),
            ExplosionLight { t: 0.0 },
            Name::new("explosion light"),
        ));
        // SpawnExplosionDecal: the phys property's decal (Decals_Dirt_02) on the ground below,
        // mDecalSize 200 x 200, gone after mDecalLifeTime 60 + mDecalFadeTime 2
        if ex.decal.is_none() {
            ex.decal = Some(regs.first().and_then(|r| load_decal(r, &mut images)));
        }
        if let Some(Some(img)) = ex.decal.clone().filter(|_| std::env::var_os("GOAT_NO_DECALS").is_none()) {
            // only on flat ground just below the blast (a box this size also marks nearby walls)
            let ray = rp::Ray::new(convert::to_rapier(origin + Vec3::Y * 0.5), rp::Vector::NEG_Y);
            let floor = phys.world.cast_ray_and_get_normal(&ray, 1.5, true, rp::QueryFilter::only_fixed()).filter(|(_, h)| h.normal.y > 0.8);
            if let Some((_, h)) = floor {
                let hit = origin + Vec3::Y * (0.5 - h.time_of_impact);
                commands.spawn((
                    bevy::light::ClusteredDecal { base_color_texture: Some(img), ..default() },
                    Transform::from_translation(hit).looking_to(Vec3::NEG_Y, Vec3::X).with_scale(Vec3::new(200.0 * S, 200.0 * S, 0.3)),
                    Expire(62.0),
                    Name::new("scorch"),
                ));
            }
        }
        // GGGameInfo.OnExplosion -> GGTrickExplosion
        score.report(crate::score::explosion());
        // the actor itself: ShutDown, or GGExplosiveActorWreckable's wreck + mExplosiveMomentumOnSelf
        match e.wreck {
            Some((parts, self_momentum)) => {
                commands.entity(e.entity).despawn_related::<Children>();
                commands.entity(e.entity).with_children(|c| {
                    for (m, mat) in parts {
                        c.spawn((Mesh3d(m), MeshMaterial3d(mat)));
                    }
                });
                phys.wake(blast.body);
                if let Some(rb) = phys.world.bodies.get_mut(blast.body) {
                    let j = (convert::dir(self_momentum.to_array()) * self_momentum.length() * S).clamp_length_max(rb.mass() * MAX_LAUNCH);
                    rb.apply_impulse(convert::to_rapier(j), true);
                }
            }
            None => {
                commands.entity(e.entity).despawn();
                phys.world.remove_body_with_colliders(blast.body, true);
                phys.names.remove(&blast.body);
                phys.prop_info.remove(&blast.body);
            }
        }
        if let Ok(mut t) = entities.get_mut(e.entity) {
            t.translation = origin;
        }
        ex.exploded += 1;
        phys.world_event(crate::physics::WorldEvent::Exploded { export: e.export, by_goat: blast.by_goat });
        ex.last = Some((e.name.clone(), origin));
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    explosion {} at {origin:.2}: damage {} radius {} momentum {}", e.name, e.damage, e.radius, e.momentum);
        }
    }
}

/// GGTrickHurtByExplosion: the damaged actor's score, no name.
fn hurt_trick(s: i32) -> crate::score::Trick {
    crate::score::Trick {
        id: crate::score::TrickId::HurtByExplosion,
        name: String::new(),
        clean_name: "HurtByExplosion".into(),
        score: s,
        adds_multiplier: false,
        has_name: false,
        min_score: 1,
    }
}

/// The default explosion decal's texture (alpha from its own channel, else darkness).
fn load_decal(reg: &Registry, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let pp = reg.find_path("Default__GGPhysicalMaterialProperty")?;
    let ps = reg.merged_props(pp);
    let mat = match upk::registry::find(&ps, "mExplosionDecalMat") {
        Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o)?,
        _ => reg.find_path("Decals.Decals.Decals_Dirt_02_TimeVarying")?,
    };
    // a BLEND_Modulate DecalMaterial: Emissive = lerp(tint (0.5, 0.4, 0.4) x texture, ...,
    // texture alpha); take the TextureSample of the instance's parent
    let mut cur = Some(mat);
    let mut tex_ref = None;
    for _ in 0..4 {
        let Some(m) = cur else { break };
        let ps = reg.merged_props(m);
        if let Some((pk, v)) = upk::registry::find(&ps, "Expressions") {
            for e in upk::material::object_array(Some(v)).into_iter().filter_map(|o| reg.resolve(pk, o)) {
                if reg.class_name(e).contains("TextureSample") {
                    if let Some((tp, upk::props::Value::Object(o))) = upk::registry::find(&reg.merged_props(e), "Texture") {
                        tex_ref = reg.resolve(tp, *o);
                        break;
                    }
                }
            }
        }
        if tex_ref.is_some() {
            break;
        }
        cur = match upk::registry::find(&ps, "Parent") {
            Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o),
            _ => None,
        };
    }
    let Some(t) = tex_ref else {
        warn!("explosion decal: no texture in {}", reg.path(mat));
        return None;
    };
    let tex = upk::texture::Texture::parse(reg.pkg(t), t.export).ok()?;
    let mut img = tex.decode(reg.pkg(t), t.export, 512).ok()?;
    let has_alpha = img.rgba.chunks_exact(4).any(|c| c[3] < 250);
    // modulate: darken with the tinted texture, as much as its alpha says
    for c in img.rgba.chunks_exact_mut(4) {
        let a = if has_alpha { c[3] } else { 255 - ((c[0] as u32 + c[1] as u32 + c[2] as u32) / 3).min(255) as u8 };
        c[0] = (c[0] as f32 * 0.5 * 0.5) as u8;
        c[1] = (c[1] as f32 * 0.4 * 0.5) as u8;
        c[2] = (c[2] as f32 * 0.4 * 0.5) as u8;
        c[3] = a;
    }
    info!("explosion decal: {} ({}x{}, alpha {})", reg.path(t), img.width, img.height, if has_alpha { "own" } else { "from darkness" });
    Some(images.add(crate::world::make_image(img, tex.srgb)))
}

/// An explosion's light: full for 0.3 s, gone at 0.4 s.
#[derive(Component)]
pub struct ExplosionLight {
    t: f32,
}

fn explosion_lights(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &mut ExplosionLight, &mut PointLight)>) {
    for (e, mut l, mut p) in &mut q {
        l.t += time.delta_secs();
        if l.t >= 0.4 {
            commands.entity(e).despawn();
        } else if l.t > 0.3 {
            p.intensity = 6.0e6 * (0.4 - l.t) / 0.1;
        }
    }
}

/// Despawn after a while (one-shot effects).
#[derive(Component)]
pub struct Expire(pub f32);

fn expire(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &mut Expire)>) {
    for (e, mut x) in &mut q {
        x.0 -= time.delta_secs();
        if x.0 <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

/// Blow one up (tests).
pub fn force(ex: &Explosives, phys: &mut Physics, h: rp::RigidBodyHandle, by_goat: bool) {
    if !ex.items.contains_key(&h) {
        return;
    }
    if let Some(c) = phys.world.bodies.get(h).and_then(|b| b.colliders().first().copied()) {
        phys.hits.push(Hit { collider: c, damage: 10_000, momentum: Vec3::ZERO, kind: HitKind::Ability, goat: by_goat.then_some(0.0), point: Vec3::ZERO });
    }
}

impl Explosives {
    /// (body, name, world position) of the unexploded ones.
    pub fn list<'a>(&'a self, phys: &'a Physics) -> impl Iterator<Item = (rp::RigidBodyHandle, &'a str, Vec3)> + 'a {
        self.items.iter().filter_map(|(&h, e)| phys.world.bodies.get(h).map(|b| (h, e.name.as_str(), convert::from_rapier(b.translation()))))
    }
    pub fn is_wreckable(&self, h: rp::RigidBodyHandle) -> bool {
        self.items.get(&h).is_some_and(|e| e.wreck.is_some())
    }
}

pub struct ExplosivePlugin;

impl Plugin for ExplosivePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Explosives>()
            .init_resource::<Wrecks>()
            .add_systems(Update, (init, triggers.after(crate::destruct::collisions)).chain().after(crate::goat::goat_move))
            .add_systems(Update, (expire, explosion_lights));
    }
}

