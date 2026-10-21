//! Compiled UnrealScript: UStruct/UFunction/UState/UClass and UProperty exports and the
//! UE3 (v868) bytecode stream inside them.
//!
//! On disk an object reference is a 4-byte package index and an FName 8 bytes; in memory
//! (the 64-bit game) object references are 8 bytes, and every jump/skip offset in the
//! bytecode counts memory bytes. The decoder tracks both so jump targets resolve.

use crate::package::{FName, Package};
use crate::props;
use crate::reader::Reader;
use anyhow::{Result, bail};

/// Bytes an object reference takes in the game's in-memory bytecode.
pub const MEM_PTR: u32 = 8;

pub const FUNC_FINAL: u32 = 0x1;
pub const FUNC_DEFINED: u32 = 0x2;
pub const FUNC_ITERATOR: u32 = 0x4;
pub const FUNC_LATENT: u32 = 0x8;
pub const FUNC_PRE_OPERATOR: u32 = 0x10;
pub const FUNC_SINGULAR: u32 = 0x20;
pub const FUNC_NET: u32 = 0x40;
pub const FUNC_SIMULATED: u32 = 0x100;
pub const FUNC_EXEC: u32 = 0x200;
pub const FUNC_NATIVE: u32 = 0x400;
pub const FUNC_EVENT: u32 = 0x800;
pub const FUNC_OPERATOR: u32 = 0x1000;
pub const FUNC_STATIC: u32 = 0x2000;
pub const FUNC_HAS_OPTIONAL: u32 = 0x4000;
pub const FUNC_DELEGATE: u32 = 0x10_0000;

pub const CPF_OPTIONAL_PARM: u64 = 0x10;
pub const CPF_PARM: u64 = 0x80;
pub const CPF_OUT_PARM: u64 = 0x100;
pub const CPF_SKIP_PARM: u64 = 0x200;
pub const CPF_RETURN_PARM: u64 = 0x400;
pub const CPF_COERCE_PARM: u64 = 0x800;
pub const CPF_NET: u64 = 0x20;
pub const CPF_CONST: u64 = 0x2;

#[derive(Debug, Clone, Default)]
pub struct FuncInfo {
    pub native: u16,
    pub precedence: u8,
    pub flags: u32,
    pub friendly_name: String,
}

#[derive(Debug, Clone, Default)]
pub struct StateInfo {
    pub probe_mask: u32,
    pub label_table_offset: u16,
    pub state_flags: u32,
    pub func_map: Vec<(String, i32)>,
}

/// A UStruct-derived export (ScriptStruct, Function, State, Class).
#[derive(Debug, Clone, Default)]
pub struct ScriptStruct {
    pub kind: String,
    pub next: i32,
    pub super_: i32,
    pub children: i32,
    pub line: i32,
    pub text_pos: i32,
    pub mem_size: u32,
    pub script: Vec<u8>,
    pub func: Option<FuncInfo>,
    pub state: Option<StateInfo>,
    /// UClass: ClassFlags, ClassWithin, ClassConfigName.
    pub class_flags: u32,
    pub class_within: i32,
    pub config_name: String,
    /// UScriptStruct: StructFlags.
    pub struct_flags: u32,
    /// UClass: implemented interface classes (package indices).
    pub interfaces: Vec<i32>,
    /// Byte offset after the UStruct part (subclass data follows).
    pub tail: usize,
}

/// A UProperty export.
#[derive(Debug, Clone, Default)]
pub struct PropInfo {
    pub name: String,
    pub kind: String,
    pub next: i32,
    pub array_dim: i32,
    pub flags: u64,
    pub category: String,
    /// Kind-specific references: Byte -> [Enum], Object/Component/Interface -> [Class],
    /// Class -> [Class, MetaClass], Struct -> [Struct], Array -> [Inner], Map -> [Key, Value],
    /// Delegate -> [Function, SourceDelegate].
    pub refs: Vec<i32>,
}

fn fname(p: &Package, r: &mut Reader) -> Result<String> {
    let index = r.i32()?;
    let number = r.i32()?;
    if index < 0 || index as usize >= p.names.len() {
        bail!("bad name index {index} at {}", r.pos - 8);
    }
    Ok(p.name_str(FName { index, number }))
}

/// Skip NetIndex + the (empty) tagged property list that precede UField data.
fn object_header(p: &Package, data: &[u8]) -> Result<usize> {
    let mut r = Reader::at(data, 4);
    props::parse_props(p, &mut r)?;
    Ok(r.pos)
}

pub fn is_struct_kind(kind: &str) -> bool {
    matches!(kind, "Function" | "State" | "Class" | "ScriptStruct")
}

