//! UE3 particle systems (ParticleSystem -> sprite emitters -> LOD 0 modules), read into plain
//! data a renderer can simulate. Cooked packages store only values that differ from the
//! archetype (the engine's Default__ParticleModuleX in Engine.u), so every lookup walks the
//! archetype chain; load Engine.u into the registry for the engine defaults.
//!
//! Module values are RawDistributions: the cooker's baked LookupTable (what the game's runtime
//! samples) is used when present, else the Distribution object (Constant / Uniform / ConstantCurve
//! / ParticleParameter's Constant).

use crate::material::{self, Objects};
use crate::props::{Prop, Value};
use crate::registry::{ObjRef, Registry};

/// A RawDistributionFloat / RawDistributionVector.
#[derive(Debug, Clone, Default)]
pub struct Dist {
    /// 1 = none, 2 = random (lerp between min and max), 3 = extreme (min or max).
    pub op: u8,
    pub elems: u8,
    pub chunk: u8,
    pub table: Vec<f32>,
    pub scale: f32,
    pub start: f32,
    pub fallback: Fallback,
}

#[derive(Debug, Clone, Default)]
pub enum Fallback {
    #[default]
    Zero,
    Const([f32; 3]),
    Uniform([f32; 3], [f32; 3]),
    Curve(Vec<(f32, [f32; 3])>),
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: [f32; 3]) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t[0], a[1] + (b[1] - a[1]) * t[1], a[2] + (b[2] - a[2]) * t[2]]
}

impl Dist {
    pub fn constant(v: [f32; 3]) -> Self {
        Dist { fallback: Fallback::Const(v), ..Default::default() }
    }

    fn baked(&self) -> bool {
        self.chunk > 0 && self.table.len() >= 2 + self.chunk as usize
    }

    /// Value at time `t` (seconds of emitter time, or 0..1 of particle life); `rnd` are three
    /// uniform 0..1 numbers for random distributions.
    pub fn eval(&self, t: f32, rnd: [f32; 3]) -> [f32; 3] {
        if self.baked() {
            let chunk = self.chunk as usize;
            let n = (self.table.len() - 2) / chunk;
            let idx = ((t - self.start) * self.scale).max(0.0);
            let i1 = (idx as usize).min(n - 1);
            let i2 = (i1 + 1).min(n - 1);
            let a = (idx - idx.floor()).clamp(0.0, 1.0);
            let a = if idx as usize >= n - 1 { 0.0 } else { a };
            let w = (chunk / self.elems.max(1) as usize).clamp(1, 3);
            let at = |i: usize, k: usize| -> [f32; 3] {
                let o = 2 + i * chunk + k * w;
                let mut v = [0.0; 3];
                for j in 0..w {
                    v[j] = self.table.get(o + j).copied().unwrap_or(0.0);
                }
                v
            };
            let s = |k: usize| lerp3(at(i1, k), at(i2, k), [a; 3]);
            return match (self.op, self.elems) {
                (2, 2..) => lerp3(s(0), s(1), rnd),
                (3, 2..) => {
                    if rnd[0] > 0.5 {
                        s(1)
                    } else {
                        s(0)
                    }
                }
                _ => s(0),
            };
        }
        match &self.fallback {
            Fallback::Zero => [0.0; 3],
            Fallback::Const(v) => *v,
            Fallback::Uniform(a, b) => lerp3(*a, *b, rnd),
            Fallback::Curve(pts) => {
                if pts.is_empty() {
                    return [0.0; 3];
                }
                if t <= pts[0].0 {
                    return pts[0].1;
                }
                for w in pts.windows(2) {
                    if t <= w[1].0 {
                        let span = (w[1].0 - w[0].0).max(1e-6);
                        let a = (t - w[0].0) / span;
                        return lerp3(w[0].1, w[1].1, [a; 3]);
                    }
                }
                pts[pts.len() - 1].1
            }
        }
    }

    pub fn eval1(&self, t: f32, rnd: f32) -> f32 {
        self.eval(t, [rnd; 3])[0]
    }

