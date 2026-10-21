//! UnrealScript VM: runs compiled UE3 bytecode from the game's own packages (GoatGame.u,
//! Engine.u, Core.u) and from Steam Workshop mutators (mods/workshop/<id>/Scripts/*.u).
//!
//! Objects are script-level only: a class, a property map keyed by property name and an
//! optional state. Properties start from the class's default object (tagged defaults merged
//! down the class chain). Natives map to Rust functions (`natives.rs`) by "Class.Function";
//! anything missing is logged once and returns the zero value, so unknown code keeps going.

pub mod actors;
pub mod bridge;
pub mod collectibles;
pub mod config;
pub mod exec;
pub mod fx;
pub mod gfx;
pub mod inventory;
pub mod leaderboard;
pub mod kismet;
pub mod matinee;
pub mod menu_actions;
pub mod menus;
pub mod natives;
pub mod natives_engine;
pub mod npcs;
pub mod plugin;
pub mod progress;
pub mod proxies;
pub mod race;
pub mod settings;
pub mod sync;
pub mod touch;
pub mod value;

use std::collections::HashMap;
use upk::registry::{ObjRef, Registry};
use upk::script::{self, PropInfo, Stmt};
pub use value::{StructVal, Value};

pub type ObjId = u32;

/// Case-insensitive interned name (UnrealScript names and identifiers ignore case).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Sym(pub u32);

#[derive(Default)]
pub struct Interner {
    map: HashMap<String, Sym>,
    names: Vec<String>,
}