pub fn read_struct(p: &Package, export: usize) -> Result<ScriptStruct> {
    let kind = p.class_name(export as i32 + 1);
    if !is_struct_kind(&kind) {
        bail!("{} is a {kind}, not a struct", p.obj_path(export as i32 + 1));
    }
    let data = p.export_data(export);
    // UClass skips the script property block: only NetIndex precedes its UField data
    let start = if kind == "Class" { 4 } else { object_header(p, data)? };
    let mut r = Reader::at(data, start);
    let mut s = ScriptStruct { kind: kind.clone(), ..Default::default() };
    s.next = r.i32()?;
    s.super_ = r.i32()?;
    let _script_text = r.i32()?;
    s.children = r.i32()?;
    let _cpp_text = r.i32()?;
    s.line = r.i32()?;
    s.text_pos = r.i32()?;
    s.mem_size = r.u32()?;
    let disk = r.i32()?;
    if disk < 0 {
        bail!("bad script size {disk}");
    }
    s.script = r.bytes(disk as usize)?.to_vec();
    s.tail = r.pos;
    match kind.as_str() {
        "Function" => {
            let native = r.u16()?;
            let precedence = r.u8()?;
            let flags = r.u32()?;
            if flags & FUNC_NET != 0 {
                r.u16()?;
            }
            let friendly_name = fname(p, &mut r)?;
            s.func = Some(FuncInfo { native, precedence, flags, friendly_name });
        }
        "ScriptStruct" => s.struct_flags = r.u32()?,
        "State" | "Class" => {
            let probe_mask = r.u32()?;
            let label_table_offset = r.u16()?;
            let state_flags = r.u32()?;
            let n = r.i32()?;
            let mut func_map = Vec::new();
            for _ in 0..n.max(0) {
                func_map.push((fname(p, &mut r)?, r.i32()?));
            }
            s.state = Some(StateInfo { probe_mask, label_table_offset, state_flags, func_map });
            if kind == "Class" {
                s.class_flags = r.u32()?;
                s.class_within = r.i32()?;
                s.config_name = fname(p, &mut r)?;
                // ComponentNameToDefaultObjectMap, then Interfaces {Class, PointerProperty}
                let tail = (|| -> Result<Vec<i32>> {
                    let n = r.i32()?;
                    for _ in 0..n.max(0) {
                        fname(p, &mut r)?;
                        r.i32()?;
                    }
                    let n = r.i32()?;
                    let mut v = vec![];
                    for _ in 0..n.max(0) {
                        v.push(r.i32()?);
                        r.i32()?;
                    }
                    Ok(v)
                })();
                s.interfaces = tail.unwrap_or_default();
            }
        }
        _ => {}
    }
    Ok(s)
}

pub fn is_property_kind(kind: &str) -> bool {
    kind.ends_with("Property")
}

pub fn read_property(p: &Package, export: usize) -> Result<PropInfo> {
    let kind = p.class_name(export as i32 + 1);
    if !is_property_kind(&kind) {
        bail!("{} is a {kind}, not a property", p.obj_path(export as i32 + 1));
    }
    let data = p.export_data(export);
    let mut r = Reader::at(data, object_header(p, data)?);
    let mut q = PropInfo { name: p.obj_name(export as i32 + 1), kind: kind.clone(), ..Default::default() };
    q.next = r.i32()?;
    q.array_dim = r.i32()?;
    q.flags = r.u64()?;
    q.category = fname(p, &mut r)?;
    let _array_size_enum = r.i32()?;
    if q.flags & CPF_NET != 0 {
        r.u16()?;
    }
    let nrefs = match kind.as_str() {
        "ByteProperty" | "ObjectProperty" | "ComponentProperty" | "InterfaceProperty" | "StructProperty"
        | "ArrayProperty" => 1,
        "ClassProperty" | "MapProperty" | "DelegateProperty" => 2,
        _ => 0,
    };
    for _ in 0..nrefs {
        if r.remaining() >= 4 {
            q.refs.push(r.i32()?);
        }
    }
    Ok(q)
}

/// Walk a Children/Next chain of fields.
pub fn children(p: &Package, first: i32) -> Vec<i32> {
    let mut out = Vec::new();
    let mut cur = first;
    while cur > 0 && out.len() < 10000 {
        out.push(cur);
        let idx = cur as usize - 1;
        let kind = p.class_name(cur);
        cur = if is_property_kind(&kind) {
            read_property(p, idx).map(|q| q.next).unwrap_or(0)
        } else {
            // every UField starts its own data with Next
            let data = p.export_data(idx);
            let start = if kind == "Class" { Some(4) } else { object_header(p, data).ok() };
            start.and_then(|o| Reader::at(data, o).i32().ok()).unwrap_or(0)
        };
    }
    out
}

