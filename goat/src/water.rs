//! Water volumes (GGWaterVolume / GGCoinVolume, both WaterVolumes): their Touch / UnTouch call
//! GGPawn.SetIsInWater. Entering puts out a fire (SetOnFire(false)); NPCs drown (GGNpc.EnterWater:
//! SetRagdoll(true) + mBubblePSTemplate bubbles at the jaw) and stay down while in the water; the
//! goat plays mWaterSoundCue (the swimming loop) and, ragdolled, is pushed along by its movement
//! input (GGGoat.GiveWaterBoost: Mesh.AddForce(Normal2D(Acceleration) x mWaterAccelRate x mass)).
//! The goat's Jump in water is DoRagdollJump and it only stands up near the shore (goat.rs).

use crate::convert::{self, S};
use crate::goat::{Goat, GoatCamera, InputState};
use crate::npc::{Knocked, Npc, NpcState};
use crate::physics::Physics;
use crate::physics::volumes::{Kind, Volumes};
use crate::ragdoll::ActiveRagdoll;
use bevy::audio::Volume;
use bevy::prelude::*;
use rapier3d::prelude as rp;

const BUBBLES_FX: &str = "Goat_Effects.Effects.Effects_Bubbles_01";
const SWIM_SOUND: &str = "Goat_Sounds.Cue.Effect_Goat_Swimming_loop_cue";
/// Default__GGGoat.mWaterAccelRate and Mass.
const WATER_ACCEL_RATE: f32 = 13.0;
const GOAT_MASS: f32 = 45.0;

#[derive(Component)]
pub struct InWater {
    bubbles: Option<Entity>,
    sound: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct WaterStats {
    /// NPCs that went into the water / times the goat did (tests).
    pub npcs: u32,
    pub goat: u32,
    pub fires_out: u32,
}

fn in_water(vols: &Volumes, p: Vec3) -> bool {
    vols.others.iter().any(|v| matches!(v.kind, Kind::Water) && v.contains(p, 0.5))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn water(
    time: Res<Time>,
    vols: Res<Volumes>,
    mut stats: ResMut<WaterStats>,
    mut phys: ResMut<Physics>,
    input: Res<InputState>,
    cam: Query<&GoatCamera>,
    goats: Query<(Entity, &Goat, Option<&ActiveRagdoll>, Option<&InWater>)>,
    mut npcs: Query<(Entity, &mut Npc, &Transform, Option<&InWater>), Without<Goat>>,
    mut burning: Query<&mut crate::fire::Burning>,
    mut commands: Commands,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut lib: ResMut<crate::particles::FxLibrary>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut sfx: Option<ResMut<crate::audio::Sfx>>,
    mut audio: ResMut<Assets<AudioSource>>,
) {
    let _prof_scope = crate::prof::scope("water::water");
    if !vols.others.iter().any(|v| matches!(v.kind, Kind::Water)) {
        return;
    }
    let dt = time.delta_secs().min(0.05);
    // SetOnFire(false)
    let mut put_out = |e: Entity, stats: &mut WaterStats| {
        if let Ok(mut b) = burning.get_mut(e) {
            if b.left > 0.0 {
                b.left = 0.0;
                stats.fires_out += 1;
            }
        }
    };
    if let Ok((ge, g, rd, inw)) = goats.single() {
        match (g.env.water, inw) {
            (true, None) => {
                put_out(ge, &mut stats);
                stats.goat += 1;
                let mut sound = None;
                if let (Some(gr), Some(sfx)) = (game.as_ref(), sfx.as_deref_mut()) {
                    if !sfx.cues.contains_key(SWIM_SOUND) {
                        crate::audio::load_cue(&gr.0, sfx, &mut audio, SWIM_SOUND, SWIM_SOUND);
                    }
                    if !sfx.muted {
                        if let Some(h) = sfx.cues.get(SWIM_SOUND).and_then(|l| l.first()).cloned() {
                            let vol = crate::uvm::settings::applied().map(|a| a.sfx_volume).unwrap_or(1.0) * 0.8;
                            sound = Some(commands.spawn((AudioPlayer::new(h), PlaybackSettings::LOOP.with_volume(Volume::Linear(vol)), Name::new("swim sound"))).id());
                        }
                    }
                }
                commands.entity(ge).insert(InWater { bubbles: None, sound });
            }
            (false, Some(w)) => {
                if let Some(s) = w.sound {
                    commands.entity(s).despawn();
                }
                commands.entity(ge).remove::<InWater>();
            }
            _ => {}
        }
        // GiveWaterBoost: ragdolled in the water, the movement input pushes the body
        if let (true, Some(rd)) = (g.env.water, rd) {
            let i = &input.0;
            if i.forward != 0.0 || i.right != 0.0 {
                let yaw = cam.single().map(|c| c.yaw).unwrap_or(g.yaw);
                let dir = (Quat::from_rotation_y(yaw) * Vec3::new(i.right, 0.0, -i.forward)).with_y(0.0).normalize_or_zero();
                let impulse = dir * WATER_ACCEL_RATE * GOAT_MASS * S * dt;
                if let Some(rb) = phys.world.bodies.get_mut(rd.pelvis) {
                    rb.apply_impulse(convert::to_rapier(impulse), true);
                }
            }
        }
    }
    for (e, mut n, t, inw) in &mut npcs {
        let inside = in_water(&vols, t.translation);
        match (inside, inw) {
            (true, None) => {
                // GGNpc.EnterWater: "I'M DROWNING :((("
                put_out(e, &mut stats);
                stats.npcs += 1;
                if n.state != NpcState::Ragdoll {
                    commands.entity(e).insert(Knocked { velocity: Vec3::ZERO });
                }
                let bubbles = game.as_ref().and_then(|g| lib.get(&[&g.0], BUBBLES_FX, &mut materials, &mut images)).map(|fx| {
                    commands.spawn((Transform::from_translation(Vec3::Y * 0.7), Visibility::default(), crate::particles::ParticleFx::new(fx, true), Name::new("bubbles"))).id()
                });
                if let Some(b) = bubbles {
                    commands.entity(e).add_child(b);
                }
                commands.entity(e).insert(InWater { bubbles, sound: None });
            }
            (false, Some(w)) => {
                // ExitWater: EnableBubbles(false)
                if let Some(b) = w.bubbles {
                    commands.entity(b).despawn();
                }
                commands.entity(e).remove::<InWater>();
            }
            (true, Some(_)) => {
                // drowning NPCs stay down while in the water
                if n.state == NpcState::Ragdoll {
                    n.timer = n.timer.max(1.0);
                }
            }
            _ => {}
        }
    }
    let _ = rp::Vector::ZERO;
}

pub struct WaterPlugin;

impl Plugin for WaterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WaterStats>().add_systems(Update, water.after(crate::physics::volumes::environment));
    }
}
