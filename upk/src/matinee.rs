//! UE3 matinee data (InterpData -> InterpGroup -> tracks) for Kismet's SeqAct_Interp: movement
//! tracks (InterpTrackMove PosTrack/EulerTrack, or its InterpTrackMoveAxis sub-tracks) and event
//! tracks. Animation, director, fade and property tracks are not read.

use crate::material::struct_array;
use crate::props::{self, Prop, Value};
use crate::registry::{ObjRef, Registry};

/// One InterpCurve point; `out` holds 1 (float curve) or 3 (vector curve) values.
#[derive(Debug, Clone)]
pub struct Point {
    pub t: f32,
    pub out: [f32; 3],
    pub arrive: [f32; 3],
    pub leave: [f32; 3],
    /// 0 linear, 1 constant, 2 Hermite (CurveAuto/User/Break/AutoClamped).
    pub mode: u8,
}

#[derive(Debug, Clone, Default)]
pub struct Curve {
    pub points: Vec<Point>,
}

fn hermite(p0: f32, t0: f32, p1: f32, t1: f32, a: f32) -> f32 {
    let a2 = a * a;
    let a3 = a2 * a;
    (2.0 * a3 - 3.0 * a2 + 1.0) * p0 + (a3 - 2.0 * a2 + a) * t0 + (a3 - a2) * t1 + (-2.0 * a3 + 3.0 * a2) * p1
}

impl Curve {
    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// FInterpCurve::Eval: clamps outside the keys; the left key's mode picks the segment's shape.
    pub fn eval(&self, t: f32) -> [f32; 3] {
        let p = &self.points;
        match p.len() {
            0 => return [0.0; 3],
            1 => return p[0].out,
            _ => {}
        }
        if t <= p[0].t {
            return p[0].out;
        }
        if t >= p[p.len() - 1].t {
            return p[p.len() - 1].out;
        }
        let i = p.windows(2).position(|w| t < w[1].t).unwrap_or(p.len() - 2);
        let (a, b) = (&p[i], &p[i + 1]);
        let span = b.t - a.t;
        if span <= 1e-6 {
            return b.out;
        }
        let alpha = (t - a.t) / span;
        let mut out = [0.0; 3];
        for k in 0..3 {
            out[k] = match a.mode {
                1 => a.out[k],
                2 => hermite(a.out[k], a.leave[k] * span, b.out[k], b.arrive[k] * span, alpha),
                _ => a.out[k] + (b.out[k] - a.out[k]) * alpha,
            };
        }
        out
    }
}

/// A move track: either the vector tracks or six per-axis float tracks.
#[derive(Debug, Clone, Default)]
pub struct MoveTrack {
    /// IMF_RelativeToInitial (the default) vs IMF_World.
    pub relative: bool,
    pub pos: Curve,
    /// (Roll, Pitch, Yaw) in degrees.
    pub euler: Curve,
    /// Per-axis tracks: index 0..3 translation X/Y/Z, 3..6 rotation X/Y/Z (degrees).
    pub axes: Vec<(usize, Curve)>,
}

impl MoveTrack {
    /// (translation, euler degrees (roll, pitch, yaw)) at time `t`.
    pub fn eval(&self, t: f32) -> ([f32; 3], [f32; 3]) {
        if self.axes.is_empty() {
            return (self.pos.eval(t), self.euler.eval(t));
        }
        let (mut pos, mut rot) = ([0.0; 3], [0.0; 3]);
        for (axis, c) in &self.axes {
            let v = c.eval(t)[0];
            if *axis < 3 {
                pos[*axis] = v;
            } else {
                rot[*axis - 3] = v;
            }
        }
        (pos, rot)
    }
}

#[derive(Debug, Clone, Default)]
pub struct Group {
    pub name: String,
    pub movement: Option<MoveTrack>,
    /// (time, event name) keys.
    pub events: Vec<(f32, String)>,
}

#[derive(Debug, Clone, Default)]
pub struct InterpData {
    pub path: String,
    pub length: f32,
    pub groups: Vec<Group>,
}

fn f(ps: &[Prop], name: &str) -> Option<f32> {
    match props::find(ps, name) {
        Some(Value::Float(v)) => Some(*v),
        Some(Value::Int(v)) => Some(*v as f32),
        _ => None,
    }
}

