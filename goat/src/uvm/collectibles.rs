//! GGCollectibleActor (the hidden goat trophies, 30 on Goatville): their own PostBeginPlay runs in
//! the VM at start (registers mID in GGGameInfo.mIDsOfCollectiblesOnLevel, destroys the ones the
//! progression already has); here they are drawn with GGPickUpActor's wobble (yaw
//! mWobbleRotationSpeed, bob mWobbleAltitude at mWobbleVerticalSpeed) and picked up like
//! GGPickUpActor.Tick: the goat's feet within sqrt(mPickUpRadiusSq) of Location -> the script's
//! PickedUp (progression + save, achievements, OnCollectibleFound for the quests, sound, effect).

use super::bridge::Host;
use super::plugin::Mutators;
use super::value::Value;
use super::ObjId;
use crate::convert::{self, S};
use bevy::prelude::*;
use std::collections::HashMap;

impl Host {
    /// Map collectibles -> VM PostBeginPlay (before the quest manager counts them).
    pub fn collectibles_start(&mut self) {
        let Some(mp) = self.vm.map_pkg else { return };
        let Some(base) = self.vm.class_by_name("GGCollectibleActor") else { return };
        let p = &self.vm.reg.packages[mp];
        let idx: Vec<i32> = (0..p.exports.len())
            .filter(|&i| p.class_name(i as i32 + 1).contains("Collectible") && p.obj_path(i as i32 + 1).contains(".PersistentLevel."))
            .map(|i| i as i32 + 1)
            .collect();
        for i in idx {
            let Some(o) = self.vm.obj_for_ref(mp, i) else { continue };
            if !self.vm.is_child_of(self.vm.obj(o).class, base) {
                continue;
            }
            self.vm.call_named(o, "PostBeginPlay", vec![]);
            self.collectibles.push(o);
        }
        let alive = self.collectibles.iter().filter(|&&o| !self.vm.obj(o).destroyed).count();
        self.vm.say(format!("== {} collectibles ({alive} not found yet)", self.collectibles.len()));
    }
}

/// One drawn collectible.
struct Shown {
    entity: Entity,
    base: Vec3,
    /// mPickUpRadiusSq (uu^2), wobble settings
    radius_sq: f32,
    yaw_speed: f32,
    bob_speed: f32,
    bob: f32,
    t: f32,
}

#[derive(Resource, Default)]
pub struct Collectibles {
    shown: HashMap<ObjId, Shown>,
    generation: u64,
    built: HashMap<String, Option<std::sync::Arc<super::actors::Built>>>,
    /// Picked up this run (tests).
    pub picked: u32,
}

impl Collectibles {
    /// World positions of the ones still out there (tests).
    pub fn positions(&self) -> Vec<Vec3> {
        self.shown.values().map(|s| s.base).collect()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn collectibles(
    time: Res<Time>,
    mut m: ResMut<Mutators>,
    mut c: ResMut<Collectibles>,
    game: Option<Res<super::actors::GameRegistry>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    goats: Query<&Transform, (With<crate::goat::Goat>, Without<CollectibleMesh>)>,
    mut tfs: Query<&mut Transform, With<CollectibleMesh>>,
) {
    let generation = m.generation;
    let Some(h) = m.host.as_mut() else { return };
    let c = &mut *c;
    if c.generation != generation {
        for (_, s) in c.shown.drain() {
            commands.entity(s.entity).despawn();
        }
        c.generation = generation;
        // draw the ones not found yet
        let list: Vec<ObjId> = h.collectibles.iter().copied().filter(|&o| !h.vm.obj(o).destroyed).collect();
        let mut sa = crate::rig::SkelAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes };
        for o in list {
            let loc = h.vm.get(o, "Location");
            let Value::Struct(ls) = &loc else { continue };
            let (kx, ky, kz) = (h.vm.sym("X"), h.vm.sym("Y"), h.vm.sym("Z"));
            let l = [kx, ky, kz].map(|k| ls.field(k).map(|f| f.float()).unwrap_or(0.0));
            let base = convert::pos(l);
            let comp = h.vm.get(o, "StaticMeshComponent").obj();
            let mesh_path = comp.and_then(|cc| { let v = h.vm.get(cc, "StaticMesh"); super::actors::asset_path_of(&h.vm, &v) }).unwrap_or_else(|| "goat.Mesh.CollectibleMesh".into());
            let Some(built) = super::actors::build(&mut c.built, &h.vm, game.as_ref().map(|g| &*g.0), &mesh_path, &[], &mut sa) else { continue };
            let entity = commands
                .spawn((Transform::from_translation(base), Visibility::default(), Name::new("collectible"), CollectibleMesh))
                .with_children(|ch| {
                    for (mesh, mat) in &built.parts {
                        ch.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone())));
                    }
                })
                .id();
            let f = |h: &mut Host, k: &str, d: f32| match h.vm.get(o, k) {
                Value::Float(x) => x,
                Value::Int(x) => x as f32,
                _ => d,
            };
            let shown = Shown {
                entity,
                base,
                radius_sq: f(h, "mPickUpRadiusSq", 4000.0),
                yaw_speed: f(h, "mWobbleRotationSpeed", 10000.0),
                bob_speed: f(h, "mWobbleVerticalSpeed", 3.5),
                bob: f(h, "mWobbleAltitude", 8.0),
                t: 0.0,
            };
            c.shown.insert(o, shown);
        }
        if !c.shown.is_empty() {
            info!("collectibles: {} to find", c.shown.len());
        }
    }
    if c.shown.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    let feet = goats.single().ok().map(|t| t.translation - Vec3::Y * crate::goat::tune::COLLISION_HEIGHT * S);
    let mut picked = vec![];
    for (&o, s) in c.shown.iter_mut() {
        s.t += dt;
        // GGPickUpActor.Tick wobble: yaw spin and a bob above Location
        let yaw = s.yaw_speed * s.t * std::f32::consts::TAU / 65536.0;
        let lift = ((s.t * s.bob_speed).sin() * s.bob + s.bob) * S;
        if let Ok(mut t) = tfs.get_mut(s.entity) {
            t.translation = s.base + Vec3::Y * lift;
            t.rotation = Quat::from_rotation_y(-yaw);
        }
        if let Some(f) = feet {
            if (f - s.base).length_squared() / (S * S) <= s.radius_sq {
                picked.push(o);
            }
        }
    }
    for o in picked {
        let Some(s) = c.shown.remove(&o) else { continue };
        commands.entity(s.entity).despawn();
        c.picked += 1;
        h.with_vm(move |h| {
            let goat = h.goat;
            h.vm.call_named(o, "PickedUp", vec![Value::Obj(goat)]);
        });
        info!("collectible picked up ({} left)", c.shown.len());
    }
}

/// Marks collectible entities (their own transform is driven here).
#[derive(Component)]
pub struct CollectibleMesh;
