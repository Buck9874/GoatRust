//! UE3 tagged property parsing (default properties, actor/component state).

use crate::package::{FName, Package};
use crate::reader::Reader;
use anyhow::{Result, bail};

#[derive(Debug, Clone)]
pub enum Value {
    Int(i32),
    Float(f32),
    Bool(bool),
    Byte(u8),
    Enum(String),
    Name(String),
    Str(String),
    Object(i32),
    Vector([f32; 3]),
    Rotator([i32; 3]),
    Color([u8; 4]),
    LinearColor([f32; 4]),
    Vector2D([f32; 2]),
    Quat([f32; 4]),
    Vector4([f32; 4]),
    Struct(String, Vec<Prop>),
    /// Array property: raw element bytes after the count.
    Array(i32, Vec<u8>),
    Raw(String, Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct Prop {
    pub name: String,
    pub ty: String,
    pub index: i32,
    pub value: Value,
}

fn is_property_type(s: &str) -> bool {
    matches!(
        s,
        "IntProperty"
            | "FloatProperty"
            | "BoolProperty"
            | "ByteProperty"
            | "NameProperty"
            | "StrProperty"
            | "ObjectProperty"
            | "ClassProperty"
            | "ComponentProperty"
            | "InterfaceProperty"
            | "DelegateProperty"
            | "StructProperty"
            | "ArrayProperty"
            | "MapProperty"
    )
}

fn read_fname(p: &Package, r: &mut Reader) -> Result<String> {
    let idx = r.i32()?;
    let num = r.i32()?;
    if idx < 0 || idx as usize >= p.names.len() {
        bail!("bad name index {idx}");
    }
    Ok(p.name_str(FName { index: idx, number: num }))
}

fn decode_struct(p: &Package, sname: &str, data: &[u8]) -> Value {
    let mut r = Reader::new(data);
    let v = match (sname, data.len()) {
        ("Vector", 12) => r.vec3().ok().map(Value::Vector),
        ("Rotator", 12) => (|| Ok::<_, anyhow::Error>(Value::Rotator([r.i32()?, r.i32()?, r.i32()?])))().ok(),
        ("Color", 4) => Some(Value::Color([data[0], data[1], data[2], data[3]])),
        ("LinearColor", 16) => (|| Ok::<_, anyhow::Error>(Value::LinearColor([r.f32()?, r.f32()?, r.f32()?, r.f32()?])))().ok(),
        ("Vector2D", 8) => (|| Ok::<_, anyhow::Error>(Value::Vector2D([r.f32()?, r.f32()?])))().ok(),
        ("Quat", 16) => (|| Ok::<_, anyhow::Error>(Value::Quat([r.f32()?, r.f32()?, r.f32()?, r.f32()?])))().ok(),
        ("Vector4", 16) | ("Plane", 16) => (|| Ok::<_, anyhow::Error>(Value::Vector4([r.f32()?, r.f32()?, r.f32()?, r.f32()?])))().ok(),
        _ => None,
    };
    if let Some(v) = v {
        return v;
    }
    let mut r = Reader::new(data);
    match parse_props(p, &mut r) {
        Ok(props) if r.pos == data.len() => Value::Struct(sname.to_string(), props),
        _ => Value::Raw(sname.to_string(), data.to_vec()),
    }
}

/// Parse a tagged property list until "None".
pub fn parse_props(p: &Package, r: &mut Reader) -> Result<Vec<Prop>> {
    let mut out = Vec::new();
    loop {
        let name = read_fname(p, r)?;
        if name == "None" {
            return Ok(out);
        }
        let ty = read_fname(p, r)?;
        if !is_property_type(&ty) {
            bail!("bad property type {ty} for {name}");
        }
        let size = r.i32()?;
        let index = r.i32()?;
        if size < 0 || size as usize > r.remaining() {
            bail!("bad property size {size} for {name}");
        }
        let value = match ty.as_str() {
            "BoolProperty" => Value::Bool(r.u8()? != 0),
            "StructProperty" => {
                let sname = read_fname(p, r)?;
                let d = r.bytes(size as usize)?;
                decode_struct(p, &sname, d)
            }
            "ByteProperty" => {
                let enum_name = read_fname(p, r)?;
                if size == 1 {
                    Value::Byte(r.u8()?)
                } else {
                    let v = read_fname(p, r)?;
                    let _ = enum_name;
                    Value::Enum(v)
                }
            }
            _ => {
                let d = r.bytes(size as usize)?;
                let mut vr = Reader::new(d);
                match ty.as_str() {
                    "IntProperty" => Value::Int(vr.i32()?),
                    "FloatProperty" => Value::Float(vr.f32()?),
                    "ObjectProperty" | "ClassProperty" | "ComponentProperty" | "InterfaceProperty" => {
                        Value::Object(vr.i32()?)
                    }
                    "NameProperty" => Value::Name(read_fname(p, &mut vr)?),
                    "StrProperty" => Value::Str(vr.fstring()?),
                    "ArrayProperty" => {
                        let n = vr.i32()?;
                        Value::Array(n, d[4..].to_vec())
                    }
                    _ => Value::Raw(ty.clone(), d.to_vec()),
                }
            }
        };
        out.push(Prop { name, ty, index, value });
    }
}

/// Locate and parse the property block of an export. UE3 objects start with optional
/// state-frame / component-template data and a NetIndex before the tagged properties, so
/// try the plausible start offsets and keep the first that parses cleanly.
pub fn export_props(p: &Package, export: usize) -> Result<(usize, Vec<Prop>)> {
    let data = p.export_data(export);
    let mut base = 0usize;
    if p.exports[export].object_flags & RF_HAS_STACK != 0 {
        base = skip_state_frame(data).unwrap_or(0);
    }
    let mut last_err = None;
    // NetIndex (+4), components add TemplateOwnerClass (+4) and maybe TemplateName (+8)
    for start in [4usize, 0, 8, 16, 12, 20, 24, 28, 32, 36, 40] {
        let start = base + start;
        if start + 8 > data.len() {
            break;
        }
        let mut r = Reader::at(data, start);
        match parse_props(p, &mut r) {
            Ok(props) => return Ok((r.pos, props)),
            Err(e) => last_err = Some(e),
        }
    }
    // Dominant(Directional|Spot)LightComponent serializes its whole-scene shadow depth map
    // (TArray<WORD> DominantLightShadowMap) ahead of the properties
    if data.len() >= 4 {
        let n = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
        let skip = 4 + n.saturating_mul(2);
        for start in [skip, skip + 4, skip + 8] {
            if start + 8 <= data.len() {
                let mut r = Reader::at(data, start);
                if let Ok(props) = parse_props(p, &mut r) {
                    return Ok((r.pos, props));
                }
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("export too small")))
}

pub const RF_HAS_STACK: u64 = 0x0200_0000_0000_0000;

/// FStateFrame (ver >= 691): Node, StateNode, u32 ProbeMask, u16 LatentAction,
/// TArray<{State, Node}> StateStack, i32 Offset if Node != 0.
fn skip_state_frame(data: &[u8]) -> Result<usize> {
    let mut r = Reader::new(data);
    let node = r.i32()?;
    r.i32()?;
    r.u32()?;
    r.u16()?;
    let n = r.i32()?;
    r.skip(n.max(0) as usize * 8)?;
    if node != 0 {
        r.i32()?;
    }
    Ok(r.pos)
}

pub fn find<'a>(props: &'a [Prop], name: &str) -> Option<&'a Value> {
    props.iter().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| &p.value)
}

