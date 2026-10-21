//! Placed level sounds and music.
//!
//! Ambient sounds: AmbientSoundSimple (SoundNodeAmbient: SoundSlots, RadiusMin/RadiusMax,
//! VolumeMax), AmbientSound / AmbientSoundMovable (+ GGAmbientMusicMovable radios) with their
//! AudioComponent's cue (radii from its SoundNodeAttenuation, else the DrawSoundRadius sphere).
//! Each loops while the listener is inside RadiusMax, with UE3's linear falloff between
//! RadiusMin and RadiusMax, scaled by the SFX volume (music radios by the music volume).
//!
//! Music: GGMusicManager runs in the VM (track queue, priorities, mutes; Kismet
//! GGSeqAct_ChangeMusicTrack, GGTriggerVolumeMusic, Music Party's PlayCustomTrack). Its top track
//! (mTracks[0].Cue, silent while mIsMuted) is played here as a looping stream with a cross-fade.

use crate::convert::{self, S};
use bevy::audio::{AudioSink, AudioSinkPlayback, Volume};
use bevy::prelude::*;
use upk::registry::{ObjRef, Registry};

struct Emitter {
    pos: Vec3,
    /// metres
    min: f32,
    max: f32,
    volume: f32,
    music: bool,
    sounds: Vec<Handle<AudioSource>>,
    playing: Option<Entity>,
}

#[derive(Resource, Default)]
pub struct Ambience {
    loaded: bool,
    emitters: Vec<Emitter>,
    /// Music: the cue path playing, its entity, volume ramp (0..1).
    music_cue: Option<String>,
    music: Option<(Entity, f32)>,
    fading: Vec<(Entity, f32)>,
    /// Emitters within range now (tests).
    pub audible: usize,
    pub total: usize,
}

impl Ambience {
    /// The music cue playing (tests).
    pub fn music(&self) -> Option<&str> {
        self.music_cue.as_deref()
    }
}

fn load(reg: &Registry, audio: &mut Assets<AudioSource>) -> Vec<Emitter> {
    let map = &reg.packages[0];
    let mut out = vec![];
    for i in 0..map.exports.len() {
        let class = map.class_name(i as i32 + 1);
        if !(class.starts_with("AmbientSound") || class == "GGAmbientMusicMovable") || !map.obj_path(i as i32 + 1).contains(".PersistentLevel.") {
            continue;
        }
        let actor = ObjRef { pkg: 0, export: i };
        let ps = reg.merged_props(actor);
        let obj = |ps: &[(usize, upk::props::Prop)], k: &str| match upk::registry::find(ps, k) {
            Some((pk, upk::props::Value::Object(o))) => reg.resolve(pk, *o),
            _ => None,
        };
        if matches!(upk::registry::find(&ps, "bAutoPlay"), Some((_, upk::props::Value::Bool(false)))) {
            continue;
        }
        let pos = match upk::registry::find(&ps, "Location") {
            Some((_, upk::props::Value::Vector(v))) => convert::pos(*v),
            _ => continue,
        };
        // the AudioComponent's cue (AmbientSoundSimple's points at its own SoundNodeAmbient)
        let comp = obj(&ps, "AudioComponent");
        let cps = comp.map(|c| reg.merged_props(c)).unwrap_or_default();
        let Some(cue) = obj(&cps, "SoundCue").or_else(|| obj(&ps, "SoundCueInstance")) else { continue };
        let sphere = obj(&cps, "PreviewSoundRadius")
            .and_then(|d| match upk::registry::find(&reg.merged_props(d), "SphereRadius") {
                Some((_, upk::props::Value::Float(f))) => Some(*f),
                _ => None,
            })
            .unwrap_or(3000.0);
        let (min, max, vol) = upk::sound::cue_attenuation(reg, cue).unwrap_or((sphere * 0.15, sphere, 1.0));
        let cue_vol = match upk::registry::find(&reg.merged_props(cue), "VolumeMultiplier") {
            Some((_, upk::props::Value::Float(f))) => *f,
            _ => 0.75,
        };
        let sounds: Vec<Handle<AudioSource>> = upk::sound::cue_waves(reg, cue)
            .into_iter()
            .filter_map(|w| upk::sound::wave(reg, w))
            .map(|w| audio.add(AudioSource { bytes: w.ogg.into() }))
            .collect();
        if sounds.is_empty() {
            continue;
        }
        out.push(Emitter { pos, min: min * S, max: max.max(min + 1.0) * S, volume: vol * cue_vol, music: class.contains("Music"), sounds, playing: None });
    }
    out
}

fn music_track(m: &mut crate::uvm::plugin::Mutators) -> Option<String> {
    let h = m.host.as_mut()?;
    let game = h.game?;
    let mm = h.vm.get(game, "mMusicManager").obj()?;
    if h.vm.get(mm, "mIsMuted").truthy() {
        return None;
    }
    let tracks = match h.vm.get(mm, "mTracks") {
        crate::uvm::value::Value::Array(a) => a,
        _ => return None,
    };
    let first = tracks.first()?;
    let crate::uvm::value::Value::Struct(s) = first else { return None };
    let k = h.vm.sym("Cue");
    let cue = s.field(k)?.clone();
    crate::uvm::actors::asset_path_of(&h.vm, &cue)
}

