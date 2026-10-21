//! UAnimSet / UAnimSequence: uses the raw (uncompressed) key tracks kept in cooked packages.

use crate::material::object_array;
use crate::package::{FName, Package};
use crate::props::{self, Value};
use crate::reader::Reader;
use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub struct Track {
    /// One key (constant) or one per frame.
    pub pos: Vec<[f32; 3]>,
    /// Raw UE3 quaternions (X, Y, Z, W) as stored (same convention as the ref skeleton).
    pub rot: Vec<[f32; 4]>,
}

#[derive(Debug, Clone)]
pub struct Sequence {
    pub name: String,
    pub num_frames: u32,
    pub length: f32,
    pub rate_scale: f32,
    pub tracks: Vec<Track>,
    pub looping: bool,
    /// Times (seconds) of AnimNotify_Footstep notifies.
    pub footsteps: Vec<f32>,
    /// AnimNotify_Sound notifies: (time, SoundCue path).
    pub sounds: Vec<(f32, String)>,
}

#[derive(Debug, Clone)]
pub struct AnimSet {
    pub name: String,
    pub track_bones: Vec<String>,
    pub sequences: Vec<Sequence>,
}

fn bulk_vec<'a>(r: &mut Reader<'a>, elem_expected: usize) -> Result<(usize, &'a [u8])> {
    let elem = r.i32()? as usize;
    let count = r.i32()?;
    if count < 0 || (count > 0 && elem != elem_expected) || elem * count as usize > r.remaining() {
        bail!("bad raw track bulk elem {elem} count {count}");
    }
    Ok((count as usize, r.bytes(elem * count as usize)?))
}

impl Sequence {
    pub fn parse(p: &Package, export: usize) -> Result<Self> {
        let data = p.export_data(export);
        let (end, ps) = props::export_props(p, export)?;
        let name = match props::find(&ps, "SequenceName") {
            Some(Value::Name(n)) => n.clone(),
            _ => p.obj_name(export as i32 + 1),
        };
        let num_frames = match props::find(&ps, "NumFrames") {
            Some(Value::Int(n)) => *n as u32,
            _ => 1,
        };
        let length = match props::find(&ps, "SequenceLength") {
            Some(Value::Float(f)) => *f,
            _ => 1.0,
        };
        let rate_scale = match props::find(&ps, "RateScale") {
            Some(Value::Float(f)) => *f,
            _ => 1.0,
        };
        let looping = !matches!(props::find(&ps, "bNoLoopingInterpolation"), Some(Value::Bool(true)));
        // Notifies: AnimNotifyEvent { Time, Notify, Comment, Duration }
        let mut footsteps = Vec::new();
        let mut sounds = Vec::new();
        for ev in crate::material::struct_array(p, props::find(&ps, "Notifies")) {
            let (Some(Value::Float(t)), Some(Value::Object(o))) = (props::find(&ev, "Time"), props::find(&ev, "Notify")) else { continue };
            if *o == 0 {
                continue;
            }
            match p.class_name(*o).as_str() {
                "AnimNotify_Footstep" => footsteps.push(*t),
                "AnimNotify_Sound" if *o > 0 => {
                    if let Ok((_, np)) = props::export_props(p, *o as usize - 1) {
                        if let Some(Value::Object(c)) = props::find(&np, "SoundCue") {
                            if *c != 0 {
                                sounds.push((*t, p.obj_path(*c)));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let mut r = Reader::at(data, end);
        let ntracks = r.i32()?;
        if !(0..=512).contains(&ntracks) {
            bail!("bad raw track count {ntracks}");
        }
        let mut tracks = Vec::with_capacity(ntracks as usize);
        for _ in 0..ntracks {
            let (np, pd) = bulk_vec(&mut r, 12)?;
            let mut pr = Reader::new(pd);
            let pos = (0..np).map(|_| pr.vec3()).collect::<Result<Vec<_>>>()?;
            let (nr, rd) = bulk_vec(&mut r, 16)?;
            let mut rr = Reader::new(rd);
            let rot = (0..nr).map(|_| Ok([rr.f32()?, rr.f32()?, rr.f32()?, rr.f32()?])).collect::<Result<Vec<_>>>()?;
            tracks.push(Track { pos, rot });
        }
        Ok(Sequence { name, num_frames, length, rate_scale, tracks, looping, footsteps, sounds })
    }
}

impl AnimSet {
    pub fn parse(p: &Package, export: usize) -> Result<Self> {
        let (_, ps) = props::export_props(p, export)?;
        let track_bones = match props::find(&ps, "TrackBoneNames") {
            Some(Value::Array(_, d)) => d
                .chunks_exact(8)
                .map(|c| {
                    p.name_str(FName {
                        index: i32::from_le_bytes([c[0], c[1], c[2], c[3]]),
                        number: i32::from_le_bytes([c[4], c[5], c[6], c[7]]),
                    })
                })
                .collect(),
            _ => vec![],
        };
        let mut sequences = Vec::new();
        for s in object_array(props::find(&ps, "Sequences")) {
            if s > 0 {
                if let Ok(seq) = Sequence::parse(p, s as usize - 1) {
                    sequences.push(seq);
                }
            }
        }
        Ok(AnimSet { name: p.obj_name(export as i32 + 1), track_bones, sequences })
    }
}
