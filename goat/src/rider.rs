//! The race jockey (GGGoat.SpawnGoatRider, called by GGGameInfoRace.PostLogin): a copy of a
//! random NPC's mesh, physics asset and first material attached to the goat mesh's RideSocket
//! (GGNpc.RideTheGoat) in the GoatRider state, which loops ChairIdle at rate 0.1.
//! RemoveGoatRider takes it off again. The script calls arrive through VM overrides.

use crate::goat::Goat;
use crate::rig::{self, Rig};
use bevy::prelude::*;
use std::sync::Arc;

/// Pending SpawnGoatRider (true) / RemoveGoatRider (false) calls from the VM.
static REQUESTS: std::sync::Mutex<Vec<bool>> = std::sync::Mutex::new(Vec::new());

pub fn request(spawn: bool) {
    if let Ok(mut q) = REQUESTS.lock() {
        q.push(spawn);
    }
}

/// An NPC's look, kept for copying (npc.rs).
#[derive(Component, Clone)]
pub struct RiderLook {
    pub data: Arc<rig::SkelData>,
    pub overrides: Vec<Option<Handle<StandardMaterial>>>,
    /// The mesh component's Translation (kept relative to the socket when attached).
    pub translation: Vec3,
}

/// The rider's holder entity under the goat's RideSocket bone.
#[derive(Resource, Default)]
pub struct GoatRider(pub Option<Entity>);

fn riders(
    mut commands: Commands,
    mut rider: ResMut<GoatRider>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    looks: Query<&RiderLook>,
    goats: Query<&Rig, With<Goat>>,
    mut socket: Local<Option<Option<(String, Transform)>>>,
    mut pick: Local<u32>,
    time: Res<Time>,
    mut forced: Local<bool>,
    gts: Query<&GlobalTransform>,
    goat_t: Query<&GlobalTransform, With<Goat>>,
) {
    if let (Some(e), true) = (rider.0, std::env::var_os("GOAT_TRACE").is_some()) {
        if let (Ok(r), Ok(g)) = (gts.get(e), goat_t.single()) {
            if (time.elapsed_secs() * 2.0).floor() != ((time.elapsed_secs() - time.delta_secs()) * 2.0).floor() {
                println!("    rider at {:.2?} (goat {:.2?}) scale {:.2?} up {:.2?}", r.translation(), g.translation(), r.to_scale_rotation_translation().0, r.rotation() * Vec3::Y);
            }
        }
    }
    // GOAT_RIDER=1: put a rider on the goat after a second (checking the look without a race)
    if !*forced && time.elapsed_secs() > 1.0 && std::env::var_os("GOAT_RIDER").is_some() && !looks.is_empty() {
        *forced = true;
        request(true);
    }
    let reqs = match REQUESTS.lock() {
        Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
        _ => return,
    };
    for spawn in reqs {
        if let Some(e) = rider.0.take() {
            commands.entity(e).despawn();
        }
        if !spawn {
            info!("goat rider removed");
            continue;
        }
        let Ok(goat_rig) = goats.single() else { continue };
        // the goat mesh's RideSocket (bone + transform relative to it)
        if socket.is_none() {
            *socket = Some(game.as_ref().and_then(|g| {
                let mesh = g.0.find_path("goat.Mesh.goat")?;
                crate::vehicle::socket(&g.0, mesh, "RideSocket")
            }));
        }
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    RideSocket {:?}", socket);
        }
        let Some((bone, local)) = socket.clone().flatten() else {
            warn!("goat rider: the goat mesh has no RideSocket");
            continue;
        };
        let Some(joint) = goat_rig.data.bone_names.iter().position(|b| b.eq_ignore_ascii_case(&bone)).and_then(|k| goat_rig.joints.get(k).copied()) else { continue };
        // someNPCs[Rand(Length)]: NPCs with a mesh, physics asset and material (here: that can sit)
        let candidates: Vec<&RiderLook> = looks.iter().filter(|l| l.data.clip("ChairIdle").is_some()).collect();
        if candidates.is_empty() {
            warn!("goat rider: no NPC to copy");
            continue;
        }
        *pick = pick.wrapping_mul(1103515245).wrapping_add(12345 + candidates.len() as u32);
        let look = candidates[(*pick >> 8) as usize % candidates.len()].clone();
        let holder = commands.spawn((local, Visibility::default(), Name::new("goat rider"))).id();
        commands.entity(joint).add_child(holder);
        rig::spawn(&mut commands, holder, look.data.clone(), Transform::from_translation(look.translation), &look.overrides, "ChairIdle");
        // NPCAnimationInfo.AnimationRate 0.1
        commands.entity(holder).queue(|mut w: EntityWorldMut| {
            if let Some(mut a) = w.get_mut::<rig::Animator>() {
                a.speed = 0.1;
            }
        });
        info!("goat rider spawned at RideSocket (bone {bone})");
        rider.0 = Some(holder);
    }
}

pub struct RiderPlugin;

impl Plugin for RiderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<GoatRider>().add_systems(Update, riders);
    }
}
