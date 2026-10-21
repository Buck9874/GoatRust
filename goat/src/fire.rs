//! GGPawn fire: pawns hurt by an explosion (ShouldBeOnFire: GGDamageTypeExplosiveActor) burn for
//! FRand() * 2 + 8 seconds with mFirePSTemplate (Goat_Effects.Effects.Effects_Fire_01) on their
//! spine and mFireBurnSound looping; a burning pawn sets the pawns it touches alight
//! (GGPawn.Collided); NPCs panic while they burn (GGNpc.OnSetOnFire -> Panic).

use crate::goat::Goat;
use crate::npc::{Npc, NpcState};
use bevy::audio::Volume;
use bevy::prelude::*;

const FIRE_FX: &str = "Goat_Effects.Effects.Effects_Fire_01";
const BURN_SOUND: &str = "Heist_Audio.Cue.SFX_Generic_Fire_Mono_Cue";
/// Pawns closer than this to a burning one catch fire (metres).
const SPREAD_DISTANCE: f32 = 0.9;

/// Set a pawn on fire (inserted by explosions).
#[derive(Component)]
pub struct Ignite;

#[derive(Component)]
pub struct Burning {
    pub left: f32,
    fx: Option<Entity>,
    sound: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct FireStats {
    /// Pawns set alight so far (tests).
    pub ignited: u32,
    rng: u32,
}

impl FireStats {
    fn rand(&mut self) -> f32 {
        let mut x = self.rng.max(0x2545_f491);
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
}

#[allow(clippy::too_many_arguments)]
fn ignite(
    mut commands: Commands,
    mut stats: ResMut<FireStats>,
    new: Query<(Entity, Option<&Burning>), With<Ignite>>,
    mut npcs: Query<&mut Npc>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut lib: ResMut<crate::particles::FxLibrary>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    sfx: Option<ResMut<crate::audio::Sfx>>,
    mut audio: ResMut<Assets<AudioSource>>,
) {
    let _prof_scope = crate::prof::scope("fire::ignite");
    if new.is_empty() {
        return;
    }
    let mut sfx = sfx;
    for (e, burning) in &new {
        commands.entity(e).remove::<Ignite>();
        if burning.is_some() {
            continue;
        }
        let left = stats.rand() * 2.0 + 8.0;
        stats.ignited += 1;
        // the fire effect on the spine (about half a metre above the pawn's origin)
        let fx = game.as_ref().and_then(|g| lib.get(&[&g.0], FIRE_FX, &mut materials, &mut images)).map(|fx| {
            commands
                .spawn((Transform::from_translation(Vec3::Y * 0.3), Visibility::default(), crate::particles::ParticleFx::new(fx, true), Name::new("fire")))
                .id()
        });
        if let Some(f) = fx {
            commands.entity(e).add_child(f);
        }
        // the burn loop (silent in test runs)
        let mut sound = None;
        if let (Some(g), Some(sfx)) = (game.as_ref(), sfx.as_deref_mut()) {
            if !sfx.cues.contains_key(BURN_SOUND) {
                crate::audio::load_cue(&g.0, sfx, &mut audio, BURN_SOUND, BURN_SOUND);
            }
            if !sfx.muted {
                if let Some(h) = sfx.cues.get(BURN_SOUND).and_then(|l| l.first()).cloned() {
                    let vol = crate::uvm::settings::applied().map(|a| a.sfx_volume).unwrap_or(1.0) * 0.6;
                    let s = commands.spawn((AudioPlayer::new(h), PlaybackSettings::LOOP.with_volume(Volume::Linear(vol)), Name::new("burn sound"))).id();
                    sound = Some(s);
                }
            }
        }
        commands.entity(e).insert(Burning { left, fx, sound });
        // GGNpc.OnSetOnFire: panic while burning
        if let Ok(mut n) = npcs.get_mut(e) {
            if let Some(sd) = n.script.as_mut() {
                sd.panic = true;
            } else if n.state != NpcState::Ragdoll {
                n.state = NpcState::Panic;
                n.timer = left;
            }
        }
    }
}

fn burn(
    time: Res<Time>,
    mut commands: Commands,
    mut burning: Query<(Entity, &mut Burning, &GlobalTransform)>,
    pawns: Query<(Entity, &GlobalTransform), (Or<(With<Goat>, With<Npc>)>, Without<Burning>)>,
) {
    let _prof_scope = crate::prof::scope("fire::burn");
    let dt = time.delta_secs();
    let mut spread = vec![];
    for (e, mut b, t) in &mut burning {
        b.left -= dt;
        if b.left <= 0.0 {
            if let Some(f) = b.fx.take() {
                commands.entity(f).despawn();
            }
            if let Some(s) = b.sound.take() {
                commands.entity(s).despawn();
            }
            commands.entity(e).remove::<Burning>();
            continue;
        }
        // GGPawn.Collided: touching a burning pawn sets you on fire
        let p = t.translation();
        for (o, ot) in &pawns {
            if ot.translation().distance(p) < SPREAD_DISTANCE {
                spread.push(o);
            }
        }
    }
    for o in spread {
        commands.entity(o).insert(Ignite);
    }
}

pub struct FirePlugin;

impl Plugin for FirePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FireStats>().add_systems(Update, (ignite, burn).chain());
    }
}