    /// Whether the value never changes over time (constant table or constant object).
    pub fn is_set(&self) -> bool {
        self.baked() || !matches!(self.fallback, Fallback::Zero)
    }
}

/// Blend mode of the base Material.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Blend {
    Opaque,
    Masked,
    #[default]
    Translucent,
    Additive,
    Modulate,
}

#[derive(Debug, Clone)]
pub enum Module {
    Lifetime(Dist),
    Size(Dist),
    SizeMultiplyLife { d: Dist, axes: [bool; 3] },
    SizeScale { d: Dist, axes: [bool; 3] },
    Velocity { d: Dist, radial: Dist, world: bool },
    VelocityOverLife { d: Dist, absolute: bool, world: bool },
    Acceleration { d: Dist, world: bool },
    AccelerationOverLife { d: Dist, world: bool },
    Drag(Dist),
    Color { c: Dist, a: Dist },
    ColorOverLife { c: Dist, a: Dist },
    ColorScaleOverLife { c: Dist, a: Dist, emitter_time: bool },
    Rotation(Dist),
    RotationRate(Dist),
    Location(Dist),
    Sphere { radius: Dist, offset: Dist, axes: [bool; 6], surface: bool, velocity: bool, vel_scale: Dist },
    Cylinder { radius: Dist, height: Dist, offset: Dist, axes: [bool; 6], surface: bool, velocity: bool, vel_scale: Dist, axis: u8 },
    SubUV(Dist),
    /// Offset from the particle centre (uu), rotated by Rotation (turns per axis) turning at
    /// RotationRate (turns/s); chained orbits add up.
    Orbit { offset: Dist, rotation: Dist, rate: Dist },
    /// Mesh particles' start rotation and rotation rate (turns per axis).
    MeshRotation(Dist),
    MeshRotationRate(Dist),
    /// Bounce off the world: velocity * DampingFactor per hit; after MaxCollisions the
    /// CollisionCompletionOption (0 kill, 1 freeze, 2 stop colliding, 3-5 freeze movement).
    Collision { damping: Dist, max: Dist, completion: u8 },
    /// SpawnPerUnit: particles per UnitScalar uu the emitter moves (trails).
    SpawnPerUnit { unit: f32, per: Dist, ignore_rate: bool },
    /// Read but not simulated.
    Other(String),
}

#[derive(Debug, Clone)]
pub struct Emitter {
    pub name: String,
    pub material: Option<ObjRef>,
    /// Colour texture of the material (Texture2D, behind EmissiveColor/DiffuseColor).
    pub texture: Option<ObjRef>,
    /// Texture behind Opacity, when the material has one (its alpha or brightness = opacity).
    pub opacity: Option<ObjRef>,
    pub blend: Blend,
    pub duration: f32,
    /// 0 = loop forever.
    pub loops: i32,
    pub delay: f32,
    pub local_space: bool,
    /// 0 square (camera facing), 1 rectangle, 2 velocity aligned.
    pub align: u8,
    pub sub_h: u32,
    pub sub_v: u32,
    /// PSUVIM_None 0, Linear 1, Linear_Blend 2, Random 3, Random_Blend 4.
    pub sub_interp: u8,
    pub rate: Dist,
    pub rate_scale: Dist,
    /// (count, count_low or -1, time 0..1 of the duration)
    pub bursts: Vec<(i32, i32, f32)>,
    pub max_particles: u32,
    pub modules: Vec<Module>,
    /// Non-sprite emitters (beam, ribbon...) aren't drawn; this names the type.
    pub unsupported: Option<String>,
    /// TypeDataMesh: each particle is this static mesh, with MeshMaterial's material if set.
    pub mesh: Option<ObjRef>,
    pub mesh_material: Option<ObjRef>,
    /// OrientationAxisLock.LockAxisFlags: 0 none, 1-3 face +X/+Y/+Z, 4-6 face -X/-Y/-Z,
    /// 7-9 turn about X/Y/Z only.
    pub axis_lock: u8,
    /// TypeDataRibbon: the particles of a trail drawn as one strip.
    pub ribbon: Option<Ribbon>,
    /// TypeDataBeam2: each particle a beam from its source to its target.
    pub beam: Option<Beam>,
}