// ---------------------------------------------------------------------------
// Bytecode

/// One decoded expression. Object operands are package indices of the function's package.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    LocalVariable(i32),
    InstanceVariable(i32),
    DefaultVariable(i32),
    StateVariable(i32),
    LocalOutVariable(i32),
    NativeParm(i32),
    Return(Box<Expr>),
    ReturnNothing(i32),
    Switch { prop: i32, size: u8, expr: Box<Expr> },
    /// `None` = default:
    Case { next: u16, expr: Option<Box<Expr>> },
    Jump(u16),
    JumpIfNot(u16, Box<Expr>),
    Stop,
    Assert { line: u16, debug: u8, expr: Box<Expr> },
    Nothing,
    LabelTable(Vec<(String, u32)>),
    GotoLabel(Box<Expr>),
    EatReturnValue(i32, Box<Expr>),
    Let(Box<Expr>, Box<Expr>),
    LetBool(Box<Expr>, Box<Expr>),
    LetDelegate(Box<Expr>, Box<Expr>),
    /// (index, array)
    DynArrayElement(Box<Expr>, Box<Expr>),
    ArrayElement(Box<Expr>, Box<Expr>),
    New { outer: Box<Expr>, name: Box<Expr>, flags: Box<Expr>, class: Box<Expr>, template: Box<Expr> },
    ClassContext { obj: Box<Expr>, skip: u16, prop: i32, size: u8, expr: Box<Expr> },
    Context { obj: Box<Expr>, skip: u16, prop: i32, size: u8, expr: Box<Expr> },
    InterfaceContext(Box<Expr>),
    MetaCast(i32, Box<Expr>),
    DynamicCast(i32, Box<Expr>),
    InterfaceCast(i32, Box<Expr>),
    PrimitiveCast(u8, Box<Expr>),
    EndParmValue,
    EndFunctionParms,
    SelfObj,
    Skip(u16, Box<Expr>),
    VirtualFunction(String, Vec<Expr>),
    FinalFunction(i32, Vec<Expr>),
    GlobalFunction(String, Vec<Expr>),
    NativeFunction(u16, Vec<Expr>),
    DelegateFunction { local: u8, prop: i32, name: String, args: Vec<Expr> },
    IntConst(i32),
    FloatConst(f32),
    StringConst(String),
    UnicodeStringConst(String),
    ObjectConst(i32),
    NameConst(String),
    RotationConst([i32; 3]),
    VectorConst([f32; 3]),
    ByteConst(u8),
    IntConstByte(u8),
    IntZero,
    IntOne,
    True,
    False,
    NoObject,
    EmptyDelegate,
    BoolVariable(Box<Expr>),
    Iterator(Box<Expr>, u16),
    IteratorPop,
    IteratorNext,
    StructCmpEq(i32, Box<Expr>, Box<Expr>),
    StructCmpNe(i32, Box<Expr>, Box<Expr>),
    StructMember { prop: i32, owner: i32, copy: u8, modify: u8, expr: Box<Expr> },
    DynArrayLength(Box<Expr>),
    DynArrayInsert(Box<Expr>, Box<Expr>, Box<Expr>),
    DynArrayRemove(Box<Expr>, Box<Expr>, Box<Expr>),
    DynArrayFind { arr: Box<Expr>, skip: u16, value: Box<Expr> },
    DynArrayFindStruct { arr: Box<Expr>, skip: u16, member: Box<Expr>, value: Box<Expr> },
    DynArrayAdd { arr: Box<Expr>, count: Box<Expr> },
    DynArrayAddItem { arr: Box<Expr>, skip: u16, item: Box<Expr> },
    DynArrayRemoveItem { arr: Box<Expr>, skip: u16, item: Box<Expr> },
    DynArrayInsertItem { arr: Box<Expr>, skip: u16, index: Box<Expr>, item: Box<Expr> },
    DynArrayIterator { arr: Box<Expr>, item: Box<Expr>, has_index: u8, index: Box<Expr>, skip: u16 },
    DynArraySort { arr: Box<Expr>, skip: u16, delegate: Box<Expr> },
    EqualEqualDelDel(Box<Expr>, Box<Expr>),
    NotEqualDelDel(Box<Expr>, Box<Expr>),
    EqualEqualDelFunc(Box<Expr>, Box<Expr>),
    NotEqualDelFunc(Box<Expr>, Box<Expr>),
    DelegateProperty(String, i32),
    InstanceDelegate(String),
    Conditional { cond: Box<Expr>, skip_a: u16, a: Box<Expr>, skip_b: u16, b: Box<Expr> },
    DefaultParmValue(u16, Box<Expr>),
    EmptyParmValue,
    FilterEditorOnly(u16),
    EndOfScript,
}

