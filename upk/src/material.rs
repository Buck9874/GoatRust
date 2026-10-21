//! Approximate UE3 materials: find the diffuse texture / colour behind a material or instance.
//! Works across packages (materials referenced by a map often live in Startup.upk).

use crate::package::Package;
use crate::props::{self, Prop, Value};
use crate::reader::Reader;
use crate::registry::{ObjRef, Registry};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct MatInfo {
    /// Texture2D object.
    pub diffuse: Option<ObjRef>,
    pub color: Option<[f32; 4]>,
    pub masked: bool,
    pub translucent: bool,
    pub two_sided: bool,
    /// BLEND_Masked: the texture behind OpacityMask, its channel (0..3 = RGBA) and whether a
    /// OneMinus inverts it.
    pub mask: Option<(ObjRef, usize, bool)>,
    /// The texture behind the Normal input (tangent space, DirectX green).
    pub normal: Option<ObjRef>,
    /// SpecularColor when connected: its constant colour, or mid grey for a texture.
    pub spec: Option<[f32; 3]>,
    /// SpecularPower when connected to a constant (Phong exponent).
    pub spec_power: Option<f32>,
}

/// Object lookup used by the resolver: one package, or a registry of several.
pub trait Objects {
    fn package(&self, pkg: usize) -> &Package;
    fn resolve_obj(&self, from_pkg: usize, idx: i32) -> Option<ObjRef>;
    fn props_of(&self, r: ObjRef) -> Option<Vec<Prop>> {
        props::export_props(self.package(r.pkg), r.export).ok().map(|x| x.1)
    }
    fn class_of(&self, r: ObjRef) -> String {
        self.package(r.pkg).class_name(r.export as i32 + 1)
    }
    fn name_of(&self, r: ObjRef) -> String {
        self.package(r.pkg).obj_name(r.export as i32 + 1)
    }
}

/// A single package; imports stay unresolved.
pub struct Single<'a>(pub &'a Package);

impl Objects for Single<'_> {
    fn package(&self, _pkg: usize) -> &Package {
        self.0
    }
    fn resolve_obj(&self, _from: usize, idx: i32) -> Option<ObjRef> {
        (idx > 0).then(|| ObjRef { pkg: 0, export: idx as usize - 1 })
    }
}

impl Objects for Registry {
    fn package(&self, pkg: usize) -> &Package {
        &self.packages[pkg]
    }
    fn resolve_obj(&self, from: usize, idx: i32) -> Option<ObjRef> {
        self.resolve(from, idx)
    }
}

/// Parse an ArrayProperty whose elements are tagged-property structs.
pub fn struct_array(p: &Package, v: Option<&Value>) -> Vec<Vec<Prop>> {
    let Some(Value::Array(n, d)) = v else { return vec![] };
    let mut r = Reader::new(d);
    let mut out = Vec::new();
    for _ in 0..*n {
        match props::parse_props(p, &mut r) {
            Ok(ps) => out.push(ps),
            Err(_) => break,
        }
    }
    out
}

/// Object indices stored in an ArrayProperty of object references.
pub fn object_array(v: Option<&Value>) -> Vec<i32> {
    match v {
        Some(Value::Array(_, d)) => d.chunks_exact(4).map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect(),
        _ => vec![],
    }
}

fn is_normal_like(name: &str) -> bool {
    let n = name.to_lowercase();
    ["nor", "normal", "spec", "mask", "emis", "rough", "detail", "noise", "flicker", "_n", "bump", "height"]
        .iter()
        .any(|k| n.contains(k))
}

/// Resolve an object index as seen from package 0 of a single package.
pub fn resolve(p: &Package, obj: i32) -> MatInfo {
    let s = Single(p);
    match s.resolve_obj(0, obj) {
        Some(r) => resolve_in(&s, r),
        None => MatInfo::default(),
    }
}

pub fn resolve_in(objs: &dyn Objects, start: ObjRef) -> MatInfo {
    resolve_inputs(objs, start, &["DiffuseColor"])
}