#[derive(Debug, Clone)]
pub struct Ribbon {
    pub max_in_trail: u32,
    /// TilingDistance (uu per texture repeat along the trail); 0 = the texture once.
    pub tiling: f32,
}

#[derive(Debug, Clone)]
pub struct Beam {
    /// BeamMethod: 0 distance (along the emitter's X), 1 target, 2 branch.
    pub method: u8,
    pub distance: Dist,
    pub tex_tile: f32,
    pub sheets: u32,
    /// BeamSource / BeamTarget values (emitter space, uu) when the emitter has those modules.
    pub source: Option<Dist>,
    pub target: Option<Dist>,
    /// BeamNoise: points along the beam, their offset range (uu), how long a pattern holds.
    pub noise: Option<(u32, Dist, f32)>,
}

#[derive(Debug, Clone)]
pub struct System {
    pub path: String,
    pub emitters: Vec<Emitter>,
    pub warmup: f32,
}

/// The archetype: explicit, else implied (an object's class default object; a default object's
/// super class default object).
fn arch(reg: &Registry, r: ObjRef) -> Option<ObjRef> {
    let p = &reg.packages[r.pkg];
    let e = &p.exports[r.export];
    if e.archetype != 0 {
        return reg.resolve(r.pkg, e.archetype);
    }
    let class = reg.resolve(r.pkg, e.class)?;
    let cls = if p.obj_name(r.export as i32 + 1).starts_with("Default__") {
        let ce = &reg.packages[class.pkg].exports[class.export];
        reg.resolve(class.pkg, ce.super_)?
    } else {
        class
    };
    let cname = reg.packages[cls.pkg].obj_name(cls.export as i32 + 1);
    if cname == "Object" {
        return None;
    }
    reg.find_path(&format!("Default__{cname}")).filter(|&a| a != r)
}

/// A property along the archetype chain (instance first), with its package.
fn get(reg: &Registry, r: ObjRef, name: &str) -> Option<(usize, Value)> {
    let mut cur = Some(r);
    for _ in 0..10 {
        let c = cur?;
        if let Some(p) = reg.props(c).into_iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
            return Some((c.pkg, p.value));
        }
        cur = arch(reg, c);
    }
    None
}

/// A struct property merged field by field along the archetype chain (cooked structs only hold
/// the fields that differ from the archetype's).
fn get_struct(reg: &Registry, r: ObjRef, name: &str) -> Vec<(usize, Prop)> {
    let mut out: Vec<(usize, Prop)> = vec![];
    let mut cur = Some(r);
    for _ in 0..10 {
        let Some(c) = cur else { break };
        if let Some(p) = reg.props(c).into_iter().find(|p| p.name.eq_ignore_ascii_case(name)) {
            if let Value::Struct(_, fields) = p.value {
                for f in fields {
                    if !out.iter().any(|(_, o)| o.name.eq_ignore_ascii_case(&f.name)) {
                        out.push((c.pkg, f));
                    }
                }
            }
        }
        cur = arch(reg, c);
    }
    out
}

fn f32_of(v: Option<&Value>) -> Option<f32> {
    match v {
        Some(Value::Float(f)) => Some(*f),
        Some(Value::Int(i)) => Some(*i as f32),
        _ => None,
    }
}

fn vec_of(v: &Value) -> Option<[f32; 3]> {
    match v {
        Value::Vector(v) => Some(*v),
        Value::Float(f) => Some([*f; 3]),
        Value::Vector2D(v) => Some([v[0], v[1], 0.0]),
        _ => None,
    }
}

fn bool_of(reg: &Registry, r: ObjRef, name: &str, default: bool) -> bool {
    match get(reg, r, name) {
        Some((_, Value::Bool(b))) => b,
        _ => default,
    }
}

fn float_of(reg: &Registry, r: ObjRef, name: &str, default: f32) -> f32 {
    get(reg, r, name).and_then(|(_, v)| f32_of(Some(&v))).unwrap_or(default)
}

