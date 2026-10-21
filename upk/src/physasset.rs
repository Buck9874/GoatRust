//! UPhysicsAsset: per-bone collision bodies (spheres, capsules, boxes) and joint constraints.

use crate::material::{object_array, struct_array};
use crate::package::Package;
use crate::props::{self, Prop, Value};
use anyhow::Result;

#[derive(Debug, Clone)]
pub enum ShapeKind {
    Sphere { radius: f32 },
    /// Capsule along the element's local Z axis; `length` is the cylinder part.
    Capsule { radius: f32, length: f32 },
    /// Full box extents.
    Box { x: f32, y: f32, z: f32 },
    /// Convex hull points (bone/mesh space, Unreal units).
    Convex { verts: Vec<[f32; 3]> },
}

#[derive(Debug, Clone)]
pub struct Shape {
    pub kind: ShapeKind,
    /// Element transform in bone space (UE row-major FMatrix: rows = X, Y, Z axes, origin).
    pub tm: [[f32; 4]; 4],
}

#[derive(Debug, Clone)]
pub struct Body {
    pub bone: String,
    pub shapes: Vec<Shape>,
    pub no_collision: bool,
    pub fixed: bool,
}

#[derive(Debug, Clone)]
pub struct Constraint {
    /// Child bone (frame 1) and parent bone (frame 2).
    pub bone1: String,
    pub bone2: String,
    /// Frames in PhysX units (metres); primary = twist axis.
    pub pos1: [f32; 3],
    pub pri1: [f32; 3],
    pub sec1: [f32; 3],
    pub pos2: [f32; 3],
    pub pri2: [f32; 3],
    pub sec2: [f32; 3],
    pub swing_limited: bool,
    pub twist_limited: bool,
    pub swing1: f32,
    pub swing2: f32,
    pub twist: f32,
}

#[derive(Debug, Clone, Default)]
pub struct PhysicsAsset {
    pub bodies: Vec<Body>,
    pub constraints: Vec<Constraint>,
}

fn f(ps: &[Prop], n: &str) -> f32 {
    match props::find(ps, n) {
        Some(Value::Float(v)) => *v,
        _ => 0.0,
    }
}
fn v3(ps: &[Prop], n: &str) -> [f32; 3] {
    match props::find(ps, n) {
        Some(Value::Vector(v)) => *v,
        _ => [0.0; 3],
    }
}
fn name(ps: &[Prop], n: &str) -> String {
    match props::find(ps, n) {
        Some(Value::Name(s)) => s.clone(),
        _ => String::new(),
    }
}
fn b(ps: &[Prop], n: &str) -> bool {
    matches!(props::find(ps, n), Some(Value::Bool(true)))
}

fn matrix(ps: &[Prop]) -> [[f32; 4]; 4] {
    let mut m = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
    match props::find(ps, "TM") {
        Some(Value::Raw(_, d)) if d.len() == 64 => {
            // each FPlane row is serialized W first: (W, X, Y, Z)
            let rd = |i: usize| f32::from_le_bytes([d[i * 4], d[i * 4 + 1], d[i * 4 + 2], d[i * 4 + 3]]);
            for (r, row) in m.iter_mut().enumerate() {
                *row = [rd(r * 4 + 1), rd(r * 4 + 2), rd(r * 4 + 3), rd(r * 4)];
            }
        }
        // a Matrix written as tagged planes
        Some(Value::Struct(_, planes)) => {
            for (r, pn) in ["XPlane", "YPlane", "ZPlane", "WPlane"].iter().enumerate() {
                if let Some(Value::Struct(_, p)) = props::find(planes, pn) {
                    m[r] = [f(p, "X"), f(p, "Y"), f(p, "Z"), f(p, "W")];
                }
            }
        }
        _ => {}
    }
    m
}

/// Collision shapes and physical material of an RB_BodySetup.
pub fn body_setup(p: &Package, obj: i32) -> Option<(Vec<Shape>, i32, Vec<Prop>)> {
    if obj <= 0 {
        return None;
    }
    let (_, bs) = props::export_props(p, obj as usize - 1).ok()?;
    let shapes = match props::find(&bs, "AggGeom") {
        Some(Value::Struct(_, agg)) => agg_geom(p, agg),
        _ => vec![],
    };
    let physmat = match props::find(&bs, "PhysMaterial") {
        Some(Value::Object(o)) => *o,
        _ => 0,
    };
    Some((shapes, physmat, bs))
}

