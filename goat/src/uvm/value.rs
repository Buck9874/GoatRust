//! Script values.

use super::{ObjId, Sym};

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Byte(u8),
    Int(i32),
    Bool(bool),
    Float(f32),
    Name(Sym),
    Str(String),
    Obj(Option<ObjId>),
    Struct(Box<StructVal>),
    /// Dynamic arrays and static (ArrayDim > 1) arrays.
    Array(Vec<Value>),
    Delegate(Option<ObjId>, Sym),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructVal {
    pub def: Option<usize>,
    pub name: Sym,
    pub fields: Vec<(Sym, Value)>,
}

impl StructVal {
    pub fn field(&self, y: Sym) -> Option<&Value> {
        self.fields.iter().find(|f| f.0 == y).map(|f| &f.1)
    }
    pub fn field_mut(&mut self, y: Sym) -> &mut Value {
        if let Some(i) = self.fields.iter().position(|f| f.0 == y) {
            return &mut self.fields[i].1;
        }
        self.fields.push((y, Value::Int(0)));
        &mut self.fields.last_mut().unwrap().1
    }
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Byte(b) => *b != 0,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Obj(o) => o.is_some(),
            Value::Str(s) => !s.is_empty(),
            Value::Name(n) => n.0 != 0,
            _ => false,
        }
    }
    pub fn int(&self) -> i32 {
        match self {
            Value::Int(i) => *i,
            Value::Byte(b) => *b as i32,
            Value::Bool(b) => *b as i32,
            Value::Float(f) => *f as i32,
            _ => 0,
        }
    }
    pub fn float(&self) -> f32 {
        match self {
            Value::Float(f) => *f,
            Value::Int(i) => *i as f32,
            Value::Byte(b) => *b as f32,
            Value::Bool(b) => *b as i32 as f32,
            _ => 0.0,
        }
    }
    pub fn obj(&self) -> Option<ObjId> {
        match self {
            Value::Obj(o) => *o,
            _ => None,
        }
    }
    pub fn str(&self) -> &str {
        match self {
            Value::Str(s) => s,
            _ => "",
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Byte(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
            // UnrealScript prints floats with 2 decimals
            Value::Float(x) => write!(f, "{x:.2}"),
            Value::Str(s) => write!(f, "{s}"),
            other => write!(f, "{other:?}"),
        }
    }
}