/// A top-level statement and its offset in the in-memory bytecode (jump targets).
#[derive(Debug, Clone)]
pub struct Stmt {
    pub mem: u32,
    pub disk: u32,
    pub expr: Expr,
}

struct Decoder<'a> {
    p: &'a Package,
    r: Reader<'a>,
    mem: u32,
}

impl Decoder<'_> {
    fn u8(&mut self) -> Result<u8> {
        self.mem += 1;
        self.r.u8()
    }
    fn u16(&mut self) -> Result<u16> {
        self.mem += 2;
        self.r.u16()
    }
    fn i32(&mut self) -> Result<i32> {
        self.mem += 4;
        self.r.i32()
    }
    fn f32(&mut self) -> Result<f32> {
        self.mem += 4;
        self.r.f32()
    }
    fn obj(&mut self) -> Result<i32> {
        self.mem += MEM_PTR;
        self.r.i32()
    }
    fn name(&mut self) -> Result<String> {
        self.mem += 8;
        fname(self.p, &mut self.r)
    }
    fn b(&mut self) -> Result<Box<Expr>> {
        Ok(Box::new(self.expr()?))
    }
    /// Arguments up to and including EX_EndFunctionParms.
    fn args(&mut self) -> Result<Vec<Expr>> {
        let mut v = Vec::new();
        loop {
            let e = self.expr()?;
            if e == Expr::EndFunctionParms {
                return Ok(v);
            }
            if v.len() > 64 {
                bail!("runaway argument list");
            }
            v.push(e);
        }
    }
    fn end_parms(&mut self) -> Result<()> {
        let e = self.expr()?;
        if e != Expr::EndFunctionParms {
            bail!("expected EndFunctionParms, got {e:?}");
        }
        Ok(())
    }

    fn expr(&mut self) -> Result<Expr> {
        use Expr::*;
        let at = self.r.pos;
        let op = self.u8()?;
        Ok(match op {
            0x00 => LocalVariable(self.obj()?),
            0x01 => InstanceVariable(self.obj()?),
            0x02 => DefaultVariable(self.obj()?),
            0x03 => StateVariable(self.obj()?),
            0x04 => Return(self.b()?),
            0x05 => {
                let prop = self.obj()?;
                let size = self.u8()?;
                Switch { prop, size, expr: self.b()? }
            }
            0x06 => Jump(self.u16()?),
            0x07 => {
                let off = self.u16()?;
                JumpIfNot(off, self.b()?)
            }
            0x08 => Stop,
            0x09 => {
                let line = self.u16()?;
                let debug = self.u8()?;
                Assert { line, debug, expr: self.b()? }
            }
            0x0A => {
                let next = self.u16()?;
                if next == 0xFFFF { Case { next, expr: None } } else { Case { next, expr: Some(self.b()?) } }
            }
            0x0B => Nothing,
            0x0C => {
                let mut v = Vec::new();
                loop {
                    let n = self.name()?;
                    let off = self.i32()? as u32;
                    if n == "None" {
                        break;
                    }
                    v.push((n, off));
                }
                LabelTable(v)
            }
            0x0D => GotoLabel(self.b()?),
            0x0E => {
                let prop = self.obj()?;
                EatReturnValue(prop, self.b()?)
            }
            0x0F => Let(self.b()?, self.b()?),
            0x10 => DynArrayElement(self.b()?, self.b()?),
            0x11 => New { outer: self.b()?, name: self.b()?, flags: self.b()?, class: self.b()?, template: self.b()? },
            0x12 | 0x19 => {
                let obj = self.b()?;
                let skip = self.u16()?;
                let prop = self.obj()?;
                let size = self.u8()?;
                let expr = self.b()?;
                if op == 0x12 { ClassContext { obj, skip, prop, size, expr } } else { Context { obj, skip, prop, size, expr } }
            }
            0x13 => {
                let c = self.obj()?;
                MetaCast(c, self.b()?)
            }
            0x14 => LetBool(self.b()?, self.b()?),
            0x15 => EndParmValue,
            0x16 => EndFunctionParms,
            0x17 => SelfObj,
            0x18 => {
                let s = self.u16()?;
                Skip(s, self.b()?)
            }
            0x1A => ArrayElement(self.b()?, self.b()?),
            0x1B => {
                let n = self.name()?;
                VirtualFunction(n, self.args()?)
            }
            0x1C => {
                let f = self.obj()?;
                FinalFunction(f, self.args()?)
            }
            0x1D => IntConst(self.i32()?),
            0x1E => FloatConst(self.f32()?),
            0x1F => {
                let mut s = String::new();
                loop {
                    let c = self.u8()?;
                    if c == 0 {
                        break;
                    }
                    s.push(c as char);
                }
                StringConst(s)
            }
            0x20 => ObjectConst(self.obj()?),
            0x21 => NameConst(self.name()?),
            0x22 => RotationConst([self.i32()?, self.i32()?, self.i32()?]),
            0x23 => VectorConst([self.f32()?, self.f32()?, self.f32()?]),
            0x24 => ByteConst(self.u8()?),
            0x25 => IntZero,
            0x26 => IntOne,
            0x27 => True,
            0x28 => False,
            0x29 => NativeParm(self.obj()?),
            0x2A => NoObject,
            0x2C => IntConstByte(self.u8()?),
            0x2D => BoolVariable(self.b()?),
            0x2E => {
                let c = self.obj()?;
                DynamicCast(c, self.b()?)
            }
            0x2F => {
                let e = self.b()?;
                Iterator(e, self.u16()?)
            }
            0x30 => IteratorPop,
            0x31 => IteratorNext,
            0x32 | 0x33 => {
                let s = self.obj()?;
                let a = self.b()?;
                let b = self.b()?;
                if op == 0x32 { StructCmpEq(s, a, b) } else { StructCmpNe(s, a, b) }
            }
            0x34 => {
                let mut v = Vec::new();
                loop {
                    let c = self.u16()?;
                    if c == 0 {
                        break;
                    }
                    v.push(c);
                }
                UnicodeStringConst(String::from_utf16_lossy(&v))
            }
            0x35 => {
                let prop = self.obj()?;
                let owner = self.obj()?;
                let copy = self.u8()?;
                let modify = self.u8()?;
                StructMember { prop, owner, copy, modify, expr: self.b()? }
            }
            0x36 => DynArrayLength(self.b()?),
            0x37 => {
                let n = self.name()?;
                GlobalFunction(n, self.args()?)
            }
            0x38 => {
                let t = self.u8()?;
                PrimitiveCast(t, self.b()?)
            }
            0x39 => {
                let (a, b, c) = (self.b()?, self.b()?, self.b()?);
                self.end_parms()?;
                DynArrayInsert(a, b, c)
            }
            0x3A => ReturnNothing(self.obj()?),
            0x3B..=0x3E => {
                let (a, b) = (self.b()?, self.b()?);
                self.end_parms()?;
                match op {
                    0x3B => EqualEqualDelDel(a, b),
                    0x3C => NotEqualDelDel(a, b),
                    0x3D => EqualEqualDelFunc(a, b),
                    _ => NotEqualDelFunc(a, b),
                }
            }
            0x3F => EmptyDelegate,
            0x40 => {
                let (a, b, c) = (self.b()?, self.b()?, self.b()?);
                self.end_parms()?;
                DynArrayRemove(a, b, c)
            }
            0x42 => {
                let local = self.u8()?;
                let prop = self.obj()?;
                let name = self.name()?;
                DelegateFunction { local, prop, name, args: self.args()? }
            }
            0x43 => {
                let n = self.name()?;
                DelegateProperty(n, self.obj()?)
            }
            0x44 => LetDelegate(self.b()?, self.b()?),
            0x45 => {
                let cond = self.b()?;
                let skip_a = self.u16()?;
                let a = self.b()?;
                let skip_b = self.u16()?;
                Conditional { cond, skip_a, a, skip_b, b: self.b()? }
            }
            0x46 => {
                let arr = self.b()?;
                let skip = self.u16()?;
                let value = self.b()?;
                self.end_parms()?;
                DynArrayFind { arr, skip, value }
            }
            0x47 => {
                let arr = self.b()?;
                let skip = self.u16()?;
                let member = self.b()?;
                let value = self.b()?;
                self.end_parms()?;
                DynArrayFindStruct { arr, skip, member, value }
            }
            0x48 => LocalOutVariable(self.obj()?),
            0x49 => {
                let size = self.u16()?;
                let e = self.b()?;
                let end = self.expr()?;
                if end != EndParmValue {
                    bail!("DefaultParmValue not closed: {end:?}");
                }
                DefaultParmValue(size, e)
            }
            0x4A => EmptyParmValue,
            0x4B => InstanceDelegate(self.name()?),
            0x51 => InterfaceContext(self.b()?),
            0x52 => {
                let c = self.obj()?;
                InterfaceCast(c, self.b()?)
            }
            0x53 => EndOfScript,
            0x54 => {
                let arr = self.b()?;
                let count = self.b()?;
                self.end_parms()?;
                DynArrayAdd { arr, count }
            }
            0x55..=0x57 | 0x59 => {
                let arr = self.b()?;
                let skip = self.u16()?;
                let r = match op {
                    0x55 => DynArrayAddItem { arr, skip, item: self.b()? },
                    0x56 => DynArrayRemoveItem { arr, skip, item: self.b()? },
                    0x57 => {
                        let index = self.b()?;
                        DynArrayInsertItem { arr, skip, index, item: self.b()? }
                    }
                    _ => DynArraySort { arr, skip, delegate: self.b()? },
                };
                self.end_parms()?;
                r
            }
            0x58 => {
                let arr = self.b()?;
                let item = self.b()?;
                let has_index = self.u8()?;
                let index = self.b()?;
                DynArrayIterator { arr, item, has_index, index, skip: self.u16()? }
            }
            0x5A => FilterEditorOnly(self.u16()?),
            0x60..=0x6F => {
                let lo = self.u8()?;
                NativeFunction((((op - 0x60) as u16) << 8) | lo as u16, self.args()?)
            }
            0x70..=0xFF => NativeFunction(op as u16, self.args()?),
            _ => bail!("unknown opcode 0x{op:02x} at disk offset {at}"),
        })
    }
}

