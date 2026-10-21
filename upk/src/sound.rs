//! USoundNodeWave (PC cooked: Ogg Vorbis in a bulk data block) and USoundCue node graphs.

use crate::material::Objects;
use crate::props::{self, Value};
use crate::reader::Reader;
use crate::registry::ObjRef;
use crate::texture::BulkData;

#[derive(Debug, Clone)]
pub struct Wave {
    pub name: String,
    pub duration: f32,
    pub volume: f32,
    /// Complete Ogg Vorbis file.
    pub ogg: Vec<u8>,
}

/// Read the Ogg stream of a SoundNodeWave (RawData, CompressedPCData, console blocks...).
pub fn wave(objs: &dyn Objects, r: ObjRef) -> Option<Wave> {
    let p = objs.package(r.pkg);
    let data = p.export_data(r.export);
    let (end, ps) = props::export_props(p, r.export).ok()?;
    let mut rd = Reader::at(data, end);
    for _ in 0..8 {
        let Ok(b) = BulkData::read(&mut rd) else { break };
        if b.is_empty() {
            continue;
        }
        if let Ok(bytes) = b.load(data, None) {
            if bytes.starts_with(b"OggS") {
                let f = |n: &str, d: f32| match props::find(&ps, n) {
                    Some(Value::Float(v)) => *v,
                    _ => d,
                };
                return Some(Wave { name: objs.name_of(r), duration: f("Duration", 0.0), volume: f("Volume", 1.0), ogg: bytes });
            }
        }
    }
    None
}

/// All SoundNodeWaves reachable from a SoundCue (random/mixer/attenuation nodes...).
pub fn cue_waves(objs: &dyn Objects, cue: ObjRef) -> Vec<ObjRef> {
    let mut out = Vec::new();
    let mut stack = Vec::new();
    if let Some(ps) = objs.props_of(cue) {
        if let Some(Value::Object(o)) = props::find(&ps, "FirstNode") {
            if let Some(r) = objs.resolve_obj(cue.pkg, *o) {
                stack.push(r);
            }
        }
    }
    let mut guard = 0;
    while let Some(n) = stack.pop() {
        guard += 1;
        if guard > 256 {
            break;
        }
        if objs.class_of(n) == "SoundNodeWave" {
            if !out.contains(&n) {
                out.push(n);
            }
            continue;
        }
        let Some(ps) = objs.props_of(n) else { continue };
        for o in crate::material::object_array(props::find(&ps, "ChildNodes")) {
            if let Some(r) = objs.resolve_obj(n.pkg, o) {
                stack.push(r);
            }
        }
        // SoundNodeAmbient(NonLoop): SoundSlots { Wave, PitchScale, VolumeScale, Weight }
        for slot in crate::material::struct_array(objs.package(n.pkg), props::find(&ps, "SoundSlots")) {
            if let Some(Value::Object(o)) = props::find(&slot, "Wave") {
                if let Some(r) = objs.resolve_obj(n.pkg, *o) {
                    stack.push(r);
                }
            }
        }
        if let Some(Value::Object(o)) = props::find(&ps, "Wave") {
            if let Some(r) = objs.resolve_obj(n.pkg, *o) {
                stack.push(r);
            }
        }
    }
    out
}

/// Distance attenuation of a cue: the first node with RadiusMin/RadiusMax (SoundNodeAmbient,
/// SoundNodeAttenuation), in Unreal units, with its VolumeMax (ambient nodes, default 0.7).
pub fn cue_attenuation(objs: &dyn Objects, cue: ObjRef) -> Option<(f32, f32, f32)> {
    let mut stack = Vec::new();
    if let Some(ps) = objs.props_of(cue) {
        if let Some(Value::Object(o)) = props::find(&ps, "FirstNode") {
            stack.extend(objs.resolve_obj(cue.pkg, *o));
        }
    }
    let mut guard = 0;
    while let Some(n) = stack.pop() {
        guard += 1;
        if guard > 64 {
            break;
        }
        let class = objs.class_of(n);
        let Some(ps) = objs.props_of(n) else { continue };
        if class.starts_with("SoundNodeAmbient") || class == "SoundNodeAttenuation" {
            let f = |k: &str, d: f32| match props::find(&ps, k) {
                Some(Value::Float(v)) => *v,
                _ => d,
            };
            let vol = if class.starts_with("SoundNodeAmbient") { f("VolumeMax", 0.7) } else { 1.0 };
            return Some((f("RadiusMin", if class == "SoundNodeAttenuation" { 400.0 } else { 2000.0 }), f("RadiusMax", 5000.0), vol));
        }
        for o in crate::material::object_array(props::find(&ps, "ChildNodes")) {
            stack.extend(objs.resolve_obj(n.pkg, o));
        }
    }
    None
}