/// The shapes of a KAggregateGeom struct (RB_BodySetup.AggGeom, BrushComponent.BrushAggGeom).
pub fn agg_geom(p: &Package, agg: &[Prop]) -> Vec<Shape> {
    let mut shapes = Vec::new();
    {
        for e in struct_array(p, props::find(agg, "SphereElems")) {
            shapes.push(Shape { kind: ShapeKind::Sphere { radius: f(&e, "Radius") }, tm: matrix(&e) });
        }
        for e in struct_array(p, props::find(agg, "SphylElems")) {
            shapes.push(Shape { kind: ShapeKind::Capsule { radius: f(&e, "Radius"), length: f(&e, "Length") }, tm: matrix(&e) });
        }
        for e in struct_array(p, props::find(agg, "BoxElems")) {
            shapes.push(Shape { kind: ShapeKind::Box { x: f(&e, "X"), y: f(&e, "Y"), z: f(&e, "Z") }, tm: matrix(&e) });
        }
        for e in struct_array(p, props::find(agg, "ConvexElems")) {
            if let Some(Value::Array(_, d)) = props::find(&e, "VertexData") {
                let verts: Vec<[f32; 3]> = d
                    .chunks_exact(12)
                    .map(|c| {
                        let g = |o: usize| f32::from_le_bytes([c[o], c[o + 1], c[o + 2], c[o + 3]]);
                        [g(0), g(4), g(8)]
                    })
                    .collect();
                if verts.len() >= 4 {
                    let mut tm = [[0.0; 4]; 4];
                    for (i, row) in tm.iter_mut().enumerate() {
                        row[i] = 1.0;
                    }
                    shapes.push(Shape { kind: ShapeKind::Convex { verts }, tm });
                }
            }
        }
    }
    shapes
}

/// PhysicalMaterial density/friction/restitution (falls back to the parent material).
pub fn phys_material(p: &Package, obj: i32) -> (f32, f32, f32) {
    let (mut density, mut friction, mut restitution) = (None, None, None);
    let mut cur = obj;
    for _ in 0..4 {
        if cur <= 0 {
            break;
        }
        let Ok((_, ps)) = props::export_props(p, cur as usize - 1) else { break };
        density = density.or(match props::find(&ps, "Density") {
            Some(Value::Float(v)) => Some(*v),
            _ => None,
        });
        friction = friction.or(match props::find(&ps, "Friction") {
            Some(Value::Float(v)) => Some(*v),
            _ => None,
        });
        restitution = restitution.or(match props::find(&ps, "Restitution") {
            Some(Value::Float(v)) => Some(*v),
            _ => None,
        });
        cur = match props::find(&ps, "Parent") {
            Some(Value::Object(o)) => *o,
            _ => 0,
        };
    }
    // UE3 PhysicalMaterial defaults: Density 1, Friction 0.7, Restitution 0.3
    (density.unwrap_or(1.0), friction.unwrap_or(0.7), restitution.unwrap_or(0.3))
}

impl PhysicsAsset {
    pub fn parse(p: &Package, export: usize) -> Result<Self> {
        let (_, ps) = props::export_props(p, export)?;
        let mut out = PhysicsAsset::default();
        for o in object_array(props::find(&ps, "BodySetup")) {
            if o <= 0 {
                continue;
            }
            let Some((shapes, _, bs)) = body_setup(p, o) else { continue };
            out.bodies.push(Body { bone: name(&bs, "BoneName"), shapes, no_collision: b(&bs, "bNoCollision"), fixed: b(&bs, "bFixed") });
        }
        for o in object_array(props::find(&ps, "ConstraintSetup")) {
            if o <= 0 {
                continue;
            }
            let Ok((_, cs)) = props::export_props(p, o as usize - 1) else { continue };
            let def = |n: &str, d: [f32; 3]| if props::find(&cs, n).is_some() { v3(&cs, n) } else { d };
            out.constraints.push(Constraint {
                bone1: name(&cs, "ConstraintBone1"),
                bone2: name(&cs, "ConstraintBone2"),
                pos1: v3(&cs, "Pos1"),
                pri1: def("PriAxis1", [1.0, 0.0, 0.0]),
                sec1: def("SecAxis1", [0.0, 1.0, 0.0]),
                pos2: v3(&cs, "Pos2"),
                pri2: def("PriAxis2", [1.0, 0.0, 0.0]),
                sec2: def("SecAxis2", [0.0, 1.0, 0.0]),
                swing_limited: b(&cs, "bSwingLimited"),
                twist_limited: b(&cs, "bTwistLimited"),
                swing1: f(&cs, "Swing1LimitAngle"),
                swing2: f(&cs, "Swing2LimitAngle"),
                twist: f(&cs, "TwistLimitAngle"),
            });
        }
        Ok(out)
    }
}