/// Decode a whole script into statements. Errors carry the offset of the bad token.
pub fn decode(p: &Package, script: &[u8]) -> Result<Vec<Stmt>> {
    match decode_partial(p, script) {
        (v, None) => Ok(v),
        (_, Some(e)) => Err(e),
    }
}

/// Statements up to the first undecodable one, plus the error if any.
pub fn decode_partial(p: &Package, script: &[u8]) -> (Vec<Stmt>, Option<anyhow::Error>) {
    let mut d = Decoder { p, r: Reader::new(script), mem: 0 };
    let mut out = Vec::new();
    while d.r.remaining() > 0 {
        let (mem, disk) = (d.mem, d.r.pos as u32);
        let expr = match d.expr() {
            Ok(e) => e,
            Err(e) => return (out, Some(anyhow::anyhow!("{e} (statement at mem {mem:#x}, disk {disk:#x})"))),
        };
        let end = expr == Expr::EndOfScript;
        out.push(Stmt { mem, disk, expr });
        if end {
            break;
        }
    }
    (out, None)
}

/// Decode and check that the memory size matches what the game recorded.
pub fn decode_struct(p: &Package, s: &ScriptStruct) -> Result<Vec<Stmt>> {
    let stmts = decode(p, &s.script)?;
    let mem = {
        let mut d = Decoder { p, r: Reader::new(&s.script), mem: 0 };
        while d.r.remaining() > 0 && d.expr()? != Expr::EndOfScript {}
        d.mem
    };
    if mem != s.mem_size {
        bail!("memory size mismatch: decoded {mem} vs recorded {}", s.mem_size);
    }
    Ok(stmts)
}