/// Like resolve_in, for unlit materials (particles): the texture behind the emissive colour or
/// opacity.
pub fn resolve_emissive(objs: &dyn Objects, start: ObjRef) -> MatInfo {
    resolve_inputs(objs, start, &["EmissiveColor", "Opacity", "DiffuseColor", "OpacityMask"])
}

/// The first texture/colour behind the named material inputs (in order).
pub fn resolve_inputs(objs: &dyn Objects, start: ObjRef, inputs: &[&str]) -> MatInfo {
    let mut tex_params: HashMap<String, ObjRef> = HashMap::new();
    let mut vec_params: HashMap<String, [f32; 4]> = HashMap::new();
    let mut cur = Some(start);
    let mut info = MatInfo::default();
    // Walk MaterialInstanceConstant parents, collecting overrides (child wins).
    for _ in 0..16 {
        let Some(r) = cur else { return info };
        let class = objs.class_of(r);
        let Some(ps) = objs.props_of(r) else { return info };
        let p = objs.package(r.pkg);
        if class.contains("MaterialInstance") {
            for e in struct_array(p, props::find(&ps, "TextureParameterValues")) {
                if let (Some(Value::Name(n)), Some(Value::Object(t))) = (props::find(&e, "ParameterName"), props::find(&e, "ParameterValue")) {
                    if let Some(tr) = objs.resolve_obj(r.pkg, *t) {
                        tex_params.entry(n.to_lowercase()).or_insert(tr);
                    }
                }
            }
            for e in struct_array(p, props::find(&ps, "VectorParameterValues")) {
                if let (Some(Value::Name(n)), Some(Value::LinearColor(c))) = (props::find(&e, "ParameterName"), props::find(&e, "ParameterValue")) {
                    vec_params.entry(n.to_lowercase()).or_insert(*c);
                }
            }
            cur = match props::find(&ps, "Parent") {
                Some(Value::Object(o)) => objs.resolve_obj(r.pkg, *o),
                _ => None,
            };
            continue;
        }
        // A real Material.
        info.two_sided = matches!(props::find(&ps, "TwoSided"), Some(Value::Bool(true)));
        match props::find(&ps, "BlendMode") {
            Some(Value::Enum(b)) if b == "BLEND_Masked" => info.masked = true,
            Some(Value::Enum(b)) if b.starts_with("BLEND_Trans") || b == "BLEND_Additive" => info.translucent = true,
            _ => {}
        }
        let mut seen = HashSet::new();
        for input in inputs {
            if let Some(Value::Struct(_, inp)) = props::find(&ps, input) {
                if let Some(Value::Object(e)) = props::find(inp, "Expression") {
                    if let Some(er) = objs.resolve_obj(r.pkg, *e) {
                        walk(objs, er, &tex_params, &vec_params, &mut info, &mut seen, 0);
                    }
                }
            }
        }
        if info.diffuse.is_none() && info.color.is_none() {
            // fall back to any diffuse-looking texture parameter
            info.diffuse = tex_params.iter().find(|(k, _)| !is_normal_like(k)).map(|(_, v)| *v);
        }
        // Normal, SpecularColor and SpecularPower (each walked on its own)
        let sub = |name: &str| -> Option<MatInfo> {
            let Some(Value::Struct(_, inp)) = props::find(&ps, name) else { return None };
            let Some(Value::Object(e)) = props::find(inp, "Expression") else { return None };
            let er = objs.resolve_obj(r.pkg, *e)?;
            let mut i = MatInfo::default();
            walk(objs, er, &tex_params, &vec_params, &mut i, &mut HashSet::new(), 0);
            Some(i)
        };
        info.normal = sub("Normal").and_then(|i| i.diffuse);
        info.spec = sub("SpecularColor").and_then(|i| match (i.color, i.diffuse) {
            (Some(c), _) => Some([c[0], c[1], c[2]]),
            (None, Some(_)) => Some([0.5; 3]),
            _ => None,
        });
        info.spec_power = sub("SpecularPower").and_then(|i| i.color.map(|c| c[0]));
        if info.masked {
            if let Some(Value::Struct(_, inp)) = props::find(&ps, "OpacityMask") {
                info.mask = mask_source(objs, r.pkg, inp, &tex_params, false, 0);
            }
            // a mask texture that also feeds DiffuseColor is the main texture (a generic detail
            // texture multiplied in first would otherwise win)
            if let Some((t, _, _)) = info.mask {
                if info.diffuse != Some(t) && !inputs.is_empty() {
                    let mut all = Vec::new();
                    if let Some(Value::Struct(_, inp)) = props::find(&ps, inputs[0]) {
                        if let Some(er) = match props::find(inp, "Expression") {
                            Some(Value::Object(e)) => objs.resolve_obj(r.pkg, *e),
                            _ => None,
                        } {
                            chain_textures(objs, er, &tex_params, &mut all, &mut HashSet::new(), 0);
                        }
                    }
                    if all.contains(&t) {
                        info.diffuse = Some(t);
                    }
                }
            }
        }
        return info;
    }
    info
}