fn obj_of(reg: &Registry, r: ObjRef, name: &str) -> Option<ObjRef> {
    match get(reg, r, name) {
        Some((pk, Value::Object(o))) if o != 0 => reg.resolve(pk, o),
        _ => None,
    }
}

fn objs_of(reg: &Registry, r: ObjRef, name: &str) -> Vec<ObjRef> {
    match get(reg, r, name) {
        Some((pk, v)) => material::object_array(Some(&v)).into_iter().filter_map(|o| reg.resolve(pk, o)).collect(),
        None => vec![],
    }
}

fn enum_of(reg: &Registry, r: ObjRef, name: &str) -> String {
    match get(reg, r, name) {
        Some((_, Value::Enum(e))) => e,
        Some((_, Value::Byte(b))) => b.to_string(),
        _ => String::new(),
    }
}

/// A distribution object's value without its lookup table.
fn fallback(reg: &Registry, d: ObjRef) -> Fallback {
    let class = reg.class_name(d);
    let v = |n: &str| get(reg, d, n).and_then(|(_, v)| vec_of(&v));
    if class.ends_with("ConstantCurve") {
        let Some((pk, Value::Struct(_, cs))) = get(reg, d, "ConstantCurve") else { return Fallback::Zero };
        let p = &reg.packages[pk];
        let pts = material::struct_array(p, crate::props::find(&cs, "Points"));
        let mut out = vec![];
        for pt in pts {
            let inv = f32_of(crate::props::find(&pt, "InVal")).unwrap_or(0.0);
            let outv = crate::props::find(&pt, "OutVal").and_then(vec_of).unwrap_or([0.0; 3]);
            out.push((inv, outv));
        }
        return Fallback::Curve(out);
    }
    if class.ends_with("Uniform") {
        let lo = v("Min").unwrap_or([0.0; 3]);
        let hi = v("Max").unwrap_or([0.0; 3]);
        return Fallback::Uniform(lo, hi);
    }
    // Constant, ParticleParameter and SoundParameter all carry a Constant default
    Fallback::Const(v("Constant").unwrap_or([0.0; 3]))
}

/// A RawDistributionFloat/Vector property of a module.
fn dist(reg: &Registry, r: ObjRef, name: &str) -> Dist {
    let fields = get_struct(reg, r, name);
    let f = |n: &str| fields.iter().find(|(_, p)| p.name.eq_ignore_ascii_case(n)).map(|(pk, p)| (*pk, &p.value));
    let byte = |n: &str, d: u8| match f(n) {
        Some((_, Value::Byte(b))) => *b,
        Some((_, Value::Int(i))) => *i as u8,
        _ => d,
    };
    let mut out = Dist {
        op: byte("Op", 1),
        elems: byte("LookupTableNumElements", 1),
        chunk: byte("LookupTableChunkSize", 1),
        scale: f("LookupTableTimeScale").and_then(|(_, v)| f32_of(Some(v))).unwrap_or(0.0),
        start: f("LookupTableStartTime").and_then(|(_, v)| f32_of(Some(v))).unwrap_or(0.0),
        ..Default::default()
    };
    if let Some((_, Value::Array(_, d))) = f("LookupTable") {
        out.table = d.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    }
    if let Some((pk, Value::Object(o))) = f("Distribution") {
        if *o != 0 {
            if let Some(d) = reg.resolve(pk, *o) {
                out.fallback = fallback(reg, d);
            }
        }
    }
    out
}

fn axes6(reg: &Registry, r: ObjRef) -> [bool; 6] {
    ["Positive_X", "Positive_Y", "Positive_Z", "Negative_X", "Negative_Y", "Negative_Z"].map(|n| bool_of(reg, r, n, true))
}