fn v3(v: Option<&Value>) -> [f32; 3] {
    match v {
        Some(Value::Vector(v)) => *v,
        Some(Value::Float(x)) => [*x, 0.0, 0.0],
        _ => [0.0; 3],
    }
}

fn mode(v: Option<&Value>) -> u8 {
    let name = match v {
        Some(Value::Enum(e)) => e.as_str(),
        Some(Value::Byte(b)) => match b {
            0 => "CIM_Linear",
            1 => "CIM_CurveAuto",
            2 => "CIM_Constant",
            _ => "CIM_CurveUser",
        },
        _ => "CIM_Linear",
    };
    match name {
        "CIM_Linear" => 0,
        "CIM_Constant" => 1,
        _ => 2,
    }
}

/// An InterpCurveFloat/Vector struct property of an export.
fn curve(reg: &Registry, r: ObjRef, ps: &[Prop], name: &str) -> Curve {
    let Some(Value::Struct(_, inner)) = props::find(ps, name) else { return Curve::default() };
    let p = &reg.packages[r.pkg];
    let points = struct_array(p, props::find(inner, "Points"))
        .into_iter()
        .map(|pt| Point {
            t: f(&pt, "InVal").unwrap_or(0.0),
            out: v3(props::find(&pt, "OutVal")),
            arrive: v3(props::find(&pt, "ArriveTangent")),
            leave: v3(props::find(&pt, "LeaveTangent")),
            mode: mode(props::find(&pt, "InterpMode")),
        })
        .collect();
    Curve { points }
}

fn objs(reg: &Registry, r: ObjRef, ps: &[Prop], name: &str) -> Vec<ObjRef> {
    crate::material::object_array(props::find(ps, name)).into_iter().filter_map(|o| reg.resolve(r.pkg, o)).collect()
}

fn move_track(reg: &Registry, r: ObjRef, ps: &[Prop]) -> MoveTrack {
    let relative = !matches!(props::find(ps, "MoveFrame"), Some(Value::Enum(e)) if e == "IMF_World");
    let mut axes = vec![];
    for sub in objs(reg, r, ps, "SubTracks") {
        if !reg.class_name(sub).contains("MoveAxis") {
            continue;
        }
        let sp = reg.props(sub);
        let axis = match props::find(&sp, "MoveAxis") {
            Some(Value::Enum(e)) => match e.as_str() {
                "AXIS_TranslationY" => 1,
                "AXIS_TranslationZ" => 2,
                "AXIS_RotationX" => 3,
                "AXIS_RotationY" => 4,
                "AXIS_RotationZ" => 5,
                _ => 0,
            },
            Some(Value::Byte(b)) => (*b as usize).min(5),
            _ => 0,
        };
        let c = curve(reg, sub, &sp, "FloatTrack");
        if !c.is_empty() {
            axes.push((axis, c));
        }
    }
    MoveTrack { relative, pos: curve(reg, r, ps, "PosTrack"), euler: curve(reg, r, ps, "EulerTrack"), axes }
}

/// Read an InterpData export.
pub fn load_interp(reg: &Registry, r: ObjRef) -> Option<InterpData> {
    if reg.class_name(r) != "InterpData" {
        return None;
    }
    let ps = reg.props(r);
    let mut groups = vec![];
    for g in objs(reg, r, &ps, "InterpGroups") {
        let gp = reg.props(g);
        let name = match props::find(&gp, "GroupName") {
            Some(Value::Name(n)) => n.clone(),
            _ => "InterpGroup".into(),
        };
        let mut group = Group { name, ..Default::default() };
        for t in objs(reg, g, &gp, "InterpTracks") {
            let class = reg.class_name(t);
            let tp = reg.props(t);
            if class == "InterpTrackMove" {
                group.movement = Some(move_track(reg, t, &tp));
            } else if class == "InterpTrackEvent" {
                for k in struct_array(&reg.packages[t.pkg], props::find(&tp, "EventTrack")) {
                    let name = match props::find(&k, "EventName") {
                        Some(Value::Name(n)) => n.clone(),
                        _ => continue,
                    };
                    group.events.push((f(&k, "Time").unwrap_or(0.0), name));
                }
            }
        }
        groups.push(group);
    }
    Some(InterpData { path: reg.path(r), length: f(&ps, "InterpLength").unwrap_or(5.0).max(0.0), groups })
}
