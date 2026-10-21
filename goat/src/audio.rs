//! Sound effects straight from the game's SoundCues (Ogg Vorbis inside SoundNodeWaves).
//! Test runs are silent.

use bevy::audio::{AudioSource, Volume};
use bevy::prelude::*;
use std::collections::HashMap;
use upk::registry::Registry;

/// Cue name -> the waves its random node picks from.
#[derive(Resource, Default)]
pub struct Sfx {
    pub cues: HashMap<String, Vec<Handle<AudioSource>>>,
    pub muted: bool,
}

/// Sounds requested this frame: (cue key, world position for distance attenuation).
#[derive(Resource, Default)]
pub struct SoundQueue(pub Vec<(String, Option<Vec3>)>);

impl SoundQueue {
    pub fn play(&mut self, cue: &str, at: Option<Vec3>) {
        self.0.push((cue.to_string(), at));
    }
}

/// How often each cue was requested (also in silent test runs).
#[derive(Resource, Default)]
pub struct SoundLog(pub HashMap<String, u32>);

impl SoundLog {
    pub fn count(&self, prefix: &str) -> u32 {
        self.0.iter().filter(|(k, _)| k.starts_with(prefix)).map(|(_, n)| n).sum()
    }
}

/// GGPawnSoundGroup footstep tables: `<prefix><MaterialType>` per FootstepInfo entry and `<prefix>`
/// for the default cue.
pub fn load_footsteps(reg: &Registry, sfx: &mut Sfx, audio: &mut Assets<AudioSource>, prefix: &str, list: &str, default_cue: &str) {
    let Some(obj) = reg.find_path("Default__GGPawnSoundGroup") else { return };
    let props = reg.merged_props(obj);
    if let Some((pk, upk::props::Value::Object(o))) = upk::registry::find(&props, default_cue) {
        if let Some(r) = reg.resolve(pk, *o) {
            load_cue(reg, sfx, audio, prefix, &reg.path(r));
        }
    }
    if let Some((pk, v)) = upk::registry::find(&props, list) {
        for e in upk::material::struct_array(&reg.packages[pk], Some(v)) {
            let (Some(upk::props::Value::Name(kind)), Some(upk::props::Value::Object(o))) = (upk::props::find(&e, "MaterialType"), upk::props::find(&e, "Sound")) else {
                continue;
            };
            if let Some(r) = reg.resolve(pk, *o) {
                load_cue(reg, sfx, audio, &format!("{prefix}{kind}"), &reg.path(r));
            }
        }
    }
}

/// Load a cue by full path under a short key.
pub fn load_cue(reg: &Registry, sfx: &mut Sfx, audio: &mut Assets<AudioSource>, key: &str, path: &str) -> usize {
    let Some(cue) = reg.find_path(path) else {
        warn!("sound cue {path} not found");
        return 0;
    };
    let waves = if reg.class_name(cue) == "SoundNodeWave" { vec![cue] } else { upk::sound::cue_waves(reg, cue) };
    let handles: Vec<Handle<AudioSource>> = waves
        .into_iter()
        .filter_map(|w| upk::sound::wave(reg, w))
        .map(|w| audio.add(AudioSource { bytes: w.ogg.into() }))
        .collect();
    let n = handles.len();
    sfx.cues.entry(key.to_string()).or_default().extend(handles);
    n
}

/// Load the cues of an object array property on a class default (e.g. GGNpc mKnockedOverSounds).
pub fn load_cue_list(reg: &Registry, sfx: &mut Sfx, audio: &mut Assets<AudioSource>, key: &str, default_obj: &str, prop: &str) {
    let Some(obj) = reg.find_path(default_obj) else { return };
    let props = reg.merged_props(obj);
    let Some((pk, v)) = upk::registry::find(&props, prop) else { return };
    for o in upk::material::object_array(Some(v)) {
        if let Some(r) = reg.resolve(pk, o) {
            let path = reg.path(r);
            load_cue(reg, sfx, audio, key, &path);
        }
    }
}

pub fn play_queued(
    mut commands: Commands,
    mut queue: ResMut<SoundQueue>,
    mut log: ResMut<SoundLog>,
    sfx: Res<Sfx>,
    listener: Query<&GlobalTransform, With<Camera3d>>,
    time: Res<Time<Real>>,
) {
    let _prof_scope = crate::prof::scope("audio::play_queued");
    if queue.0.is_empty() {
        return;
    }
    let ear = listener.single().map(|t| t.translation()).unwrap_or(Vec3::ZERO);
    for (key, at) in queue.0.drain(..) {
        *log.0.entry(key.clone()).or_default() += 1;
        if std::env::var_os("GOAT_SOUNDS").is_some() {
            println!("    sound {key}");
        }
        if sfx.muted {
            continue;
        }
        let Some(list) = sfx.cues.get(&key).filter(|l| !l.is_empty()) else { continue };
        // cheap deterministic "random" pick, like SoundNodeRandom
        let i = (time.elapsed().as_nanos() as usize / 7919) % list.len();
        let vol = match at {
            Some(p) => (1.0 - p.distance(ear) / 60.0).clamp(0.0, 1.0),
            None => 1.0,
        };
        // Options volumes: NPC voices (npc_*) use the voice slider, everything else SFX
        let vol = vol * crate::uvm::settings::applied().map(|a| if key.starts_with("npc_") { a.voice_volume } else { a.sfx_volume }).unwrap_or(1.0);
        if vol <= 0.01 {
            continue;
        }
        let pitch = 0.95 + ((time.elapsed().as_nanos() / 13) % 100) as f32 / 1000.0;
        commands.spawn((AudioPlayer::new(list[i].clone()), PlaybackSettings::DESPAWN.with_volume(Volume::Linear(vol)).with_speed(pitch)));
    }
}