/// The texture, channel and inversion behind a scalar material input (OpacityMask).
fn mask_source(objs: &dyn Objects, pkg: usize, inp: &[Prop], tex: &HashMap<String, ObjRef>, invert: bool, depth: u32) -> Option<(ObjRef, usize, bool)> {
    if depth > 12 {
        return None;
    }
    let e = match props::find(inp, "Expression") {
        Some(Value::Object(o)) if *o != 0 => objs.resolve_obj(pkg, *o)?,
        _ => return None,
    };
    let on = |k: &str| matches!(props::find(inp, k), Some(Value::Int(i)) if *i != 0);
    let channel = ["MaskR", "MaskG", "MaskB", "MaskA"].iter().position(|k| on(k)).unwrap_or(0);
    let class = objs.class_of(e);
    let ps = objs.props_of(e)?;
    if class.contains("TextureSample") {
        let pname = match props::find(&ps, "ParameterName") {
            Some(Value::Name(n)) => Some(n.to_lowercase()),
            _ => None,
        };
        let t = pname.and_then(|n| tex.get(&n).copied()).or(match props::find(&ps, "Texture") {
            Some(Value::Object(o)) if *o != 0 => objs.resolve_obj(e.pkg, *o),
            _ => None,
        })?;
        return Some((t, channel, invert));
    }
    let invert = if class == "MaterialExpressionOneMinus" { !invert } else { invert };
    for q in &ps {
        if let Value::Struct(_, s) = &q.value {
            if let Some(found) = mask_source(objs, e.pkg, s, tex, invert, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

/// Every texture in an expression chain.
fn chain_textures(objs: &dyn Objects, e: ObjRef, tex: &HashMap<String, ObjRef>, out: &mut Vec<ObjRef>, seen: &mut HashSet<ObjRef>, depth: u32) {
    if depth > 24 || !seen.insert(e) {
        return;
    }
    let Some(ps) = objs.props_of(e) else { return };
    if objs.class_of(e).contains("TextureSample") {
        let pname = match props::find(&ps, "ParameterName") {
            Some(Value::Name(n)) => Some(n.to_lowercase()),
            _ => None,
        };
        if let Some(t) = pname.and_then(|n| tex.get(&n).copied()).or(match props::find(&ps, "Texture") {
            Some(Value::Object(o)) if *o != 0 => objs.resolve_obj(e.pkg, *o),
            _ => None,
        }) {
            out.push(t);
        }
    }
    for q in &ps {
        if let Value::Struct(_, s) = &q.value {
            if let Some(Value::Object(o)) = props::find(s, "Expression") {
                if let Some(r) = objs.resolve_obj(e.pkg, *o) {
                    chain_textures(objs, r, tex, out, seen, depth + 1);
                }
            }
        }
    }
}

fn walk(
    objs: &dyn Objects,
    e: ObjRef,
    tex: &HashMap<String, ObjRef>,
    vecs: &HashMap<String, [f32; 4]>,
    info: &mut MatInfo,
    seen: &mut HashSet<ObjRef>,
    depth: u32,
) {
    if depth > 24 || info.diffuse.is_some() || !seen.insert(e) {
        return;
    }
    let class = objs.class_of(e);
    let Some(ps) = objs.props_of(e) else { return };
    // ParticleSubUV and FlipBookSample are TextureSample subclasses (flipbook sprites)
    if class.contains("TextureSample") || class.contains("ParticleSubUV") || class.contains("FlipBookSample") {
        let pname = match props::find(&ps, "ParameterName") {
            Some(Value::Name(n)) => Some(n.to_lowercase()),
            _ => None,
        };
        let t = pname.and_then(|n| tex.get(&n).copied()).or(match props::find(&ps, "Texture") {
            Some(Value::Object(o)) if *o != 0 => objs.resolve_obj(e.pkg, *o),
            _ => None,
        });
        if let Some(t) = t {
            if !is_normal_like(&objs.name_of(t)) || info.diffuse.is_none() {
                info.diffuse = Some(t);
                return;
            }
        }
    }
    if class == "MaterialExpressionVectorParameter" && info.color.is_none() {
        let pname = match props::find(&ps, "ParameterName") {
            Some(Value::Name(n)) => n.to_lowercase(),
            _ => String::new(),
        };
        info.color = vecs.get(&pname).copied().or(match props::find(&ps, "DefaultValue") {
            Some(Value::LinearColor(c)) => Some(*c),
            _ => None,
        });
    }
    if class == "MaterialExpressionConstant3Vector" && info.color.is_none() {
        let g = |n: &str| match props::find(&ps, n) {
            Some(Value::Float(f)) => *f,
            _ => 0.0,
        };
        info.color = Some([g("R"), g("G"), g("B"), 1.0]);
    }
    if class == "MaterialExpressionConstant" && info.color.is_none() {
        if let Some(Value::Float(r)) = props::find(&ps, "R") {
            info.color = Some([*r, *r, *r, 1.0]);
        }
    }
    // Static switches: follow the selected branch first.
    let mut inputs: Vec<i32> = Vec::new();
    if class == "MaterialExpressionStaticSwitchParameter" {
        let on = matches!(props::find(&ps, "DefaultValue"), Some(Value::Bool(true)));
        let order = if on { ["A", "B"] } else { ["B", "A"] };
        for k in order {
            if let Some(Value::Struct(_, s)) = props::find(&ps, k) {
                if let Some(Value::Object(o)) = props::find(s, "Expression") {
                    inputs.push(*o);
                }
            }
        }
    } else {
        for q in &ps {
            if let Value::Struct(_, s) = &q.value {
                if let Some(Value::Object(o)) = props::find(s, "Expression") {
                    inputs.push(*o);
                }
            }
        }
    }
    for i in inputs {
        if let Some(ir) = objs.resolve_obj(e.pkg, i) {
            walk(objs, ir, tex, vecs, info, seen, depth + 1);
        }
    }
}

/// Landscape paint layers: (layer name, diffuse texture, UV tiling per landscape quad).
/// Tiling = product of scalar parameters on the TextureSample's Coordinates chain divided by
/// the TerrainLayerCoords MappingScale.
pub fn landscape_layers(objs: &dyn Objects, mat: ObjRef) -> Vec<(String, ObjRef, f32)> {
    // landscape materials are plain Materials (instances may sit on top)
    let mut cur = Some(mat);
    let mut base = None;
    for _ in 0..8 {
        let Some(r) = cur else { break };
        if !objs.class_of(r).contains("MaterialInstance") {
            base = Some(r);
            break;
        }
        cur = objs.props_of(r).and_then(|ps| match props::find(&ps, "Parent") {
            Some(Value::Object(o)) => objs.resolve_obj(r.pkg, *o),
            _ => None,
        });
    }
    let Some(base) = base else { return vec![] };
    let Some(ps) = objs.props_of(base) else { return vec![] };
    let mut out: Vec<(String, ObjRef, f32)> = Vec::new();
    for o in object_array(props::find(&ps, "Expressions")) {
        let Some(e) = objs.resolve_obj(base.pkg, o) else { continue };
        if !objs.class_of(e).contains("TextureSampleParameter") {
            continue;
        }
        let Some(eps) = objs.props_of(e) else { continue };
        let name = match props::find(&eps, "ParameterName") {
            Some(Value::Name(n)) => n.clone(),
            _ => continue,
        };
        if out.iter().any(|(n, _, _)| n.eq_ignore_ascii_case(&name)) || is_normal_like(&name) {
            continue;
        }
        let Some(tex) = (match props::find(&eps, "Texture") {
            Some(Value::Object(t)) => objs.resolve_obj(e.pkg, *t),
            _ => None,
        }) else {
            continue;
        };
        let coords = match props::find(&eps, "Coordinates") {
            Some(Value::Struct(_, c)) => match props::find(c, "Expression") {
                Some(Value::Object(x)) => objs.resolve_obj(e.pkg, *x),
                _ => None,
            },
            _ => None,
        };
        out.push((name, tex, coord_scale(objs, coords, 0)));
    }
    out
}

fn coord_scale(objs: &dyn Objects, e: Option<ObjRef>, depth: u32) -> f32 {
    let Some(e) = e else { return 1.0 };
    if depth > 8 {
        return 1.0;
    }
    let class = objs.class_of(e);
    let Some(ps) = objs.props_of(e) else { return 1.0 };
    let input = |k: &str| match props::find(&ps, k) {
        Some(Value::Struct(_, c)) => match props::find(c, "Expression") {
            Some(Value::Object(x)) => objs.resolve_obj(e.pkg, *x),
            _ => None,
        },
        _ => None,
    };
    match class.as_str() {
        "MaterialExpressionTerrainLayerCoords" | "MaterialExpressionLandscapeLayerCoords" => match props::find(&ps, "MappingScale") {
            Some(Value::Float(m)) if *m > 0.0 => 1.0 / *m,
            _ => 1.0,
        },
        "MaterialExpressionScalarParameter" => match props::find(&ps, "DefaultValue") {
            Some(Value::Float(v)) => *v,
            _ => 1.0,
        },
        "MaterialExpressionConstant" => match props::find(&ps, "R") {
            Some(Value::Float(v)) => *v,
            _ => 1.0,
        },
        "MaterialExpressionMultiply" => coord_scale(objs, input("A"), depth + 1) * coord_scale(objs, input("B"), depth + 1),
        // bump offset / panners keep the incoming coordinate scale
        _ => coord_scale(objs, input("Coordinate").or_else(|| input("Coordinates")), depth + 1),
    }
}

/// A DiffuseColor expression graph simple enough to evaluate per texel.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A texture sample at UV * tiling.
    Tex { tex: ObjRef, tiling: [f32; 2] },
    Const([f32; 4]),
    /// One channel of the input, as grey.
    Channel(Box<Node>, usize),
    Mul(Box<Node>, Box<Node>),
    Add(Box<Node>, Box<Node>),
    OneMinus(Box<Node>),
    Lerp(Box<Node>, Box<Node>, Box<Node>),
}

impl Node {
    pub fn textures(&self, out: &mut Vec<ObjRef>) {
        match self {
            Node::Tex { tex, .. } => {
                if !out.contains(tex) {
                    out.push(*tex)
                }
            }
            Node::Const(_) => {}
            Node::Channel(a, _) | Node::OneMinus(a) => a.textures(out),
            Node::Mul(a, b) | Node::Add(a, b) => {
                a.textures(out);
                b.textures(out);
            }
            Node::Lerp(a, b, c) => {
                a.textures(out);
                b.textures(out);
                c.textures(out);
            }
        }
    }
}

/// The DiffuseColor graph of a material (instance parents walked; their texture/vector/scalar
/// parameters applied), or None when it uses anything this evaluator doesn't know.
pub fn diffuse_graph(objs: &dyn Objects, start: ObjRef) -> Option<Node> {
    let mut tex: HashMap<String, ObjRef> = HashMap::new();
    let mut vecs: HashMap<String, [f32; 4]> = HashMap::new();
    let mut scalars: HashMap<String, f32> = HashMap::new();
    let mut cur = Some(start);
    for _ in 0..16 {
        let r = cur?;
        let ps = objs.props_of(r)?;
        let p = objs.package(r.pkg);
        if objs.class_of(r).contains("MaterialInstance") {
            for e in struct_array(p, props::find(&ps, "TextureParameterValues")) {
                if let (Some(Value::Name(n)), Some(Value::Object(t))) = (props::find(&e, "ParameterName"), props::find(&e, "ParameterValue")) {
                    if let Some(tr) = objs.resolve_obj(r.pkg, *t) {
                        tex.entry(n.to_lowercase()).or_insert(tr);
                    }
                }
            }
            for e in struct_array(p, props::find(&ps, "VectorParameterValues")) {
                if let (Some(Value::Name(n)), Some(Value::LinearColor(c))) = (props::find(&e, "ParameterName"), props::find(&e, "ParameterValue")) {
                    vecs.entry(n.to_lowercase()).or_insert(*c);
                }
            }
            for e in struct_array(p, props::find(&ps, "ScalarParameterValues")) {
                if let (Some(Value::Name(n)), Some(Value::Float(f))) = (props::find(&e, "ParameterName"), props::find(&e, "ParameterValue")) {
                    scalars.entry(n.to_lowercase()).or_insert(*f);
                }
            }
            cur = match props::find(&ps, "Parent") {
                Some(Value::Object(o)) => objs.resolve_obj(r.pkg, *o),
                _ => None,
            };
            continue;
        }
        let Some(Value::Struct(_, inp)) = props::find(&ps, "DiffuseColor") else { return None };
        let g = Graph { objs, tex: &tex, vecs: &vecs, scalars: &scalars };
        return g.input(r.pkg, inp, 0);
    }
    None
}

struct Graph<'a> {
    objs: &'a dyn Objects,
    tex: &'a HashMap<String, ObjRef>,
    vecs: &'a HashMap<String, [f32; 4]>,
    scalars: &'a HashMap<String, f32>,
}

impl Graph<'_> {
    /// An ExpressionInput: its expression, narrowed to one channel by its Mask flags.
    fn input(&self, pkg: usize, inp: &[Prop], depth: u32) -> Option<Node> {
        let e = match props::find(inp, "Expression") {
            Some(Value::Object(o)) if *o != 0 => self.objs.resolve_obj(pkg, *o)?,
            _ => return None,
        };
        let node = self.expr(e, depth + 1)?;
        let on = |k: &str| matches!(props::find(inp, k), Some(Value::Int(i)) if *i != 0);
        if !on("Mask") {
            return Some(node);
        }
        let chans: Vec<usize> = ["MaskR", "MaskG", "MaskB", "MaskA"].iter().enumerate().filter(|(_, k)| on(k)).map(|(i, _)| i).collect();
        match chans.as_slice() {
            [c] => Some(Node::Channel(Box::new(node), *c)),
            [0, 1, 2] | [0, 1, 2, 3] => Some(node),
            _ => None,
        }
    }

    /// A binary input, or the expression's ConstA/ConstB when it isn't connected.
    fn operand(&self, pkg: usize, ps: &[Prop], name: &str, konst: &str, default: f32, depth: u32) -> Option<Node> {
        match props::find(ps, name) {
            Some(Value::Struct(_, s)) if matches!(props::find(s, "Expression"), Some(Value::Object(o)) if *o != 0) => self.input(pkg, s, depth),
            _ => {
                let v = match props::find(ps, konst) {
                    Some(Value::Float(f)) => *f,
                    _ => default,
                };
                Some(Node::Const([v, v, v, v]))
            }
        }
    }

    fn expr(&self, e: ObjRef, depth: u32) -> Option<Node> {
        if depth > 24 {
            return None;
        }
        let class = self.objs.class_of(e);
        let ps = self.objs.props_of(e)?;
        let f = |k: &str, d: f32| match props::find(&ps, k) {
            Some(Value::Float(x)) => *x,
            _ => d,
        };
        let pname = || match props::find(&ps, "ParameterName") {
            Some(Value::Name(n)) => n.to_lowercase(),
            _ => String::new(),
        };
        Some(match class.as_str() {
            "MaterialExpressionTextureSample" | "MaterialExpressionTextureSampleParameter2D" => {
                let t = self.tex.get(&pname()).copied().or(match props::find(&ps, "Texture") {
                    Some(Value::Object(o)) if *o != 0 => self.objs.resolve_obj(e.pkg, *o),
                    _ => None,
                })?;
                let mut tiling = [1.0, 1.0];
                if let Some(Value::Struct(_, c)) = props::find(&ps, "Coordinates") {
                    if let Some(Value::Object(o)) = props::find(c, "Expression") {
                        if *o != 0 {
                            let ce = self.objs.resolve_obj(e.pkg, *o)?;
                            if self.objs.class_of(ce) != "MaterialExpressionTextureCoordinate" {
                                return None;
                            }
                            let cp = self.objs.props_of(ce)?;
                            if matches!(props::find(&cp, "CoordinateIndex"), Some(Value::Int(i)) if *i != 0) {
                                return None;
                            }
                            let g = |k: &str| match props::find(&cp, k) {
                                Some(Value::Float(x)) => *x,
                                _ => 1.0,
                            };
                            tiling = [g("UTiling"), g("VTiling")];
                        }
                    }
                }
                Node::Tex { tex: t, tiling }
            }
            "MaterialExpressionConstant" => {
                let r = f("R", 0.0);
                Node::Const([r, r, r, r])
            }
            "MaterialExpressionConstant3Vector" => Node::Const([f("R", 0.0), f("G", 0.0), f("B", 0.0), 1.0]),
            "MaterialExpressionConstant4Vector" => Node::Const([f("R", 0.0), f("G", 0.0), f("B", 0.0), f("A", 1.0)]),
            "MaterialExpressionScalarParameter" => {
                let v = self.scalars.get(&pname()).copied().unwrap_or(f("DefaultValue", 0.0));
                Node::Const([v, v, v, v])
            }
            "MaterialExpressionVectorParameter" => {
                let v = self.vecs.get(&pname()).copied().or(match props::find(&ps, "DefaultValue") {
                    Some(Value::LinearColor(c)) => Some(*c),
                    _ => None,
                })?;
                Node::Const(v)
            }
            "MaterialExpressionMultiply" => Node::Mul(Box::new(self.operand(e.pkg, &ps, "A", "ConstA", 0.0, depth)?), Box::new(self.operand(e.pkg, &ps, "B", "ConstB", 1.0, depth)?)),
            "MaterialExpressionAdd" => Node::Add(Box::new(self.operand(e.pkg, &ps, "A", "ConstA", 0.0, depth)?), Box::new(self.operand(e.pkg, &ps, "B", "ConstB", 1.0, depth)?)),
            "MaterialExpressionOneMinus" => match props::find(&ps, "Input") {
                Some(Value::Struct(_, s)) => Node::OneMinus(Box::new(self.input(e.pkg, s, depth)?)),
                _ => return None,
            },
            "MaterialExpressionLinearInterpolate" => Node::Lerp(
                Box::new(self.operand(e.pkg, &ps, "A", "ConstA", 0.0, depth)?),
                Box::new(self.operand(e.pkg, &ps, "B", "ConstB", 1.0, depth)?),
                Box::new(self.operand(e.pkg, &ps, "Alpha", "ConstAlpha", 0.5, depth)?),
            ),
            _ => return None,
        })
    }
}