pub fn fmt_value(p: &Package, v: &Value) -> String {
    match v {
        Value::Int(i) => i.to_string(),
        Value::Float(f) => format!("{f}"),
        Value::Bool(b) => b.to_string(),
        Value::Byte(b) => b.to_string(),
        Value::Enum(s) | Value::Name(s) => s.clone(),
        Value::Str(s) => format!("{s:?}"),
        Value::Object(o) => if *o == 0 { "None".into() } else { format!("{}'{}'", p.class_name(*o), p.obj_path(*o)) },
        Value::Vector(v) => format!("(X={},Y={},Z={})", v[0], v[1], v[2]),
        Value::Rotator(v) => format!("(Pitch={},Yaw={},Roll={})", v[0], v[1], v[2]),
        Value::Color(c) => format!("(B={},G={},R={},A={})", c[0], c[1], c[2], c[3]),
        Value::LinearColor(c) => format!("(R={},G={},B={},A={})", c[0], c[1], c[2], c[3]),
        Value::Vector2D(v) => format!("(X={},Y={})", v[0], v[1]),
        Value::Quat(v) | Value::Vector4(v) => format!("(X={},Y={},Z={},W={})", v[0], v[1], v[2], v[3]),
        Value::Struct(n, props) => {
            let inner: Vec<String> = props.iter().map(|q| format!("{}={}", q.name, fmt_value(p, &q.value))).collect();
            format!("{n}({})", inner.join(","))
        }
        Value::Array(n, d) => format!("[{n} items, {} bytes]", d.len()),
        Value::Raw(t, d) => format!("<{t} {} bytes>", d.len()),
    }
}