fn module(reg: &Registry, m: ObjRef) -> Option<Module> {
    if !bool_of(reg, m, "bEnabled", true) {
        return None;
    }
    let class = reg.class_name(m);
    let c = class.strip_prefix("ParticleModule").unwrap_or(&class).trim_end_matches("_Seeded");
    let d = |n: &str| dist(reg, m, n);
    let b = |n: &str, def: bool| bool_of(reg, m, n, def);
    Some(match c {
        "Lifetime" => Module::Lifetime(d("Lifetime")),
        "Size" => Module::Size(d("StartSize")),
        "SizeMultiplyLife" => Module::SizeMultiplyLife { d: d("LifeMultiplier"), axes: [b("MultiplyX", true), b("MultiplyY", true), b("MultiplyZ", true)] },
        "SizeScale" => Module::SizeScale { d: d("SizeScale"), axes: [b("EnableX", true), b("EnableY", true), b("EnableZ", true)] },
        "Velocity" => Module::Velocity { d: d("StartVelocity"), radial: d("StartVelocityRadial"), world: b("bInWorldSpace", false) },
        "VelocityOverLifetime" => Module::VelocityOverLife { d: d("VelOverLife"), absolute: b("Absolute", false), world: b("bInWorldSpace", false) },
        "Acceleration" => Module::Acceleration { d: d("Acceleration"), world: b("bAlwaysInWorldSpace", false) },
        "AccelerationOverLifetime" => Module::AccelerationOverLife { d: d("AccelOverLife"), world: b("bAlwaysInWorldSpace", false) },
        "VelocityDecay" | "Drag" => Module::Drag(d("DragCoefficient")),
        "Color" => Module::Color { c: d("StartColor"), a: d("StartAlpha") },
        "ColorOverLife" => Module::ColorOverLife { c: d("ColorOverLife"), a: d("AlphaOverLife") },
        "ColorScaleOverLife" => Module::ColorScaleOverLife { c: d("ColorScaleOverLife"), a: d("AlphaScaleOverLife"), emitter_time: b("bEmitterTime", false) },
        "Rotation" => Module::Rotation(d("StartRotation")),
        "RotationRate" => Module::RotationRate(d("StartRotationRate")),
        "Location" => Module::Location(d("StartLocation")),
        "LocationPrimitiveSphere" => Module::Sphere {
            radius: d("StartRadius"),
            offset: d("StartLocation"),
            axes: axes6(reg, m),
            surface: b("SurfaceOnly", false),
            velocity: b("Velocity", false),
            vel_scale: d("VelocityScale"),
        },
        "LocationPrimitiveCylinder" => Module::Cylinder {
            radius: d("StartRadius"),
            height: d("StartHeight"),
            offset: d("StartLocation"),
            axes: axes6(reg, m),
            surface: b("SurfaceOnly", false),
            velocity: b("Velocity", false),
            vel_scale: d("VelocityScale"),
            axis: match enum_of(reg, m, "HeightAxis").as_str() {
                "PMLPC_HEIGHTAXIS_X" => 0,
                "PMLPC_HEIGHTAXIS_Y" => 1,
                _ => 2,
            },
        },
        "SubUV" => Module::SubUV(d("SubImageIndex")),
        "Orbit" => Module::Orbit { offset: d("OffsetAmount"), rotation: d("RotationAmount"), rate: d("RotationRateAmount") },
        "MeshRotation" => Module::MeshRotation(d("StartRotation")),
        "MeshRotationRate" => Module::MeshRotationRate(d("StartRotationRate")),
        "Collision" => Module::Collision {
            damping: d("DampingFactor"),
            max: d("MaxCollisions"),
            completion: match enum_of(reg, m, "CollisionCompletionOption").as_str() {
                "EPCC_Freeze" => 1,
                "EPCC_HaltCollisions" => 2,
                "EPCC_FreezeTranslation" => 3,
                "EPCC_FreezeRotation" => 4,
                "EPCC_FreezeMovement" => 5,
                _ => 0,
            },
        },
        "SpawnPerUnit" => Module::SpawnPerUnit { unit: float_of(reg, m, "UnitScalar", 50.0).max(1e-3), per: d("SpawnPerUnit"), ignore_rate: b("bIgnoreSpawnRateWhenMoving", false) },
        _ => Module::Other(c.to_string()),
    })
}