/// The GGPhysicalMaterialProperty of a PhysicalMaterial (or its nearest parent).
pub fn phys_property(objs: &dyn crate::material::Objects, physmat: crate::registry::ObjRef) -> Option<crate::registry::ObjRef> {
    let mut cur = Some(physmat);
    for _ in 0..4 {
        let r = cur?;
        let ps = objs.props_of(r)?;
        if let Some(Value::Object(o)) = props::find(&ps, "PhysicalMaterialProperty") {
            if let Some(pr) = objs.resolve_obj(r.pkg, *o) {
                return Some(pr);
            }
        }
        cur = match props::find(&ps, "Parent") {
            Some(Value::Object(o)) => objs.resolve_obj(r.pkg, *o),
            _ => None,
        };
    }
    None
}

/// What a physical material sounds like: GGPhysicalMaterialProperty.MaterialType (footsteps) and
/// the PhysicalMaterial's ImpactSound / ImpactThreshold / ImpactReFireDelay (nearest parent that
/// sets them).
#[derive(Debug, Clone, Default)]
pub struct SurfaceInfo {
    pub material_type: String,
    pub impact_sound: Option<crate::registry::ObjRef>,
    pub impact_threshold: f32,
    pub impact_refire: f32,
}

pub fn surface_info(objs: &dyn crate::material::Objects, physmat: crate::registry::ObjRef) -> SurfaceInfo {
    let mut out = SurfaceInfo::default();
    if let Some(pp) = phys_property(objs, physmat).and_then(|pr| objs.props_of(pr)) {
        if let Some(Value::Name(n)) = props::find(&pp, "MaterialType") {
            out.material_type = n.clone();
        }
    }
    let mut cur = Some(physmat);
    for _ in 0..4 {
        let Some(r) = cur else { break };
        let Some(ps) = objs.props_of(r) else { break };
        if let Some(Value::Object(o)) = props::find(&ps, "ImpactSound") {
            if let Some(cue) = objs.resolve_obj(r.pkg, *o) {
                out.impact_sound = Some(cue);
                out.impact_threshold = match props::find(&ps, "ImpactThreshold") {
                    Some(Value::Float(f)) => *f,
                    _ => 0.0,
                };
                out.impact_refire = match props::find(&ps, "ImpactReFireDelay") {
                    Some(Value::Float(f)) => *f,
                    _ => 0.0,
                };
                break;
            }
        }
        cur = match props::find(&ps, "Parent") {
            Some(Value::Object(o)) => objs.resolve_obj(r.pkg, *o),
            _ => None,
        };
    }
    out
}

/// GGPhysicalMaterialProperty.mShouldTripGoat: running into it at speed ragdolls the goat.
pub fn trips_goat(objs: &dyn crate::material::Objects, physmat: crate::registry::ObjRef) -> bool {
    phys_property(objs, physmat)
        .and_then(|pr| objs.props_of(pr))
        .is_some_and(|pp| matches!(props::find(&pp, "mShouldTripGoat"), Some(Value::Bool(true))))
}

/// GGPhysicalMaterialProperty of a PhysicalMaterial: (mScore, mActorInGameName).
pub fn score_info(objs: &dyn crate::material::Objects, physmat: crate::registry::ObjRef) -> Option<(i32, String)> {
    let mut cur = Some(physmat);
    for _ in 0..4 {
        let r = cur?;
        let ps = objs.props_of(r)?;
        if let Some(Value::Object(o)) = props::find(&ps, "PhysicalMaterialProperty") {
            if let Some(pr) = objs.resolve_obj(r.pkg, *o) {
                let pp = objs.props_of(pr)?;
                let score = match props::find(&pp, "mScore") {
                    Some(Value::Int(v)) => *v,
                    _ => 0,
                };
                let name = match props::find(&pp, "mActorInGameName") {
                    Some(Value::Str(s)) => s.clone(),
                    _ => String::new(),
                };
                return Some((score, name));
            }
        }
        cur = match props::find(&ps, "Parent") {
            Some(Value::Object(o)) => objs.resolve_obj(r.pkg, *o),
            _ => None,
        };
    }
    None
}

/// Material.GetPhysicalMaterial(): the PhysMaterial of a material or the nearest parent.
pub fn material_physmat(objs: &dyn crate::material::Objects, mat: crate::registry::ObjRef) -> Option<crate::registry::ObjRef> {
    let mut cur = Some(mat);
    for _ in 0..8 {
        let r = cur?;
        let ps = objs.props_of(r)?;
        if let Some(Value::Object(o)) = props::find(&ps, "PhysMaterial") {
            if let Some(pm) = objs.resolve_obj(r.pkg, *o) {
                return Some(pm);
            }
        }
        cur = match props::find(&ps, "Parent") {
            Some(Value::Object(o)) => objs.resolve_obj(r.pkg, *o),
            _ => None,
        };
    }
    None
}