// ---------------------------------------------------------------------------
// Disassembly text

/// UE3 ECastToken names (EX_PrimitiveCast operand).
pub fn cast_name(t: u8) -> &'static str {
    match t {
        0x36 => "InterfaceToObject",
        0x37 => "InterfaceToString",
        0x38 => "InterfaceToBool",
        0x39 => "RotatorToVector",
        0x3A => "ByteToInt",
        0x3B => "ByteToBool",
        0x3C => "ByteToFloat",
        0x3D => "IntToByte",
        0x3E => "IntToBool",
        0x3F => "IntToFloat",
        0x40 => "BoolToByte",
        0x41 => "BoolToInt",
        0x42 => "BoolToFloat",
        0x43 => "FloatToByte",
        0x44 => "FloatToInt",
        0x45 => "FloatToBool",
        0x46 => "ObjectToInterface",
        0x47 => "ObjectToBool",
        0x48 => "NameToBool",
        0x49 => "StringToByte",
        0x4A => "StringToInt",
        0x4B => "StringToBool",
        0x4C => "StringToFloat",
        0x4D => "StringToVector",
        0x4E => "StringToRotator",
        0x4F => "VectorToBool",
        0x50 => "VectorToRotator",
        0x51 => "RotatorToBool",
        0x52 => "ByteToString",
        0x53 => "IntToString",
        0x54 => "BoolToString",
        0x55 => "FloatToString",
        0x56 => "ObjectToString",
        0x57 => "NameToString",
        0x58 => "VectorToString",
        0x59 => "RotatorToString",
        0x5A => "DelegateToString",
        0x5B => "StringToDelegate",
        0x60 => "StringToName",
        _ => "Cast?",
    }
}

/// Formats expressions with names resolved; `native` names indexed natives.
pub struct Fmt<'a> {
    pub p: &'a Package,
    pub native: &'a dyn Fn(u16) -> Option<String>,
}