fn blend_of(reg: &Registry, mat: ObjRef) -> Blend {
    let mut cur = Some(mat);
    for _ in 0..16 {
        let Some(r) = cur else { break };
        if reg.class_name(r).contains("MaterialInstance") {
            cur = obj_of(reg, r, "Parent");
            continue;
        }
        return match enum_of(reg, r, "BlendMode").as_str() {
            "BLEND_Opaque" | "" => Blend::Opaque,
            "BLEND_Masked" => Blend::Masked,
            "BLEND_Additive" => Blend::Additive,
            "BLEND_Modulate" | "BLEND_ModulateAndAdd" => Blend::Modulate,
            _ => Blend::Translucent,
        };
    }
    Blend::Translucent
}

fn emitter(reg: &Registry, e: ObjRef) -> Option<Emitter> {
    let class = reg.class_name(e);
    let name = match get(reg, e, "EmitterName") {
        Some((_, Value::Name(n))) => n,
        _ => reg.packages[e.pkg].obj_name(e.export as i32 + 1),
    };
    if enum_of(reg, e, "EmitterRenderMode") == "ERM_None" {
        return None;
    }
    let lod = *objs_of(reg, e, "LODLevels").first()?;
    if !bool_of(reg, lod, "bEnabled", true) {
        return None;
    }
    let req = obj_of(reg, lod, "RequiredModule")?;
    let spawn = obj_of(reg, lod, "SpawnModule");
    let material = obj_of(reg, req, "Material");
    let (texture, opacity, blend) = match material {
        Some(m) => (
            material::resolve_inputs(reg as &dyn Objects, m, &["EmissiveColor", "DiffuseColor", "Opacity"]).diffuse,
            material::resolve_inputs(reg as &dyn Objects, m, &["Opacity", "OpacityMask"]).diffuse,
            blend_of(reg, m),
        ),
        None => (None, None, Blend::Translucent),
    };
    let mut unsupported = (!class.contains("Sprite")).then(|| class.clone());
    let mut mesh = None;
    let (mut ribbon, mut beam) = (None, None);
    let lod_modules = objs_of(reg, lod, "Modules");
    let module_of = |name: &str| lod_modules.iter().copied().find(|&m| reg.class_name(m).contains(name) && bool_of(reg, m, "bEnabled", true));
    if let Some(td) = obj_of(reg, lod, "TypeDataModule") {
        let tc = reg.class_name(td);
        let int = |n: &str, d: i32| match get(reg, td, n) {
            Some((_, Value::Int(i))) => i,
            _ => d,
        };
        if tc.contains("TypeDataMesh") {
            mesh = obj_of(reg, td, "Mesh");
            unsupported = mesh.is_none().then_some(tc);
        } else if tc.contains("TypeDataRibbon") || tc.contains("TypeDataTrail") {
            ribbon = Some(Ribbon { max_in_trail: int("MaxParticleInTrailCount", 500).max(2) as u32, tiling: float_of(reg, td, "TilingDistance", 0.0) });
            unsupported = None;
        } else if tc.contains("TypeDataBeam") {
            beam = Some(Beam {
                method: match enum_of(reg, td, "BeamMethod").as_str() {
                    "PEB2M_Target" => 1,
                    "PEB2M_Branch" => 2,
                    _ => 0,
                },
                distance: dist(reg, td, "Distance"),
                tex_tile: int("TextureTile", 1).max(1) as f32,
                sheets: int("Sheets", 1).max(1) as u32,
                source: module_of("BeamSource").map(|m| dist(reg, m, "Source")),
                target: module_of("BeamTarget").map(|m| dist(reg, m, "Target")),
                noise: module_of("BeamNoise").filter(|&m| bool_of(reg, m, "bLowFreq_Enabled", false)).map(|m| {
                    let f = match get(reg, m, "Frequency") {
                        Some((_, Value::Int(i))) => i.max(1) as u32,
                        _ => 1,
                    };
                    (f, dist(reg, m, "NoiseRange"), float_of(reg, m, "NoiseLockTime", 0.0))
                }),
            });
            unsupported = None;
        } else {
            unsupported = Some(tc);
        }
    }
    let mesh_material = lod_modules
        .iter()
        .filter(|&&m| reg.class_name(m).contains("MeshMaterial") && bool_of(reg, m, "bEnabled", true))
        .find_map(|&m| objs_of(reg, m, "MeshMaterials").into_iter().next());
    let axis_lock = lod_modules
        .iter()
        .filter(|&&m| reg.class_name(m).contains("OrientationAxisLock") && bool_of(reg, m, "bEnabled", true))
        .map(|&m| match enum_of(reg, m, "LockAxisFlags").as_str() {
            "EPAL_X" => 1,
            "EPAL_Y" => 2,
            "EPAL_Z" => 3,
            "EPAL_NEGATIVE_X" => 4,
            "EPAL_NEGATIVE_Y" => 5,
            "EPAL_NEGATIVE_Z" => 6,
            "EPAL_ROTATE_X" => 7,
            "EPAL_ROTATE_Y" => 8,
            "EPAL_ROTATE_Z" => 9,
            _ => 0,
        })
        .last()
        .unwrap_or(0);
    let mut rate = spawn.map(|s| dist(reg, s, "Rate")).unwrap_or_default();
    let rate_scale = spawn.map(|s| dist(reg, s, "RateScale")).unwrap_or_else(|| Dist::constant([1.0; 3]));
    if !rate.is_set() || rate.eval1(0.0, 0.5) == 0.0 {
        let legacy = dist(reg, req, "SpawnRate");
        if legacy.eval1(0.0, 0.5) > 0.0 {
            rate = legacy;
        }
    }
    let mut bursts = vec![];
    for src in [spawn, Some(req)].into_iter().flatten() {
        if let Some((pk, v)) = get(reg, src, "BurstList") {
            for b in material::struct_array(&reg.packages[pk], Some(&v)) {
                let i = |n: &str| match crate::props::find(&b, n) {
                    Some(Value::Int(i)) => *i,
                    _ => 0,
                };
                let lo = match crate::props::find(&b, "CountLow") {
                    Some(Value::Int(i)) => *i,
                    _ => -1,
                };
                bursts.push((i("Count"), lo, f32_of(crate::props::find(&b, "Time")).unwrap_or(0.0)));
            }
        }
    }
    let modules = lod_modules.into_iter().filter_map(|m| module(reg, m)).collect();
    let int = |n: &str, d: i32| match get(reg, req, n) {
        Some((_, Value::Int(i))) => i,
        _ => d,
    };
    let duration = float_of(reg, req, "EmitterDuration", 1.0);
    Some(Emitter {
        name,
        material,
        texture,
        opacity,
        blend,
        duration: if duration <= 0.0 { 1.0 } else { duration },
        loops: int("EmitterLoops", 0),
        delay: float_of(reg, req, "EmitterDelay", 0.0),
        local_space: bool_of(reg, req, "bUseLocalSpace", false),
        align: match enum_of(reg, req, "ScreenAlignment").as_str() {
            "PSA_Rectangle" => 1,
            "PSA_Velocity" => 2,
            _ => 0,
        },
        sub_h: int("SubImages_Horizontal", 1).max(1) as u32,
        sub_v: int("SubImages_Vertical", 1).max(1) as u32,
        sub_interp: match enum_of(reg, req, "InterpolationMethod").as_str() {
            "PSUVIM_Linear" => 1,
            "PSUVIM_Linear_Blend" => 2,
            "PSUVIM_Random" => 3,
            "PSUVIM_Random_Blend" => 4,
            _ => 0,
        },
        rate,
        rate_scale,
        bursts,
        max_particles: if bool_of(reg, req, "bUseMaxDrawCount", true) { int("MaxDrawCount", 500).clamp(1, 2000) as u32 } else { 2000 },
        modules,
        unsupported,
        mesh,
        mesh_material,
        axis_lock,
        ribbon,
        beam,
    })
}

/// Read a ParticleSystem export.
pub fn load_system(reg: &Registry, r: ObjRef) -> Option<System> {
    if reg.class_name(r) != "ParticleSystem" {
        return None;
    }
    let emitters = objs_of(reg, r, "Emitters").into_iter().filter_map(|e| emitter(reg, e)).collect();
    Some(System { path: reg.path(r), emitters, warmup: float_of(reg, r, "WarmupTime", 0.0) })
}