impl Interner {
    pub fn get(&mut self, s: &str) -> Sym {
        let key = s.to_ascii_lowercase();
        if let Some(&y) = self.map.get(&key) {
            return y;
        }
        let y = Sym(self.names.len() as u32);
        self.names.push(s.to_string());
        self.map.insert(key, y);
        y
    }
    /// Existing symbol without interning (read-only lookups).
    pub fn find(&self, s: &str) -> Option<Sym> {
        self.map.get(&s.to_ascii_lowercase()).copied()
    }
    pub fn str(&self, y: Sym) -> &str {
        self.names.get(y.0 as usize).map(|s| s.as_str()).unwrap_or("?")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Byte(Option<usize>),
    Int,
    Bool,
    Float,
    Name,
    Str,
    Object,
    Class,
    Interface,
    Delegate,
    Struct(usize),
    Array(Box<Ty>),
    Map,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct PropDef {
    pub name: Sym,
    pub ty: Ty,
    pub dim: i32,
    pub flags: u64,
}

pub struct StructDef {
    pub name: Sym,
    pub fields: Vec<PropDef>,
    /// Zero values with the struct's defaultproperties applied.
    pub defaults: Option<StructVal>,
    r: ObjRef,
}

pub struct EnumDef {
    pub names: Vec<String>,
}

pub struct StateDef {
    pub funcs: HashMap<Sym, ObjRef>,
    /// The state itself (its label code runs after GotoState).
    pub code: ObjRef,
}

pub struct Class {
    pub name: Sym,
    pub path: String,
    pub r: Option<ObjRef>,
    pub super_: Option<usize>,
    pub props: Vec<PropDef>,
    pub funcs: HashMap<Sym, ObjRef>,
    pub states: HashMap<Sym, StateDef>,
    pub cdo: Option<ObjId>,
    /// `config(Name)`: ini file Default<Name>.ini (None = inherited).
    pub config: Option<String>,
    /// Interfaces this class itself declares (`implements`).
    pub interfaces: Vec<usize>,
    pub is_interface: bool,
}

pub struct Func {
    pub name: Sym,
    /// "Class.Function" (state functions: "Class.State.Function").
    pub display: String,
    pub owner: usize,
    pub flags: u32,
    pub native: u16,
    pub pkg: usize,
    pub params: Vec<PropDef>,
    pub ret: Option<PropDef>,
    pub locals: Vec<PropDef>,
    pub code: std::sync::Arc<Vec<Stmt>>,
    /// Leading statements that hold optional-parameter defaults (one per optional param).
    pub n_optional: usize,
}

#[derive(Debug, Clone)]
pub enum Kind {
    Instance,
    /// A UClass object (`class'X'`); `class` is that class.
    ClassObj,
    /// Any other package object the VM doesn't model (materials, meshes, sounds...).
    Asset { path: String, class_name: String },
}

pub struct Object {
    pub class: usize,
    pub name: String,
    pub props: HashMap<Sym, Value>,
    pub state: Option<Sym>,
    pub outer: Option<ObjId>,
    pub kind: Kind,
    /// Component template inside a Default__ object (cloned per instance).
    pub template_of: Option<ObjRef>,
    pub destroyed: bool,
}

pub type NativeFn = fn(&mut Vm, &mut exec::Call) -> Value;

pub struct Timer {
    pub obj: ObjId,
    pub func: Sym,
    pub rate: f32,
    pub left: f32,
    pub looping: bool,
}

pub struct Vm {
    pub reg: Registry,
    pub syms: Interner,
    pub classes: Vec<Class>,
    class_by_ref: HashMap<ObjRef, usize>,
    class_by_name: HashMap<Sym, Vec<ObjRef>>,
    pub funcs: Vec<Func>,
    func_by_ref: HashMap<ObjRef, usize>,
    pub structs: Vec<StructDef>,
    struct_by_ref: HashMap<ObjRef, usize>,
    pub enums: Vec<EnumDef>,
    enum_by_ref: HashMap<ObjRef, usize>,
    pub objects: Vec<Object>,
    obj_by_ref: HashMap<ObjRef, ObjId>,
    obj_by_path: HashMap<String, ObjId>,
    class_obj: HashMap<usize, ObjId>,
    natives_by_index: HashMap<u16, ObjRef>,
    pub natives: HashMap<String, NativeFn>,
    /// Rust replacements for script functions ("class.function", lower case).
    pub overrides: HashMap<String, NativeFn>,
    pub timers: Vec<Timer>,
    /// Every call made, for the log ("-> Class.Function").
    pub log: Vec<String>,
    pub trace_calls: bool,
    pub missing: HashMap<String, usize>,
    pub depth: usize,
    pub budget: u64,
    /// Named well-known objects the host set up (engine, world info, game, ...).
    pub globals: HashMap<&'static str, ObjId>,
    pub echo: bool,
    pub frames: Vec<exec::Frame>,
    pub aborted: bool,
    /// Things the host game should do (material swaps...).
    pub events: Vec<bridge::HostEvent>,
    pub inis: HashMap<String, config::Ini>,
    /// The goat skeleton's bone names (MatchRefBone etc.); empty = unknown.
    pub bones: Vec<String>,
    /// Line traces against the game's physics world, installed by the host for each VM call.
    pub tracer: Option<Box<dyn Fn([f32; 3], [f32; 3]) -> Option<TraceHit> + Send + Sync>>,
    /// Game objects (NPCs, props) within a radius, installed like `tracer`.
    pub overlap: Option<OverlapFn>,
    /// A prop's world bounding box (Unreal units), installed like `tracer`.
    pub bounds: Option<BoundsFn>,
    /// The map's Kismet sequence objects (FindSeqObjectsByClass), set when Kismet loads.
    pub seq_objects: Vec<ObjId>,
    /// SequenceEvent.CheckActivate calls from script: (event, instigator), fired by the host's
    /// Kismet tick (it owns the sequence runner).
    pub kismet_activations: Vec<(ObjId, Option<ObjId>)>,
    /// WorldThing key -> its script proxy actor.
    pub proxies: HashMap<u64, ObjId>,
    pub proxy_things: HashMap<ObjId, WorldThing>,
    /// Objects running state code (GotoState with labels), see exec::tick_states.
    pub state_runs: HashMap<ObjId, exec::StateRun>,
    /// Set by a latent native (Sleep) to suspend the state code that called it.
    pub latent_sleep: Option<f32>,
    /// Set by a latent movement native (MoveTo...) to suspend the state code that called it.
    pub latent: Option<exec::Latent>,
    /// NPC animation lengths by name (host-provided), for SetAnimationInfoStruct's return value.
    pub anim_lengths: HashMap<String, f32>,
    /// Registry index of the current map package (Kismet, map actors).
    pub map_pkg: Option<usize>,
    /// Package object behind a VM object made from an export.
    pub ref_of: HashMap<ObjId, ObjRef>,
    /// Scaleform movies (GFxMoviePlayer) and the GFxObjects bound to them.
    pub gfx: gfx::Gfx,
}

/// A trace hit in Unreal units; `actor` is the script actor whose body was hit, `thing` a game
/// object without a script actor yet (None and None = level geometry).
#[derive(Debug, Clone)]
pub struct TraceHit {
    pub location: [f32; 3],
    pub normal: [f32; 3],
    pub actor: Option<ObjId>,
    pub thing: Option<WorldThing>,
}

/// A game object that gets a script proxy actor when a script finds it.
#[derive(Debug, Clone, PartialEq)]
pub enum WorldThing {
    /// NPC entity bits, actor name (GGNpc_12 -> class GGNpc).
    Npc { entity: u64, name: String },
    /// Rigid body handle bits, actor name (KActor props).
    Prop { body: u64, name: String },
}

impl WorldThing {
    pub fn key(&self) -> u64 {
        match self {
            WorldThing::Npc { entity, .. } => *entity,
            WorldThing::Prop { body, .. } => *body | (1 << 63),
        }
    }
}

pub type BoundsFn = Box<dyn Fn(&WorldThing) -> Option<([f32; 3], [f32; 3])> + Send + Sync>;
pub type OverlapFn = Box<dyn Fn([f32; 3], f32) -> Vec<(WorldThing, [f32; 3])> + Send + Sync>;

pub const NONE_CLASS: usize = usize::MAX;

impl Vm {
    pub fn new(reg: Registry) -> Self {
        let mut vm = Vm {
            reg,
            syms: Interner::default(),
            classes: vec![],
            class_by_ref: HashMap::new(),
            class_by_name: HashMap::new(),
            funcs: vec![],
            func_by_ref: HashMap::new(),
            structs: vec![],
            struct_by_ref: HashMap::new(),
            enums: vec![],
            enum_by_ref: HashMap::new(),
            objects: vec![],
            obj_by_ref: HashMap::new(),
            obj_by_path: HashMap::new(),
            class_obj: HashMap::new(),
            natives_by_index: HashMap::new(),
            natives: HashMap::new(),
            overrides: HashMap::new(),
            timers: vec![],
            log: vec![],
            trace_calls: true,
            missing: HashMap::new(),
            depth: 0,
            budget: 0,
            globals: HashMap::new(),
            echo: false,
            frames: vec![],
            aborted: false,
            events: vec![],
            inis: HashMap::new(),
            bones: vec![],
            tracer: None,
            overlap: None,
            bounds: None,
            seq_objects: vec![],
            kismet_activations: vec![],
            proxies: HashMap::new(),
            proxy_things: HashMap::new(),
            state_runs: HashMap::new(),
            latent_sleep: None,
            latent: None,
            anim_lengths: HashMap::new(),
            map_pkg: None,
            ref_of: HashMap::new(),
            gfx: Default::default(),
        };
        vm.index_packages();
        natives::register(&mut vm);
        natives_engine::register(&mut vm);
        natives_engine::register_overrides(&mut vm);
        // particles and particle-aware attach natives (replace the logging stubs)
        fx::register(&mut vm);
        gfx::register(&mut vm);
        settings::register(&mut vm);
        menu_actions::register(&mut vm);
        vm.overrides.insert("ggnpc.setanimationinfostruct".into(), npcs::set_animation_info);
        vm.natives.insert("ggaicontrollerbase.shouldcheckvisibility".into(), npcs::should_check_visibility);
        vm
    }

    fn index_packages(&mut self) {
        for pkg in 0..self.reg.packages.len() {
            let p = &self.reg.packages[pkg];
            for i in 0..p.exports.len() {
                match p.class_name(i as i32 + 1).as_str() {
                    "Class" => {
                        let n = p.obj_name(i as i32 + 1);
                        let y = self.syms.get(&n);
                        self.class_by_name.entry(y).or_default().push(ObjRef { pkg, export: i });
                    }
                    "Function" => {
                        if let Ok(s) = script::read_struct(p, i) {
                            if let Some(f) = s.func {
                                if f.native != 0 {
                                    self.natives_by_index.entry(f.native).or_insert(ObjRef { pkg, export: i });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn say(&mut self, s: String) {
        if self.echo {
            println!("[uvm] {s}");
        }
        self.log.push(s);
        if self.log.len() > 20000 {
            self.log.drain(..10000);
        }
    }

    pub fn missing(&mut self, what: String) {
        let n = self.missing.entry(what.clone()).or_default();
        *n += 1;
        if *n == 1 {
            self.say(format!("unsupported: {what} (skipped)"));
        }
    }

    pub fn sym(&mut self, s: &str) -> Sym {
        self.syms.get(s)
    }

    pub fn syms_get(&self, s: &str) -> Option<Sym> {
        self.syms.find(s)
    }

    pub fn name_of(&self, y: Sym) -> &str {
        self.syms.str(y)
    }

    // --- references --------------------------------------------------------------------

    pub fn resolve(&self, pkg: usize, idx: i32) -> Option<ObjRef> {
        if idx == 0 { None } else { self.reg.resolve(pkg, idx) }
    }

    /// Name of the object an operand points at (works for unresolvable imports too).
    pub fn ref_name(&mut self, pkg: usize, idx: i32) -> Sym {
        let n = self.reg.packages[pkg].obj_name(idx);
        self.syms.get(&n)
    }

    fn kind_of(&self, r: ObjRef) -> String {
        self.reg.class_name(r)
    }

    // --- classes ------------------------------------------------------------------------

    pub fn class_by_name(&mut self, name: &str) -> Option<usize> {
        let y = self.syms.get(name);
        let r = *self.class_by_name.get(&y)?.first()?;
        self.load_class(r)
    }

    pub fn load_class(&mut self, r: ObjRef) -> Option<usize> {
        if let Some(&c) = self.class_by_ref.get(&r) {
            return Some(c);
        }
        let p = &self.reg.packages[r.pkg];
        let s = script::read_struct(p, r.export).ok()?;
        let name = p.obj_name(r.export as i32 + 1);
        let path = format!("{}.{}", p.name, p.obj_path(r.export as i32 + 1));
        let kids = script::children(p, s.children);
        let super_ref = self.resolve(r.pkg, s.super_);
        let y = self.syms.get(&name);
        let id = self.classes.len();
        let config = Some(s.config_name.clone()).filter(|n| !n.is_empty() && !n.eq_ignore_ascii_case("None"));
        // CLASS_Interface
        let is_interface = s.class_flags & 0x4000 != 0;
        self.classes.push(Class { name: y, path, r: Some(r), super_: None, props: vec![], funcs: HashMap::new(), states: HashMap::new(), cdo: None, config, interfaces: vec![], is_interface });
        self.class_by_ref.insert(r, id);
        let sup = super_ref.and_then(|sr| self.load_class(sr));
        if sup.is_none() && s.super_ != 0 {
            let sp = self.reg.packages[r.pkg].obj_path(s.super_);
            self.say(format!("class {name}: superclass {sp} not loaded"));
        }
        self.classes[id].super_ = sup;
        let refs: Vec<ObjRef> = s.interfaces.iter().filter_map(|&i| self.resolve(r.pkg, i)).collect();
        let ifaces: Vec<usize> = refs.into_iter().filter_map(|ir| self.load_class(ir)).collect();
        self.classes[id].interfaces = ifaces;
        for k in kids {
            let kr = ObjRef { pkg: r.pkg, export: k as usize - 1 };
            let kind = self.kind_of(kr);
            let kname = self.reg.packages[r.pkg].obj_name(k);
            let ky = self.syms.get(&kname);
            if script::is_property_kind(&kind) {
                if let Some(pd) = self.prop_def(kr) {
                    self.classes[id].props.push(pd);
                }
            } else if kind == "Function" {
                self.classes[id].funcs.insert(ky, kr);
            } else if kind == "State" {
                let p = &self.reg.packages[r.pkg];
                let mut funcs = HashMap::new();
                if let Ok(st) = script::read_struct(p, kr.export) {
                    for f in script::children(p, st.children) {
                        if p.class_name(f) == "Function" {
                            let fname = p.obj_name(f);
                            funcs.insert(self.syms.get(&fname), ObjRef { pkg: r.pkg, export: f as usize - 1 });
                        }
                    }
                }
                self.classes[id].states.insert(ky, StateDef { funcs, code: kr });
            }
        }
        Some(id)
    }

    pub fn class_name(&self, c: usize) -> &str {
        if c == NONE_CLASS { "None" } else { self.syms.str(self.classes[c].name) }
    }

    pub fn is_child_of(&self, mut c: usize, parent: usize) -> bool {
        if c == NONE_CLASS || parent == NONE_CLASS {
            return false;
        }
        for _ in 0..64 {
            if c == parent {
                return true;
            }
            match self.classes[c].super_ {
                Some(s) => c = s,
                None => return false,
            }
        }
        false
    }

    /// Whether a class (or a parent) implements an interface (or one derived from it).
    pub fn implements(&self, mut c: usize, iface: usize) -> bool {
        if c == NONE_CLASS {
            return false;
        }
        for _ in 0..64 {
            if c == iface || self.classes[c].interfaces.iter().any(|&i| self.is_child_of(i, iface)) {
                return true;
            }
            match self.classes[c].super_ {
                Some(s) => c = s,
                None => return false,
            }
        }
        false
    }

    /// Property declaration by name along the class chain.
    pub fn find_prop(&self, mut c: usize, name: Sym) -> Option<&PropDef> {
        while c != NONE_CLASS {
            if let Some(p) = self.classes[c].props.iter().find(|p| p.name == name) {
                return Some(p);
            }
            c = self.classes[c].super_?;
        }
        None
    }

    fn prop_def(&mut self, r: ObjRef) -> Option<PropDef> {
        let q: PropInfo = script::read_property(&self.reg.packages[r.pkg], r.export).ok()?;
        let ty = self.ty_of(r.pkg, &q);
        Some(PropDef { name: self.syms.get(&q.name), ty, dim: q.array_dim.max(1), flags: q.flags })
    }

    fn ty_of(&mut self, pkg: usize, q: &PropInfo) -> Ty {
        match q.kind.as_str() {
            "ByteProperty" => Ty::Byte(q.refs.first().and_then(|&e| self.resolve(pkg, e)).and_then(|r| self.load_enum(r))),
            "IntProperty" => Ty::Int,
            "BoolProperty" => Ty::Bool,
            "FloatProperty" => Ty::Float,
            "NameProperty" => Ty::Name,
            "StrProperty" => Ty::Str,
            "ObjectProperty" | "ComponentProperty" => Ty::Object,
            "ClassProperty" => Ty::Class,
            "InterfaceProperty" => Ty::Interface,
            "DelegateProperty" => Ty::Delegate,
            "MapProperty" => Ty::Map,
            "StructProperty" => match q.refs.first().and_then(|&e| self.resolve(pkg, e)).and_then(|r| self.load_struct(r)) {
                Some(s) => Ty::Struct(s),
                None => Ty::Unknown,
            },
            "ArrayProperty" => {
                let inner = q.refs.first().and_then(|&e| self.resolve(pkg, e)).and_then(|r| {
                    let qi = script::read_property(&self.reg.packages[r.pkg], r.export).ok()?;
                    Some(self.ty_of(r.pkg, &qi))
                });
                Ty::Array(Box::new(inner.unwrap_or(Ty::Unknown)))
            }
            _ => Ty::Unknown,
        }
    }

    fn load_enum(&mut self, r: ObjRef) -> Option<usize> {
        if let Some(&e) = self.enum_by_ref.get(&r) {
            return Some(e);
        }
        let names = script::read_enum(&self.reg.packages[r.pkg], r.export).ok()?;
        self.enums.push(EnumDef { names });
        self.enum_by_ref.insert(r, self.enums.len() - 1);
        Some(self.enums.len() - 1)
    }

    pub fn load_struct(&mut self, r: ObjRef) -> Option<usize> {
        if let Some(&s) = self.struct_by_ref.get(&r) {
            return Some(s);
        }
        let p = &self.reg.packages[r.pkg];
        let s = script::read_struct(p, r.export).ok()?;
        let name = p.obj_name(r.export as i32 + 1);
        let kids = script::children(p, s.children);
        let super_ref = self.resolve(r.pkg, s.super_);
        let id = self.structs.len();
        let y = self.syms.get(&name);
        self.structs.push(StructDef { name: y, fields: vec![], defaults: None, r });
        self.struct_by_ref.insert(r, id);
        let mut fields = vec![];
        if let Some(sid) = super_ref.and_then(|sr| self.load_struct(sr)) {
            fields = self.structs[sid].fields.clone();
        }
        for k in kids {
            let kr = ObjRef { pkg: r.pkg, export: k as usize - 1 };
            if script::is_property_kind(&self.kind_of(kr)) {
                if let Some(pd) = self.prop_def(kr) {
                    fields.push(pd);
                }
            }
        }
        self.structs[id].fields = fields;
        Some(id)
    }

    /// Zero value of a struct with its defaultproperties.
    pub fn struct_zero(&mut self, s: usize) -> StructVal {
        if let Some(d) = &self.structs[s].defaults {
            return d.clone();
        }
        let fields = self.structs[s].fields.clone();
        let mut v = StructVal { def: Some(s), name: self.structs[s].name, fields: vec![] };
        for f in &fields {
            let z = self.zero(&f.ty, f.dim);
            v.fields.push((f.name, z));
        }
        // store before applying defaults so recursive structs terminate
        self.structs[s].defaults = Some(v.clone());
        let r = self.structs[s].r;
        if let Ok(props) = script::struct_defaults(&self.reg.packages[r.pkg], r.export) {
            for q in props {
                let y = self.syms.get(&q.name);
                let Some(f) = fields.iter().find(|f| f.name == y).cloned() else { continue };
                let val = self.convert_tagged(r.pkg, &f.ty, &q.value);
                set_indexed(v.field_mut(y), q.index, f.dim, val);
            }
        }
        self.structs[s].defaults = Some(v.clone());
        v
    }

    pub fn zero(&mut self, ty: &Ty, dim: i32) -> Value {
        if dim > 1 {
            let one = self.zero(ty, 1);
            return Value::Array(vec![one; dim as usize]);
        }
        match ty {
            Ty::Byte(_) => Value::Byte(0),
            Ty::Int => Value::Int(0),
            Ty::Bool => Value::Bool(false),
            Ty::Float => Value::Float(0.0),
            Ty::Name => Value::Name(self.syms.get("None")),
            Ty::Str => Value::Str(String::new()),
            Ty::Object | Ty::Class | Ty::Interface => Value::Obj(None),
            Ty::Delegate => Value::Delegate(None, Sym(0)),
            Ty::Struct(s) => Value::Struct(Box::new(self.struct_zero(*s))),
            Ty::Array(_) | Ty::Map => Value::Array(vec![]),
            Ty::Unknown => Value::Int(0),
        }
    }

    // --- functions ----------------------------------------------------------------------

    pub fn load_func(&mut self, r: ObjRef) -> Option<usize> {
        if let Some(&f) = self.func_by_ref.get(&r) {
            return Some(f);
        }
        let p = &self.reg.packages[r.pkg];
        let s = script::read_struct(p, r.export).ok()?;
        let fi = s.func.clone().unwrap_or_default();
        let name = p.obj_name(r.export as i32 + 1);
        let outer = p.obj_outer(r.export as i32 + 1);
        let (owner_ref, display) = if p.class_name(outer) == "State" {
            let cls = p.obj_outer(outer);
            (cls, format!("{}.{}.{}", p.obj_name(cls), p.obj_name(outer), name))
        } else {
            (outer, format!("{}.{}", p.obj_name(outer), name))
        };
        let kids = script::children(p, s.children);
        let decoded = script::decode(p, &s.script);
        let code = match decoded {
            Ok(c) => c,
            Err(e) => {
                let msg = format!("{display}: bytecode decode failed: {e}");
                self.say(msg);
                vec![]
            }
        };
        let owner = if owner_ref > 0 { self.load_class(ObjRef { pkg: r.pkg, export: owner_ref as usize - 1 }) } else { None };
        let mut params = vec![];
        let mut locals = vec![];
        let mut ret = None;
        for k in kids {
            let kr = ObjRef { pkg: r.pkg, export: k as usize - 1 };
            if !script::is_property_kind(&self.kind_of(kr)) {
                continue;
            }
            let Some(pd) = self.prop_def(kr) else { continue };
            if pd.flags & script::CPF_RETURN_PARM != 0 {
                ret = Some(pd.clone());
            } else if pd.flags & script::CPF_PARM != 0 {
                params.push(pd.clone());
            }
            locals.push(pd);
        }
        let n_optional = params.iter().filter(|p| p.flags & script::CPF_OPTIONAL_PARM != 0).count();
        let y = self.syms.get(&name);
        self.funcs.push(Func {
            name: y,
            display,
            owner: owner.unwrap_or(NONE_CLASS),
            flags: fi.flags,
            native: fi.native,
            pkg: r.pkg,
            params,
            ret,
            locals,
            code: std::sync::Arc::new(code),
            n_optional,
        });
        let id = self.funcs.len() - 1;
        self.func_by_ref.insert(r, id);
        Some(id)
    }

    pub fn native_func(&mut self, index: u16) -> Option<usize> {
        let r = *self.natives_by_index.get(&index)?;
        self.load_func(r)
    }

    /// Virtual lookup: state functions first (along the class chain), then class functions.
    pub fn find_func(&mut self, class: usize, name: Sym, state: Option<Sym>) -> Option<usize> {
        if class == NONE_CLASS {
            return None;
        }
        if let Some(st) = state {
            let mut c = Some(class);
            while let Some(ci) = c {
                if let Some(&r) = self.classes[ci].states.get(&st).and_then(|s| s.funcs.get(&name)) {
                    return self.load_func(r);
                }
                c = self.classes[ci].super_;
            }
        }
        let mut c = Some(class);
        while let Some(ci) = c {
            if let Some(&r) = self.classes[ci].funcs.get(&name) {
                return self.load_func(r);
            }
            c = self.classes[ci].super_;
        }
        None
    }

    // --- objects ------------------------------------------------------------------------

    pub fn obj(&self, id: ObjId) -> &Object {
        &self.objects[id as usize]
    }
    pub fn obj_mut(&mut self, id: ObjId) -> &mut Object {
        &mut self.objects[id as usize]
    }

    fn push_obj(&mut self, o: Object) -> ObjId {
        self.objects.push(o);
        (self.objects.len() - 1) as ObjId
    }

    /// The UClass object for a class.
    pub fn class_object(&mut self, c: usize) -> ObjId {
        if let Some(&o) = self.class_obj.get(&c) {
            return o;
        }
        let name = self.class_name(c).to_string();
        let o = self.push_obj(Object {
            class: c,
            name,
            props: HashMap::new(),
            state: None,
            outer: None,
            kind: Kind::ClassObj,
            template_of: None,
            destroyed: false,
        });
        self.class_obj.insert(c, o);
        o
    }

    /// Class of a class object, or None.
    pub fn as_class(&self, v: &Value) -> Option<usize> {
        match v {
            Value::Obj(Some(o)) if matches!(self.obj(*o).kind, Kind::ClassObj) => Some(self.obj(*o).class),
            _ => None,
        }
    }

    /// Class default object (defaults merged down the class chain).
    pub fn cdo(&mut self, c: usize) -> ObjId {
        if let Some(o) = self.classes[c].cdo {
            return o;
        }
        let mut props = match self.classes[c].super_ {
            Some(s) => {
                let so = self.cdo(s);
                self.obj(so).props.clone()
            }
            None => HashMap::new(),
        };
        let own = self.classes[c].props.clone();
        for pd in &own {
            if !props.contains_key(&pd.name) {
                let z = self.zero(&pd.ty, pd.dim);
                props.insert(pd.name, z);
            }
        }
        let name = format!("Default__{}", self.class_name(c));
        let o = self.push_obj(Object {
            class: c,
            name,
            props,
            state: None,
            outer: None,
            kind: Kind::Instance,
            template_of: None,
            destroyed: false,
        });
        self.classes[c].cdo = Some(o);
        // tagged defaults of Default__<Class>
        if let Some(r) = self.classes[c].r {
            let p = &self.reg.packages[r.pkg];
            let want = format!("Default__{}", p.obj_name(r.export as i32 + 1));
            let class_idx = r.export as i32 + 1;
            let def = (0..p.exports.len()).find(|&i| p.exports[i].class == class_idx && p.name_str(p.exports[i].name).eq_ignore_ascii_case(&want));
            if let Some(d) = def {
                let dref = ObjRef { pkg: r.pkg, export: d };
                self.obj_by_ref.insert(dref, o);
                self.apply_tagged(o, dref);
            }
        }
        self.apply_config(c, o);
        self.apply_localized(c, o);
        o
    }

    /// Apply an export's tagged properties to a VM object.
    fn apply_tagged(&mut self, o: ObjId, r: ObjRef) {
        let props = self.reg.props(r);
        let class = self.obj(o).class;
        for q in props {
            let y = self.syms.get(&q.name);
            let Some(pd) = self.find_prop(class, y).cloned() else { continue };
            let v = self.convert_tagged(r.pkg, &pd.ty, &q.value);
            let slot = self.objects[o as usize].props.entry(y).or_insert(Value::Int(0));
            set_indexed(slot, q.index, pd.dim, v);
        }
    }

    /// Object for a package reference (class object, component template or asset stub).
    pub fn obj_for_ref(&mut self, pkg: usize, idx: i32) -> Option<ObjId> {
        if idx == 0 {
            return None;
        }
        let Some(r) = self.resolve(pkg, idx) else {
            // import from a package that isn't loaded: keep a named stub
            let p = &self.reg.packages[pkg];
            let path = p.obj_path(idx);
            let class_name = p.class_name(idx);
            if let Some(&o) = self.obj_by_path.get(&path.to_lowercase()) {
                return Some(o);
            }
            let name = p.obj_name(idx);
            let class = self.class_by_name(&class_name).unwrap_or(NONE_CLASS);
            let o = self.push_obj(Object { class, name, props: HashMap::new(), state: None, outer: None, kind: Kind::Asset { path: path.clone(), class_name }, template_of: None, destroyed: false });
            self.obj_by_path.insert(path.to_lowercase(), o);
            return Some(o);
        };
        self.obj_for(r)
    }

    pub fn obj_for(&mut self, r: ObjRef) -> Option<ObjId> {
        if let Some(&o) = self.obj_by_ref.get(&r) {
            return Some(o);
        }
        let kind = self.kind_of(r);
        if kind == "Class" {
            let c = self.load_class(r)?;
            let o = self.class_object(c);
            self.obj_by_ref.insert(r, o);
            return Some(o);
        }
        let p = &self.reg.packages[r.pkg];
        // cooked-in assets keep their own package path (goat.Materials.Goat_Mat_08)
        let path = p.obj_path(r.export as i32 + 1);
        let name = p.obj_name(r.export as i32 + 1);
        // inside a Default__ object -> a component/subobject template with real properties
        let mut outer = p.exports[r.export].outer;
        let mut in_default = None;
        while outer > 0 {
            if p.obj_name(outer).starts_with("Default__") {
                in_default = Some(ObjRef { pkg: r.pkg, export: outer as usize - 1 });
                break;
            }
            outer = p.obj_outer(outer);
        }
        let class_ref = self.resolve(r.pkg, p.exports[r.export].class);
        let class = class_ref.and_then(|cr| self.load_class(cr)).or_else(|| self.class_by_name(&kind)).unwrap_or(NONE_CLASS);
        if in_default.is_some() && class != NONE_CLASS {
            let cdo = self.cdo(class);
            let props = self.obj(cdo).props.clone();
            let o = self.push_obj(Object { class, name, props, state: None, outer: None, kind: Kind::Instance, template_of: in_default, destroyed: false });
            self.obj_by_ref.insert(r, o);
            self.apply_tagged(o, r);
            return Some(o);
        }
        // map actors, their components and Kismet objects: real instances with their saved values
        let world_class = ["Actor", "SequenceObject", "ActorComponent", "MapInfo"].iter().any(|b| self.class_by_name(b).is_some_and(|bc| self.is_child_of(class, bc)))
            // GoatGame objects saved in the level (GGMapInfo's GGMapMusicInfo...)
            || (path.contains(".PersistentLevel.") && self.class_name(class).starts_with("GG"));
        if world_class && class != NONE_CLASS {
            self.cdo(class);
            let o = self.push_obj(Object { class, name, props: HashMap::new(), state: None, outer: None, kind: Kind::Instance, template_of: None, destroyed: false });
            self.obj_by_ref.insert(r, o);
            self.ref_of.insert(o, r);
            let outer_ref = self.reg.packages[r.pkg].exports[r.export].outer;
            if outer_ref > 0 {
                let oo = self.obj_for_ref(r.pkg, outer_ref);
                self.obj_mut(o).outer = oo;
            }
            if let Some(&wi) = self.globals.get("worldinfo") {
                if self.class_by_name("Actor").is_some_and(|a| self.is_child_of(class, a)) {
                    self.objects[o as usize].props.insert(self.syms.get("WorldInfo"), Value::Obj(Some(wi)));
                }
            }
            self.apply_tagged(o, r);
            return Some(o);
        }
        let o = self.push_obj(Object { class, name, props: HashMap::new(), state: None, outer: None, kind: Kind::Asset { path, class_name: kind }, template_of: None, destroyed: false });
        self.obj_by_ref.insert(r, o);
        // textures keep their tagged properties: scripts read Texture2D.SizeX/SizeY directly
        if self.class_by_name("Texture").is_some_and(|t| self.is_child_of(class, t)) {
            self.apply_tagged(o, r);
        }
        Some(o)
    }

    /// Convert a tagged default value to a VM value of the declared type.
    pub fn convert_tagged(&mut self, pkg: usize, ty: &Ty, v: &upk::props::Value) -> Value {
        use upk::props::Value as T;
        let fv = |x: &[f32], names: &[&str], vm: &mut Vm| {
            let fields = names.iter().zip(x).map(|(n, f)| (vm.syms.get(n), Value::Float(*f))).collect();
            let name = vm.syms.get(match names.len() { 2 => "Vector2D", 3 => "Vector", _ => "Vector4" });
            Value::Struct(Box::new(StructVal { def: None, name, fields }))
        };
        let base = match ty {
            Ty::Struct(s) => Some(self.struct_zero(*s)),
            _ => None,
        };
        let with_fields = |vm: &mut Vm, pairs: Vec<(&str, Value)>| -> Value {
            let mut sv = base.clone().unwrap_or_else(|| StructVal { def: None, name: Sym(0), fields: vec![] });
            for (n, x) in pairs {
                let y = vm.syms.get(n);
                *sv.field_mut(y) = x;
            }
            Value::Struct(Box::new(sv))
        };
        match v {
            T::Int(i) => match ty {
                Ty::Float => Value::Float(*i as f32),
                _ => Value::Int(*i),
            },
            T::Float(f) => Value::Float(*f),
            T::Bool(b) => Value::Bool(*b),
            T::Byte(b) => Value::Byte(*b),
            T::Enum(s) => {
                let idx = match ty {
                    // "None" (a value past the enum when cooked) loads as INDEX_NONE -> 255 in UE3
                    Ty::Byte(Some(e)) => self.enums[*e].names.iter().position(|n| n.eq_ignore_ascii_case(s)).unwrap_or(if s == "None" { 255 } else { 0 }),
                    _ => 0,
                };
                Value::Byte(idx as u8)
            }
            T::Name(s) => Value::Name(self.syms.get(s)),
            T::Str(s) => Value::Str(s.clone()),
            T::Object(o) => Value::Obj(self.obj_for_ref(pkg, *o)),
            T::Vector(x) => with_fields(self, vec![("X", Value::Float(x[0])), ("Y", Value::Float(x[1])), ("Z", Value::Float(x[2]))]),
            T::Rotator(x) => with_fields(self, vec![("Pitch", Value::Int(x[0])), ("Yaw", Value::Int(x[1])), ("Roll", Value::Int(x[2]))]),
            T::Color(c) => with_fields(self, vec![("B", Value::Byte(c[0])), ("G", Value::Byte(c[1])), ("R", Value::Byte(c[2])), ("A", Value::Byte(c[3]))]),
            T::LinearColor(c) => with_fields(self, vec![("R", Value::Float(c[0])), ("G", Value::Float(c[1])), ("B", Value::Float(c[2])), ("A", Value::Float(c[3]))]),
            T::Vector2D(x) => with_fields(self, vec![("X", Value::Float(x[0])), ("Y", Value::Float(x[1]))]),
            T::Quat(x) | T::Vector4(x) => {
                if base.is_some() {
                    let names: &[&str] = if matches!(v, T::Quat(_)) { &["X", "Y", "Z", "W"] } else { &["X", "Y", "Z", "W"] };
                    with_fields(self, names.iter().zip(x).map(|(n, f)| (*n, Value::Float(*f))).collect())
                } else {
                    fv(x, &["X", "Y", "Z", "W"], self)
                }
            }
            T::Struct(_, props) => {
                let mut sv = base.clone().unwrap_or_else(|| StructVal { def: None, name: Sym(0), fields: vec![] });
                let fields = match ty {
                    Ty::Struct(s) => self.structs[*s].fields.clone(),
                    _ => vec![],
                };
                for q in props {
                    let y = self.syms.get(&q.name);
                    let (fty, dim) = fields.iter().find(|f| f.name == y).map(|f| (f.ty.clone(), f.dim)).unwrap_or((Ty::Unknown, 1));
                    let x = self.convert_tagged(pkg, &fty, &q.value);
                    set_indexed(sv.field_mut(y), q.index, dim, x);
                }
                Value::Struct(Box::new(sv))
            }
            T::Array(n, raw) => {
                let inner = match ty {
                    Ty::Array(i) => (**i).clone(),
                    _ => Ty::Unknown,
                };
                Value::Array(self.decode_array(pkg, &inner, *n, raw))
            }
            T::Raw(..) => self.zero(ty, 1),
        }
    }

    fn decode_array(&mut self, pkg: usize, inner: &Ty, n: i32, raw: &[u8]) -> Vec<Value> {
        let mut r = upk::reader::Reader::new(raw);
        let mut out = vec![];
        for _ in 0..n.max(0) {
            let v = (|| -> anyhow::Result<Value> {
                Ok(match inner {
                    Ty::Int => Value::Int(r.i32()?),
                    Ty::Float => Value::Float(r.f32()?),
                    Ty::Bool => Value::Bool(r.u32()? != 0),
                    Ty::Byte(_) => Value::Byte(r.u8()?),
                    Ty::Object | Ty::Class | Ty::Interface => {
                        let i = r.i32()?;
                        Value::Obj(self.obj_for_ref(pkg, i))
                    }
                    Ty::Name => {
                        let (i, num) = (r.i32()?, r.i32()?);
                        let s = self.reg.packages[pkg].name_str(upk::package::FName { index: i, number: num });
                        Value::Name(self.syms.get(&s))
                    }
                    Ty::Str => Value::Str(r.fstring()?),
                    Ty::Struct(_) => {
                        let props = upk::props::parse_props(&self.reg.packages[pkg], &mut r)?;
                        let sname = String::new();
                        self.convert_tagged(pkg, inner, &upk::props::Value::Struct(sname, props))
                    }
                    _ => anyhow::bail!("array of {inner:?}"),
                })
            })();
            match v {
                Ok(v) => out.push(v),
                Err(_) => break,
            }
        }
        out
    }

    /// A property's value on the class default object (instances read through to it).
    pub fn default_of(&self, o: ObjId, y: Sym) -> Option<&Value> {
        let c = self.obj(o).class;
        if c == NONE_CLASS {
            return None;
        }
        let cdo = self.classes[c].cdo?;
        if cdo == o {
            return None;
        }
        self.obj(cdo).props.get(&y)
    }

    /// New instance of a class: CDO values, fresh copies of component templates.
    pub fn spawn_object(&mut self, class: usize, name: Option<String>) -> ObjId {
        self.spawn_object_with(class, name, false)
    }

    /// `light`: only the component properties are copied; everything else reads through to
    /// the class defaults until written (proxies for thousands of props).
    pub fn spawn_object_with(&mut self, class: usize, name: Option<String>, light: bool) -> ObjId {
        let cdo = self.cdo(class);
        let mut props = if light {
            self.obj(cdo).props.iter().filter(|(_, v)| v.obj().is_some_and(|t| self.obj(t).template_of.is_some())).map(|(k, v)| (*k, v.clone())).collect()
        } else {
            self.obj(cdo).props.clone()
        };
        let mut copied: HashMap<ObjId, ObjId> = HashMap::new();
        let keys: Vec<Sym> = props.keys().copied().collect();
        for k in keys {
            if let Some(Value::Obj(Some(t))) = props.get(&k) {
                let t = *t;
                if self.obj(t).template_of.is_some() {
                    let fresh = *copied.entry(t).or_insert_with(|| {
                        let src = &self.objects[t as usize];
                        let o = Object { class: src.class, name: src.name.clone(), props: src.props.clone(), state: None, outer: None, kind: Kind::Instance, template_of: None, destroyed: false };
                        self.objects.push(o);
                        (self.objects.len() - 1) as ObjId
                    });
                    props.insert(k, Value::Obj(Some(fresh)));
                }
            }
        }
        let n = self.objects.len();
        let name = name.unwrap_or_else(|| format!("{}_{}", self.class_name(class), n));
        let o = self.push_obj(Object { class, name, props, state: None, outer: None, kind: Kind::Instance, template_of: None, destroyed: false });
        for fresh in copied.into_values() {
            self.obj_mut(fresh).outer = Some(o);
        }
        o
    }

    pub fn get(&mut self, o: ObjId, prop: &str) -> Value {
        let y = self.syms.get(prop);
        self.obj(o).props.get(&y).or_else(|| self.default_of(o, y)).cloned().unwrap_or(Value::Int(0))
    }

    pub fn set(&mut self, o: ObjId, prop: &str, v: Value) {
        let y = self.syms.get(prop);
        self.obj_mut(o).props.insert(y, v);
    }

    pub fn describe(&self, v: &Value) -> String {
        match v {
            Value::Obj(Some(o)) => {
                let ob = self.obj(*o);
                match &ob.kind {
                    Kind::ClassObj => format!("class'{}'", ob.name),
                    Kind::Asset { path, class_name } => format!("{class_name}'{path}'"),
                    Kind::Instance => ob.name.clone(),
                }
            }
            Value::Obj(None) => "None".into(),
            Value::Name(y) => format!("'{}'", self.syms.str(*y)),
            Value::Str(s) => format!("{s:?}"),
            Value::Struct(s) => {
                let f: Vec<String> = s.fields.iter().map(|(n, x)| format!("{}={}", self.syms.str(*n), self.describe(x))).collect();
                format!("({})", f.join(","))
            }
            Value::Array(a) => {
                let f: Vec<String> = a.iter().take(8).map(|x| self.describe(x)).collect();
                format!("[{}{}]", f.join(","), if a.len() > 8 { ",..." } else { "" })
            }
            Value::Delegate(o, f) => format!("delegate {}.{}", o.map(|o| self.obj(o).name.clone()).unwrap_or_default(), self.syms.str(*f)),
            other => other.to_string(),
        }
    }

    /// Every distinct native/override name a set of functions would need (for reports).
    pub fn native_report(&self) -> Vec<(String, usize)> {
        let mut v: Vec<(String, usize)> = self.missing.iter().map(|(k, n)| (k.clone(), *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1));
        v
    }
}

/// Write `v` into element `index` of a (possibly static-array) slot.
pub fn set_indexed(slot: &mut Value, index: i32, dim: i32, v: Value) {
    if dim > 1 {
        if let Value::Array(a) = slot {
            if (index as usize) < a.len() {
                a[index as usize] = v;
            }
            return;
        }
    }
    *slot = v;
}

impl Vm {
    /// Whether state `t` of `class` (or a parent class) is `parent` or extends it, following
    /// each State's SuperField (Object.IsChildState).
    pub fn state_extends(&mut self, class: usize, t: Sym, parent: Sym) -> bool {
        let want = self.name_of(parent).to_ascii_lowercase();
        // the class (or parent class) that declares state `t`
        let mut ci = Some(class);
        let mut r = None;
        for _ in 0..32 {
            let Some(c) = ci else { break };
            if let Some(st) = self.classes[c].states.get(&t) {
                r = Some(st.code);
                break;
            }
            ci = self.classes[c].super_;
        }
        for _ in 0..16 {
            let Some(sr) = r else { return false };
            let p = &self.reg.packages[sr.pkg];
            if p.obj_name(sr.export as i32 + 1).eq_ignore_ascii_case(&want) {
                return true;
            }
            r = upk::script::read_struct(p, sr.export).ok().filter(|st| st.super_ != 0).and_then(|st| self.reg.resolve(sr.pkg, st.super_));
        }
        false
    }

    /// The script actor standing in for a game object (created on first use, no BeginPlay).
    pub fn proxy_for(&mut self, thing: &WorldThing, location: [f32; 3]) -> ObjId {
        if let Some(&o) = self.proxies.get(&thing.key()) {
            return o;
        }
        let (class, name) = match thing {
            WorldThing::Npc { name, .. } => {
                let base = name.rsplit_once('_').map(|(b, _)| b).unwrap_or(name);
                (self.class_by_name(base).or_else(|| self.class_by_name("GGNpc")), name.clone())
            }
            WorldThing::Prop { name, .. } => (self.class_by_name("GGKActor").or_else(|| self.class_by_name("KActor")), name.clone()),
        };
        let class = class.or_else(|| self.class_by_name("Actor")).expect("Actor class");
        let from_map = self.map_pkg.and_then(|mp| {
            let p = &self.reg.packages[mp];
            let want = format!(".PersistentLevel.{name}").to_ascii_lowercase();
            let i = (0..p.exports.len()).find(|&i| p.obj_path(i as i32 + 1).to_ascii_lowercase().ends_with(&want))?;
            self.obj_for_ref(mp, i as i32 + 1)
        });
        let o = match from_map {
            Some(o) => o,
            None => self.spawn_object_with(class, Some(name), true),
        };
        if let Some(&wi) = self.globals.get("worldinfo") {
            self.set(o, "WorldInfo", Value::Obj(Some(wi)));
        }
        let loc = self.vector(location[0], location[1], location[2]);
        self.set(o, "Location", loc);
        self.set(o, "Physics", Value::Byte(if matches!(thing, WorldThing::Prop { .. }) { 10 } else { 1 }));
        self.proxies.insert(thing.key(), o);
        self.proxy_things.insert(o, thing.clone());
        o
    }
}