impl Fmt<'_> {
    fn o(&self, i: i32) -> String {
        if i == 0 { "None".into() } else { self.p.obj_name(i) }
    }
    fn args(&self, a: &[Expr]) -> String {
        a.iter().map(|e| self.e(e)).collect::<Vec<_>>().join(", ")
    }
    pub fn e(&self, e: &Expr) -> String {
        use Expr::*;
        match e {
            LocalVariable(o) => self.o(*o),
            InstanceVariable(o) => format!("self.{}", self.o(*o)),
            DefaultVariable(o) => format!("default.{}", self.o(*o)),
            StateVariable(o) => format!("state.{}", self.o(*o)),
            LocalOutVariable(o) => format!("out {}", self.o(*o)),
            NativeParm(o) => format!("parm {}", self.o(*o)),
            Return(x) => format!("return {}", self.e(x)),
            ReturnNothing(o) => format!("return /*nothing, {}*/", self.o(*o)),
            Switch { expr, size, .. } => format!("switch ({}) /*size {size}*/", self.e(expr)),
            Case { next, expr: None } => format!("default: /*next {next:#x}*/"),
            Case { next, expr: Some(x) } => format!("case {}: /*next {next:#x}*/", self.e(x)),
            Jump(o) => format!("goto {o:#x}"),
            JumpIfNot(o, x) => format!("if !({}) goto {o:#x}", self.e(x)),
            Stop => "stop".into(),
            Assert { line, expr, .. } => format!("assert({}) /*line {line}*/", self.e(expr)),
            Nothing => "nothing".into(),
            LabelTable(v) => format!("labels {}", v.iter().map(|(n, o)| format!("{n}@{o:#x}")).collect::<Vec<_>>().join(" ")),
            GotoLabel(x) => format!("goto label {}", self.e(x)),
            EatReturnValue(_, x) => self.e(x),
            Let(a, b) | LetBool(a, b) | LetDelegate(a, b) => format!("{} = {}", self.e(a), self.e(b)),
            DynArrayElement(i, a) | ArrayElement(i, a) => format!("{}[{}]", self.e(a), self.e(i)),
            New { outer, name, flags, class, template } => {
                let mut s = String::from("new");
                if !matches!((&**outer, &**name, &**flags), (Nothing, Nothing, Nothing)) {
                    s += &format!("({}, {}, {})", self.e(outer), self.e(name), self.e(flags));
                }
                s += &format!(" {}", self.e(class));
                if **template != Nothing {
                    s += &format!("({})", self.e(template));
                }
                s
            }
            ClassContext { obj, expr, .. } => format!("{}.static.{}", self.e(obj), self.e(expr)),
            Context { obj, expr, .. } => format!("{}.{}", self.e(obj), self.e(expr).trim_start_matches("self.")),
            InterfaceContext(x) => self.e(x),
            MetaCast(c, x) => format!("class<{}>({})", self.o(*c), self.e(x)),
            DynamicCast(c, x) => format!("{}({})", self.o(*c), self.e(x)),
            InterfaceCast(c, x) => format!("{}({})", self.o(*c), self.e(x)),
            PrimitiveCast(t, x) => format!("{}({})", cast_name(*t), self.e(x)),
            EndParmValue => "endparmvalue".into(),
            EndFunctionParms => ")".into(),
            SelfObj => "self".into(),
            Skip(_, x) => self.e(x),
            VirtualFunction(n, a) | GlobalFunction(n, a) => {
                let g = if matches!(e, GlobalFunction(..)) { "global." } else { "" };
                format!("{g}{n}({})", self.args(a))
            }
            FinalFunction(f, a) => format!("{}.{}({})", self.o(self.p.obj_outer(*f)), self.o(*f), self.args(a)),
            NativeFunction(i, a) => {
                let n = (self.native)(*i).unwrap_or_else(|| format!("native{i}"));
                format!("{n}({})", self.args(a))
            }
            DelegateFunction { name, args, .. } => format!("{name}({})", self.args(args)),
            IntConst(i) => i.to_string(),
            FloatConst(f) => format!("{f:?}"),
            StringConst(s) | UnicodeStringConst(s) => format!("{s:?}"),
            ObjectConst(o) => format!("{}'{}'", self.p.class_name(*o), self.p.obj_path(*o)),
            NameConst(n) => format!("'{n}'"),
            RotationConst(r) => format!("rot({}, {}, {})", r[0], r[1], r[2]),
            VectorConst(v) => format!("vect({:?}, {:?}, {:?})", v[0], v[1], v[2]),
            ByteConst(b) | IntConstByte(b) => b.to_string(),
            IntZero => "0".into(),
            IntOne => "1".into(),
            True => "true".into(),
            False => "false".into(),
            NoObject => "none".into(),
            EmptyDelegate => "none /*delegate*/".into(),
            BoolVariable(x) => self.e(x),
            Iterator(x, s) => format!("foreach {} /*end {s:#x}*/", self.e(x)),
            IteratorPop => "iterator pop".into(),
            IteratorNext => "iterator next".into(),
            StructCmpEq(_, a, b) => format!("{} == {}", self.e(a), self.e(b)),
            StructCmpNe(_, a, b) => format!("{} != {}", self.e(a), self.e(b)),
            StructMember { prop, expr, .. } => format!("{}.{}", self.e(expr), self.o(*prop)),
            DynArrayLength(x) => format!("{}.Length", self.e(x)),
            DynArrayInsert(a, i, n) => format!("{}.Insert({}, {})", self.e(a), self.e(i), self.e(n)),
            DynArrayRemove(a, i, n) => format!("{}.Remove({}, {})", self.e(a), self.e(i), self.e(n)),
            DynArrayFind { arr, value, .. } => format!("{}.Find({})", self.e(arr), self.e(value)),
            DynArrayFindStruct { arr, member, value, .. } => {
                format!("{}.Find({}, {})", self.e(arr), self.e(member), self.e(value))
            }
            DynArrayAdd { arr, count, .. } => format!("{}.Add({})", self.e(arr), self.e(count)),
            DynArrayAddItem { arr, item, .. } => format!("{}.AddItem({})", self.e(arr), self.e(item)),
            DynArrayRemoveItem { arr, item, .. } => format!("{}.RemoveItem({})", self.e(arr), self.e(item)),
            DynArrayInsertItem { arr, index, item, .. } => {
                format!("{}.InsertItem({}, {})", self.e(arr), self.e(index), self.e(item))
            }
            DynArrayIterator { arr, item, has_index, index, skip } => {
                let idx = if *has_index != 0 { format!(", {}", self.e(index)) } else { String::new() };
                format!("foreach {}({}{idx}) /*end {skip:#x}*/", self.e(arr), self.e(item))
            }
            DynArraySort { arr, delegate, .. } => format!("{}.Sort({})", self.e(arr), self.e(delegate)),
            EqualEqualDelDel(a, b) | EqualEqualDelFunc(a, b) => format!("{} == {}", self.e(a), self.e(b)),
            NotEqualDelDel(a, b) | NotEqualDelFunc(a, b) => format!("{} != {}", self.e(a), self.e(b)),
            DelegateProperty(n, _) | InstanceDelegate(n) => n.clone(),
            Conditional { cond, a, b, .. } => format!("({} ? {} : {})", self.e(cond), self.e(a), self.e(b)),
            DefaultParmValue(_, x) => format!("default parm = {}", self.e(x)),
            EmptyParmValue => "/*omitted*/".into(),
            FilterEditorOnly(s) => format!("if (!editor) goto +{s:#x}"),
            EndOfScript => "end of script".into(),
        }
    }
}

