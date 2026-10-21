//! The Rust side of races (the rules run in the VM, uvm/race.rs): checkpoint meshes (the
//! mMeshToUse StaticMeshActors, hidden outside races) shown while their holder for player 0 is
//! visible and not deactivated, the goat held still during the countdown (state RaceCountdown),
//! and for BikeRace a bicycle placed in front of the goat (PostLogin spawns one in the script).

use bevy::prelude::*;

/// A checkpoint's mesh (its StaticMeshActor name).
#[derive(Component)]
pub struct CheckpointMesh(pub String);

static FROZEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The race countdown holds the goat in place.
pub fn frozen() -> bool {
    FROZEN.load(std::sync::atomic::Ordering::Relaxed)
}

#[derive(Resource, Default)]
pub struct RaceState {
    bike_placed: bool,
    /// Checkpoints shown last frame (tests).
    pub shown: usize,
    pub total: usize,
}

fn race(
    mut st: ResMut<RaceState>,
    mut meshes: Query<(&CheckpointMesh, &mut Visibility)>,
    goats: Query<&Transform, (With<crate::goat::Goat>, Without<crate::vehicle::Bike>)>,
    bikes: Query<(&crate::vehicle::Bike, &Transform)>,
    mut phys: ResMut<crate::physics::Physics>,
) {
    let Some(view) = crate::uvm::race::view() else {
        FROZEN.store(false, std::sync::atomic::Ordering::Relaxed);
        return;
    };
    FROZEN.store(view.frozen, std::sync::atomic::Ordering::Relaxed);
    let mut shown = 0;
    for (m, mut vis) in &mut meshes {
        let on = view.checkpoints.iter().any(|(n, s)| *s && n.eq_ignore_ascii_case(&m.0));
        shown += on as usize;
        let want = if on { Visibility::Inherited } else { Visibility::Hidden };
        if *vis != want {
            *vis = want;
        }
    }
    st.shown = shown;
    st.total = view.checkpoints.len();
    // GGGameInfoRace.PostLogin (BikeRace): a bike 200 uu in front of the goat
    if !st.bike_placed && crate::uvm::race::race_id().as_deref() == Some("BikeRace") {
        let Ok(gt) = goats.single() else { return };
        let fwd = gt.rotation * -Vec3::Z;
        let at = gt.translation + fwd * (150.0 * crate::convert::S) + Vec3::Y * 0.2;
        if let Some((b, _)) = bikes.iter().filter(|(b, _)| b.def.name == "Bicycle").min_by(|a, b| a.1.translation.distance(at).total_cmp(&b.1.translation.distance(at))) {
            if let Some(rb) = phys.world.bodies.get_mut(b.body) {
                let heading = Quat::from_rotation_arc(Vec3::X, fwd.with_y(0.0).normalize_or(Vec3::X));
                rb.set_position(rapier3d::prelude::Pose::from_parts(crate::convert::to_rapier(at), crate::convert::quat_to_rapier(heading)), true);
                rb.set_linvel(rapier3d::prelude::Vector::ZERO, true);
                rb.set_angvel(rapier3d::prelude::Vector::ZERO, true);
            }
        }
        st.bike_placed = true;
    }
}

pub struct RacePlugin;

impl Plugin for RacePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<RaceState>().add_systems(Update, race);
    }
}