#[allow(clippy::too_many_arguments)]
pub fn ambience(
    time: Res<Time>,
    mut amb: ResMut<Ambience>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    sfx: Option<Res<crate::audio::Sfx>>,
    mut audio: ResMut<Assets<AudioSource>>,
    mut m: ResMut<crate::uvm::plugin::Mutators>,
    mut commands: Commands,
    listener: Query<&GlobalTransform, With<Camera3d>>,
    mut sinks: Query<&mut AudioSink>,
) {
    let _prof_scope = crate::prof::scope("ambient::ambience");
    let Some(game) = game else { return };
    let muted = sfx.as_ref().is_none_or(|s| s.muted);
    if !amb.loaded {
        amb.loaded = true;
        if std::env::var_os("GOAT_NO_AMBIENT").is_none() {
            let t = std::time::Instant::now();
            amb.emitters = load(&game.0, &mut audio);
            amb.total = amb.emitters.len();
            info!("ambient sounds: {} in {:?}", amb.emitters.len(), t.elapsed());
        }
    }
    let ear = listener.single().map(|t| t.translation()).unwrap_or(Vec3::ZERO);
    let vols = crate::uvm::settings::applied();
    let sfx_vol = vols.map(|a| a.sfx_volume).unwrap_or(1.0);
    let music_vol = vols.map(|a| a.music_volume).unwrap_or(1.0);
    let mut audible = 0;
    for e in &mut amb.emitters {
        let d = e.pos.distance(ear);
        let gain = if d <= e.min { 1.0 } else { (1.0 - (d - e.min) / (e.max - e.min)).clamp(0.0, 1.0) };
        let v = gain * e.volume * if e.music { music_vol } else { sfx_vol };
        if gain > 0.0 {
            audible += 1;
        }
        if v > 0.003 && !muted {
            match e.playing {
                Some(ent) => {
                    if let Ok(mut s) = sinks.get_mut(ent) {
                        s.set_volume(Volume::Linear(v));
                    }
                }
                None => {
                    let pick = (e.pos.x.abs() as usize) % e.sounds.len();
                    e.playing = Some(commands.spawn((AudioPlayer::new(e.sounds[pick].clone()), PlaybackSettings::LOOP.with_volume(Volume::Linear(v)), Name::new("ambient"))).id());
                }
            }
        } else if let Some(ent) = e.playing.take() {
            commands.entity(ent).despawn();
        }
    }
    amb.audible = audible;

    // music: the VM music manager's top track
    // (followed in silent test runs too; only playback is skipped)
    let want = music_track(&mut m);
    if want != amb.music_cue {
        if let Some((ent, v)) = amb.music.take() {
            amb.fading.push((ent, v));
        }
        amb.music_cue = want.clone();
        if let Some(p) = &want {
            info!("music track: {p}");
        }
        if let Some(path) = want.filter(|_| !muted) {
            // the cue may only be in the VM's packages (mutators, DLC)
            let load = |reg: &Registry| -> Vec<Vec<u8>> {
                // a SoundCue, or a bare SoundNodeWave (Goat City Bay's BGM_temp)
                let waves = match reg.find_path(&path) {
                    Some(c) if reg.class_name(c) == "SoundNodeWave" => vec![c],
                    Some(c) => upk::sound::cue_waves(reg, c),
                    None => vec![],
                };
                waves.into_iter().filter_map(|w| upk::sound::wave(reg, w)).map(|w| w.ogg).collect()
            };
            let mut oggs = load(&game.0);
            if oggs.is_empty() {
                if let Some(h) = m.host.as_ref() {
                    oggs = load(&h.vm.reg);
                }
            }
            let handles: Vec<Handle<AudioSource>> = oggs.into_iter().map(|b| audio.add(AudioSource { bytes: b.into() })).collect();
            if let Some(h) = handles.first() {
                info!("music: {path}");
                let ent = commands.spawn((AudioPlayer::new(h.clone()), PlaybackSettings::LOOP.with_volume(Volume::Linear(0.0)), Name::new("music"))).id();
                amb.music = Some((ent, 0.0));
            }
        }
    }
    // GGMusicManager cross-fade: mCurrentFadeFactor 0.8 per second
    let dt = time.delta_secs();
    if let Some((ent, v)) = amb.music.as_mut() {
        *v = (*v + 0.8 * dt).min(1.0);
        if let Ok(mut s) = sinks.get_mut(*ent) {
            s.set_volume(Volume::Linear(*v * music_vol));
        }
    }
    let mut keep = vec![];
    for (ent, mut v) in std::mem::take(&mut amb.fading) {
        v -= 0.8 * dt;
        if v <= 0.0 {
            commands.entity(ent).despawn();
        } else {
            if let Ok(mut s) = sinks.get_mut(ent) {
                s.set_volume(Volume::Linear(v * music_vol));
            }
            keep.push((ent, v));
        }
    }
    amb.fading = keep;
}

pub struct AmbientPlugin;

impl Plugin for AmbientPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Ambience>().add_systems(Update, ambience);
    }
}