/// Indexed natives (`native(N)`) declared in a package: index -> display name.
pub fn native_names(p: &Package, out: &mut std::collections::HashMap<u16, String>) {
    for i in 0..p.exports.len() {
        if p.class_name(i as i32 + 1) != "Function" {
            continue;
        }
        let Ok(s) = read_struct(p, i) else { continue };
        let Some(f) = &s.func else { continue };
        if f.native == 0 {
            continue;
        }
        let name = p.obj_name(i as i32 + 1);
        let shown = if f.flags & (FUNC_OPERATOR | FUNC_PRE_OPERATOR) != 0 && !f.friendly_name.is_empty() {
            format!("{}<{}>", f.friendly_name, name)
        } else {
            name
        };
        out.entry(f.native).or_insert(shown);
    }
}

/// Find a function/state/class export by "Class.Function" (or "Class.State.Function").
pub fn find_struct(p: &Package, path: &str) -> Option<usize> {
    let want = path.to_lowercase();
    (0..p.exports.len()).find(|&i| {
        let path = p.obj_path(i as i32 + 1).to_lowercase();
        // seek-free (_SF) packages keep their classes under a Package export
        is_struct_kind(&p.class_name(i as i32 + 1)) && (path == want || path.ends_with(&format!(".{want}")))
    })
}

/// UEnum value names.
pub fn read_enum(p: &Package, export: usize) -> Result<Vec<String>> {
    let data = p.export_data(export);
    let mut r = Reader::at(data, object_header(p, data)?);
    r.i32()?; // Next
    r.array(|r| fname(p, r))
}

/// Struct defaults (tagged properties after StructFlags) of a ScriptStruct export.
pub fn struct_defaults(p: &Package, export: usize) -> Result<Vec<props::Prop>> {
    let s = read_struct(p, export)?;
    let data = p.export_data(export);
    let mut r = Reader::at(data, s.tail + 4);
    props::parse_props(p, &mut r)
}
