//! The bytecode interpreter: statements, expressions, places (lvalues) and calls.

use super::*;
use std::sync::Arc;
use upk::script::{CPF_OUT_PARM, Expr, FUNC_NATIVE, FUNC_STATIC};

/// Arguments of a call as the callee sees them. Natives may write back out params by
/// changing `args[i]`.
pub struct Call {
    pub this: Option<ObjId>,
    pub func: usize,
    pub args: Vec<Value>,
    pub passed: Vec<bool>,
}

impl Call {
    pub fn arg(&self, i: usize) -> &Value {
        static NONE: Value = Value::Obj(None);
        self.args.get(i).unwrap_or(&NONE)
    }
    pub fn int(&self, i: usize) -> i32 {
        self.arg(i).int()
    }
    pub fn float(&self, i: usize) -> f32 {
        self.arg(i).float()
    }
    pub fn str(&self, i: usize) -> String {
        self.arg(i).str().to_string()
    }
    pub fn obj(&self, i: usize) -> Option<ObjId> {
        self.arg(i).obj()
    }
    pub fn given(&self, i: usize) -> bool {
        self.passed.get(i).copied().unwrap_or(false)
    }
}

/// State code in progress: the state's code, the next statement, seconds left asleep.
pub struct StateRun {
    pub func: usize,
    pub pc: usize,
    pub sleep: f32,
    /// A latent movement the code waits for (MoveTo, FinishRotation...).
    pub wait: Option<Latent>,
}

/// Latent controller functions: the state code resumes when the pawn gets there (or on timeout).
#[derive(Debug, Clone)]
pub enum Latent {
    /// MoveTo / MoveToward: reach `dest` (or `target`'s location) within `dist` Unreal units.
    Move { dest: [f32; 3], target: Option<ObjId>, dist: f32, walk: bool, left: f32 },
    /// FinishRotation / FinishAnim: a short wait.
    Wait { left: f32 },
    /// WaitForLanding: until the pawn stops falling.
    Land { left: f32 },
}

pub struct Frame {
    pub func: usize,
    pub this: Option<ObjId>,
    pub locals: HashMap<Sym, Value>,
    iters: Vec<Iter>,
}

struct Iter {
    items: Vec<Vec<Value>>,
    next: usize,
    places: Vec<Place>,
    body: usize,
    end: usize,
}

#[derive(Debug, Clone)]
pub enum Place {
    Local(Sym),
    Obj(ObjId, Sym),
    Member(Box<Place>, Sym),
    /// Array element; the zero value grows dynamic arrays on write.
    Index(Box<Place>, usize, Option<Value>),
    /// A dynamic array's Length (assigning resizes it, with the element zero).
    Length(Box<Place>, Option<Value>),
    Temp(Value),
}

enum Flow {
    Next,
    Jump(usize),
    Return(Value),
}

/// Statements a single top-level call may run before it is stopped (runaway loops).
pub const BUDGET: u64 = 2_000_000;
pub const MAX_DEPTH: usize = 1000;

impl Vm {
    pub fn frames(&self) -> &[Frame] {
        &self.frames
    }

    fn this(&self) -> Option<ObjId> {
        self.frames.last().and_then(|f| f.this)
    }

    fn cur_func(&self) -> String {
        self.frames.last().map(|f| self.funcs[f.func].display.clone()).unwrap_or_else(|| "<host>".into())
    }

    fn cur_pkg(&self) -> usize {
        self.frames.last().map(|f| self.funcs[f.func].pkg).unwrap_or(0)
    }

    fn rsym(&mut self, idx: i32) -> Sym {
        let pkg = self.cur_pkg();
        self.ref_name(pkg, idx)
    }

    // --- host entry points ----------------------------------------------------------------

    /// Call a function by name on an object (virtual lookup). Missing functions are a no-op.
    pub fn call_named(&mut self, this: ObjId, name: &str, args: Vec<Value>) -> Option<Value> {
        let y = self.sym(name);
        let class = self.obj(this).class;
        let state = self.obj(this).state;
        let f = self.find_func(class, y, state)?;
        let top = self.frames.is_empty();
        if top {
            self.budget = BUDGET;
            self.aborted = false;
        }
        let passed = vec![true; args.len()];
        let (v, _) = self.invoke(Some(this), f, args, passed);
        Some(v)
    }

    /// Whether a class (or a parent below `stop_at`) defines a script function with this name.
    pub fn defines(&mut self, class: usize, name: &str) -> Option<usize> {
        let y = self.sym(name);
        let f = self.find_func(class, y, None)?;
        if self.funcs[f].flags & FUNC_NATIVE != 0 || self.funcs[f].code.len() <= 2 {
            return None;
        }
        Some(f)
    }

    // --- calls -------------------------------------------------------------------------------

    /// Run a function with evaluated arguments. Returns the result and final parameter values.
    pub fn invoke(&mut self, this: Option<ObjId>, func: usize, mut args: Vec<Value>, mut passed: Vec<bool>) -> (Value, Vec<Value>) {
        let nparams = self.funcs[func].params.len();
        let params = self.funcs[func].params.clone();
        while args.len() < nparams {
            args.push(Value::Int(0));
            passed.push(false);
        }
        for (i, p) in params.iter().enumerate() {
            if !passed[i] {
                args[i] = self.zero(&p.ty, p.dim);
            } else {
                args[i] = coerce(std::mem::replace(&mut args[i], Value::Int(0)), &p.ty);
            }
        }
        if self.depth >= MAX_DEPTH || self.aborted {
            if !self.aborted {
                let d = self.funcs[func].display.clone();
                self.say(format!("call depth limit at {d}; stopping this event"));
                self.aborted = true;
            }
            return (self.ret_zero(func), args);
        }
        let display = self.funcs[func].display.clone();
        let key = display.to_ascii_lowercase();
        if self.trace_calls {
            let a: Vec<String> = args.iter().map(|v| self.describe(v)).collect();
            let who = this.map(|o| self.obj(o).name.clone()).unwrap_or_default();
            let ind = "  ".repeat(self.depth.min(20));
            let native = match (self.funcs[func].flags & FUNC_NATIVE != 0, self.funcs[func].native) {
                (false, _) => String::new(),
                (true, 0) => " [native]".into(),
                (true, n) => format!(" [native {n}]"),
            };
            self.say(format!("{ind}-> {display}({}){native}{}", a.join(", "), if who.is_empty() { String::new() } else { format!(" on {who}") }));
        }
        let ov = self.overrides.get(&key).copied();
        let flags = self.funcs[func].flags;
        if ov.is_some() || flags & FUNC_NATIVE != 0 {
            let f = ov.or_else(|| self.native_lookup(func));
            let mut call = Call { this, func, args, passed };
            self.depth += 1;
            let v = match f {
                Some(f) => f(self, &mut call),
                None => match natives::operator(self, &mut call) {
                    Some(v) => v,
                    None => {
                        self.missing(format!("native {display}"));
                        self.ret_zero(func)
                    }
                },
            };
            self.depth -= 1;
            let v = match self.funcs[func].ret.clone() {
                Some(r) => coerce(v, &r.ty),
                None => v,
            };
            return (v, call.args);
        }
        // script function
        let locals_def = self.funcs[func].locals.clone();
        let mut locals = HashMap::new();
        for l in &locals_def {
            let z = self.zero(&l.ty, l.dim);
            locals.insert(l.name, z);
        }
        for (i, p) in params.iter().enumerate() {
            locals.insert(p.name, args[i].clone());
        }
        let this = if flags & FUNC_STATIC != 0 && this.is_none() { None } else { this };
        self.frames.push(Frame { func, this, locals, iters: vec![] });
        self.depth += 1;
        let code = self.funcs[func].code.clone();
        // optional parameter defaults: one leading statement per optional param
        let mut pc = 0;
        let n_opt = self.funcs[func].n_optional;
        if n_opt > 0 && code.len() >= n_opt && code[..n_opt].iter().all(|s| matches!(s.expr, Expr::Nothing | Expr::DefaultParmValue(..))) {
            let optional: Vec<usize> = params.iter().enumerate().filter(|(_, p)| p.flags & upk::script::CPF_OPTIONAL_PARM != 0).map(|(i, _)| i).collect();
            for (k, &pi) in optional.iter().enumerate() {
                if let Expr::DefaultParmValue(_, e) = &code[k].expr {
                    if !passed[pi] {
                        let v = self.eval(e, this);
                        let v = coerce(v, &params[pi].ty);
                        if let Some(f) = self.frames.last_mut() {
                            f.locals.insert(params[pi].name, v);
                        }
                    }
                }
            }
            pc = n_opt;
        }
        let ret = self.run(&code, pc, this);
        self.depth -= 1;
        let frame = self.frames.pop().unwrap();
        let finals: Vec<Value> = params.iter().map(|p| frame.locals.get(&p.name).cloned().unwrap_or(Value::Int(0))).collect();
        let ret = match ret {
            Some(v) => v,
            None => match &self.funcs[func].ret {
                Some(r) => frame.locals.get(&r.name).cloned().unwrap_or(Value::Int(0)),
                None => Value::Int(0),
            },
        };
        let ret = match self.funcs[func].ret.clone() {
            Some(r) => coerce(ret, &r.ty),
            None => ret,
        };
        (ret, finals)
    }

    /// Native implementation for a function: "Class.Func" along the owner's class chain
    /// (natives declared in MeshComponent also serve SkeletalMeshComponent), then "Func".
    fn native_lookup(&self, func: usize) -> Option<NativeFn> {
        let fname = self.name_of(self.funcs[func].name).to_ascii_lowercase();
        let mut c = Some(self.funcs[func].owner).filter(|&c| c != NONE_CLASS);
        while let Some(ci) = c {
            let key = format!("{}.{fname}", self.class_name(ci).to_ascii_lowercase());
            if let Some(f) = self.natives.get(&key) {
                return Some(*f);
            }
            c = self.classes[ci].super_;
        }
        self.natives.get(&fname).copied()
    }

    fn ret_zero(&mut self, func: usize) -> Value {
        match self.funcs[func].ret.clone() {
            Some(r) => self.zero(&r.ty, 1),
            None => Value::Int(0),
        }
    }

    /// Evaluate call arguments in the caller's frame, run, write back out params.
    fn call_with(&mut self, this: Option<ObjId>, func: usize, arg_exprs: &[Expr]) -> Value {
        let params = self.funcs[func].params.clone();
        let display = self.funcs[func].display.as_str();
        // short-circuit operators (`skip` params)
        if display == "Object.AndAnd_BoolBool" || display == "Object.OrOr_BoolBool" {
            let and = display.starts_with("Object.And");
            let a = arg_exprs.first().map(|e| self.eval(e, self.this())).unwrap_or(Value::Bool(false)).truthy();
            if and != a {
                return Value::Bool(a);
            }
            let b = arg_exprs.get(1).map(|e| self.eval(e, self.this())).unwrap_or(Value::Bool(false)).truthy();
            return Value::Bool(b);
        }
        let mut args = Vec::with_capacity(params.len());
        let mut passed = Vec::with_capacity(params.len());
        let mut outs = vec![];
        let fthis = self.this();
        for (i, e) in arg_exprs.iter().enumerate() {
            if *e == Expr::EmptyParmValue {
                args.push(Value::Int(0));
                passed.push(false);
                continue;
            }
            let is_out = params.get(i).is_some_and(|p| p.flags & CPF_OUT_PARM != 0);
            if is_out {
                let pl = self.place(e, fthis);
                args.push(self.read(&pl));
                outs.push((i, pl));
            } else {
                args.push(self.eval(e, fthis));
            }
            passed.push(true);
        }
        let (ret, finals) = self.invoke(this, func, args, passed);
        for (i, pl) in outs {
            if let Some(v) = finals.get(i) {
                self.write(&pl, v.clone());
            }
        }
        ret
    }

    fn call_virtual(&mut self, ctx: Option<ObjId>, name: Sym, args: &[Expr], global: bool) -> Value {
        let Some(o) = ctx else {
            let f = self.cur_func();
            let n = self.name_of(name).to_string();
            self.missing(format!("Accessed None calling {n} in {f}"));
            return Value::Int(0);
        };
        let class = self.obj(o).class;
        let state = if global { None } else { self.obj(o).state };
        match self.find_func(class, name, state) {
            Some(f) => self.call_with(Some(o), f, args),
            None => {
                let n = format!("{}.{}", self.class_name(class), self.name_of(name));
                // still evaluate arguments for their side effects
                for a in args {
                    let t = self.this();
                    self.eval(a, t);
                }
                self.missing(format!("function {n} not found"));
                Value::Int(0)
            }
        }
    }

    // --- statements ---------------------------------------------------------------------------

    fn index_of(code: &[Stmt], mem: u32) -> usize {
        code.binary_search_by_key(&mem, |s| s.mem).unwrap_or_else(|i| i)
    }

    /// Run statements from `pc`; Some(value) on an explicit return.
    fn run(&mut self, code: &Arc<Vec<Stmt>>, mut pc: usize, this: Option<ObjId>) -> Option<Value> {
        while pc < code.len() {
            if self.aborted {
                return None;
            }
            if self.budget == 0 {
                let f = self.cur_func();
                self.say(format!("statement budget exhausted in {f} (runaway loop?); stopping this event"));
                self.aborted = true;
                return None;
            }
            self.budget -= 1;
            match self.stmt(code, pc, this) {
                Flow::Next => pc += 1,
                Flow::Jump(i) => pc = i,
                Flow::Return(v) => return Some(v),
            }
        }
        None
    }

    fn stmt(&mut self, code: &Arc<Vec<Stmt>>, pc: usize, this: Option<ObjId>) -> Flow {
        let s = &code[pc];
        match &s.expr {
            Expr::Let(a, b) | Expr::LetBool(a, b) | Expr::LetDelegate(a, b) => {
                let pl = self.place(a, this);
                let v = self.eval(b, this);
                self.write(&pl, v);
                Flow::Next
            }
            Expr::Return(e) => {
                let v = self.eval(e, this);
                Flow::Return(v)
            }
            Expr::ReturnNothing(_) | Expr::EndOfScript | Expr::Stop => self.ret_local().map(Flow::Return).unwrap_or(Flow::Return(Value::Int(0))),
            Expr::Jump(off) => Flow::Jump(Self::index_of(code, *off as u32)),
            Expr::JumpIfNot(off, e) => {
                if self.eval(e, this).truthy() { Flow::Next } else { Flow::Jump(Self::index_of(code, *off as u32)) }
            }
            Expr::Switch { expr, .. } => {
                let v = self.eval(expr, this);
                let mut i = pc + 1;
                while i < code.len() {
                    match &code[i].expr {
                        Expr::Case { expr: None, .. } => return Flow::Jump(i + 1),
                        Expr::Case { next, expr: Some(c) } => {
                            let cv = self.eval(c, this);
                            if values_equal(&v, &cv) {
                                return Flow::Jump(i + 1);
                            }
                            i = Self::index_of(code, *next as u32);
                        }
                        _ => return Flow::Jump(i),
                    }
                }
                Flow::Jump(i)
            }
            Expr::Case { .. } | Expr::LabelTable(_) | Expr::Nothing | Expr::DefaultParmValue(..) => Flow::Next,
            Expr::Assert { line, expr, .. } => {
                if !self.eval(expr, this).truthy() {
                    let f = self.cur_func();
                    self.say(format!("assertion failed in {f} line {line}"));
                }
                Flow::Next
            }
            Expr::FilterEditorOnly(skip) => Flow::Jump(Self::index_of(code, s.mem + 3 + *skip as u32)),
            Expr::GotoLabel(_) => {
                let f = self.cur_func();
                self.missing(format!("state code (GotoLabel) in {f}"));
                Flow::Return(Value::Int(0))
            }
            Expr::Iterator(call, end) => {
                let end_i = Self::index_of(code, *end as u32);
                let (items, places) = self.iterator_items(call, this);
                if items.is_empty() {
                    return Flow::Jump(end_i + 1);
                }
                self.iter_assign(&places, &items[0]);
                if let Some(f) = self.frames.last_mut() {
                    f.iters.push(Iter { items, next: 1, places, body: pc + 1, end: end_i });
                }
                Flow::Next
            }
            Expr::DynArrayIterator { arr, item, has_index, index, skip } => {
                let end_i = Self::index_of(code, *skip as u32);
                let list = match self.eval(arr, this) {
                    Value::Array(a) => a,
                    _ => vec![],
                };
                let mut places = vec![self.place(item, this)];
                if *has_index != 0 {
                    places.push(self.place(index, this));
                }
                let items: Vec<Vec<Value>> = list.into_iter().enumerate().map(|(i, v)| vec![v, Value::Int(i as i32)]).collect();
                if items.is_empty() {
                    return Flow::Jump(end_i + 1);
                }
                self.iter_assign(&places, &items[0]);
                if let Some(f) = self.frames.last_mut() {
                    f.iters.push(Iter { items, next: 1, places, body: pc + 1, end: end_i });
                }
                Flow::Next
            }
            Expr::IteratorNext => {
                let Some(it) = self.frames.last_mut().and_then(|f| f.iters.last_mut()) else { return Flow::Next };
                if it.next < it.items.len() {
                    let row = it.items[it.next].clone();
                    it.next += 1;
                    let (places, body) = (it.places.clone(), it.body);
                    self.iter_assign(&places, &row);
                    Flow::Jump(body)
                } else {
                    let end = it.end;
                    self.frames.last_mut().unwrap().iters.pop();
                    Flow::Jump(end + 1)
                }
            }
            Expr::IteratorPop => {
                if let Some(f) = self.frames.last_mut() {
                    f.iters.pop();
                }
                Flow::Next
            }
            e => {
                self.eval(e, this);
                Flow::Next
            }
        }
    }

    fn ret_local(&self) -> Option<Value> {
        let f = self.frames.last()?;
        let r = self.funcs[f.func].ret.as_ref()?;
        f.locals.get(&r.name).cloned()
    }

    fn iter_assign(&mut self, places: &[Place], row: &[Value]) {
        for (p, v) in places.iter().zip(row) {
            self.write(p, v.clone());
        }
    }

    /// Items of a `foreach Iterator(...)`: rows of out-parameter values.
    fn iterator_items(&mut self, call: &Expr, this: Option<ObjId>) -> (Vec<Vec<Value>>, Vec<Place>) {
        let (ctx, inner) = match call {
            Expr::Context { obj, expr, .. } => (self.eval(obj, this).obj(), &**expr),
            e => (this, e),
        };
        let (func, args) = match inner {
            Expr::NativeFunction(i, a) => (self.native_func(*i), a),
            Expr::FinalFunction(r, a) => {
                let pkg = self.cur_pkg();
                (self.resolve(pkg, *r).and_then(|r| self.load_func(r)), a)
            }
            Expr::VirtualFunction(n, a) => {
                let y = self.sym(n);
                let c = ctx.map(|o| self.obj(o).class).unwrap_or(NONE_CLASS);
                (self.find_func(c, y, None), a)
            }
            _ => (None, &vec![]),
        };
        let Some(func) = func else {
            let f = self.cur_func();
            self.missing(format!("iterator in {f}"));
            return (vec![], vec![]);
        };
        let params = self.funcs[func].params.clone();
        let mut vals = vec![];
        let mut places = vec![];
        let fthis = self.this();
        for (i, e) in args.iter().enumerate() {
            if params.get(i).is_some_and(|p| p.flags & CPF_OUT_PARM != 0) {
                places.push(self.place(e, fthis));
                vals.push(Value::Obj(None));
            } else if *e == Expr::EmptyParmValue {
                vals.push(Value::Obj(None));
            } else {
                vals.push(self.eval(e, fthis));
            }
        }
        let display = self.funcs[func].display.clone();
        if self.trace_calls {
            let a: Vec<String> = vals.iter().map(|v| self.describe(v)).collect();
            let ind = "  ".repeat(self.depth.min(20));
            self.say(format!("{ind}-> foreach {display}({}) [native]", a.join(", ")));
        }
        let key = display.to_ascii_lowercase();
        let rows = match natives::iterator(self, &key, ctx, &vals) {
            Some(r) => r,
            None => {
                self.missing(format!("iterator {display}"));
                vec![]
            }
        };
        (rows, places)
    }

    // --- places ------------------------------------------------------------------------------

    pub fn place(&mut self, e: &Expr, ctx: Option<ObjId>) -> Place {
        match e {
            Expr::LocalVariable(r) | Expr::LocalOutVariable(r) => Place::Local(self.rsym(*r)),
            Expr::InstanceVariable(r) => {
                let y = self.rsym(*r);
                match ctx {
                    Some(o) => Place::Obj(o, y),
                    None => {
                        let f = self.cur_func();
                        let n = self.name_of(y).to_string();
                        self.missing(format!("Accessed None reading {n} in {f}"));
                        Place::Temp(Value::Int(0))
                    }
                }
            }
            Expr::DefaultVariable(r) | Expr::StateVariable(r) => {
                let y = self.rsym(*r);
                match ctx {
                    Some(o) => {
                        let c = self.obj(o).class;
                        let cdo = if matches!(self.obj(o).kind, Kind::ClassObj) { self.cdo(c) } else if c != NONE_CLASS { self.cdo(c) } else { o };
                        Place::Obj(cdo, y)
                    }
                    None => Place::Temp(Value::Int(0)),
                }
            }
            Expr::Context { obj, expr, .. } => match self.eval(obj, ctx).obj() {
                Some(o) => self.place(expr, Some(o)),
                None => {
                    let f = self.cur_func();
                    self.missing(format!("Accessed None in {f}"));
                    Place::Temp(Value::Int(0))
                }
            },
            Expr::ClassContext { obj, expr, .. } => {
                let v = self.eval(obj, ctx);
                match self.as_class(&v) {
                    Some(c) => {
                        let cdo = self.cdo(c);
                        self.place(expr, Some(cdo))
                    }
                    None => Place::Temp(Value::Int(0)),
                }
            }
            Expr::ArrayElement(i, a) | Expr::DynArrayElement(i, a) => {
                let fthis = self.this();
                let idx = self.eval(i, fthis).int();
                let inner = self.place(a, ctx);
                if idx < 0 {
                    let f = self.cur_func();
                    self.missing(format!("negative array index in {f}"));
                    return Place::Temp(Value::Int(0));
                }
                let zero = if matches!(e, Expr::DynArrayElement(..)) { self.elem_zero(a, ctx) } else { None };
                Place::Index(Box::new(inner), idx as usize, zero)
            }
            Expr::StructMember { prop, expr, .. } => {
                let y = self.rsym(*prop);
                let inner = self.place(expr, ctx);
                Place::Member(Box::new(inner), y)
            }
            Expr::BoolVariable(x) | Expr::InterfaceContext(x) => self.place(x, ctx),
            // `Array.Length = n` (out_Texts.Length = 0 empties an out array)
            Expr::DynArrayLength(a) => {
                let zero = self.elem_zero(a, ctx);
                let inner = self.place(a, ctx);
                Place::Length(Box::new(inner), zero)
            }
            other => Place::Temp(self.eval(other, ctx)),
        }
    }

    /// Zero element for a dynamic array expression (grows the array on out-of-range writes).
    fn elem_zero(&mut self, a: &Expr, ctx: Option<ObjId>) -> Option<Value> {
        let (r, class) = match a {
            Expr::LocalVariable(r) | Expr::LocalOutVariable(r) => {
                let y = self.rsym(*r);
                let f = self.frames.last()?.func;
                let ty = self.funcs[f].locals.iter().find(|l| l.name == y)?.ty.clone();
                return match ty {
                    Ty::Array(t) => Some(self.zero(&t, 1)),
                    _ => None,
                };
            }
            Expr::InstanceVariable(r) | Expr::DefaultVariable(r) => (*r, ctx.map(|o| self.obj(o).class)?),
            Expr::Context { obj, expr, .. } => {
                let o = self.eval(obj, ctx).obj();
                return self.elem_zero(expr, o);
            }
            Expr::StructMember { prop, .. } => {
                // look the member up in every loaded struct with that field name
                let y = self.rsym(*prop);
                let ty = self.structs.iter().flat_map(|s| s.fields.iter()).find(|f| f.name == y).map(|f| f.ty.clone())?;
                return match ty {
                    Ty::Array(t) => Some(self.zero(&t, 1)),
                    _ => None,
                };
            }
            _ => return None,
        };
        let y = self.rsym(r);
        let ty = self.find_prop(class, y)?.ty.clone();
        match ty {
            Ty::Array(t) => Some(self.zero(&t, 1)),
            _ => None,
        }
    }

    pub fn read(&mut self, p: &Place) -> Value {
        match p {
            Place::Local(y) => self.frames.last().and_then(|f| f.locals.get(y)).cloned().unwrap_or(Value::Int(0)),
            Place::Obj(o, y) => {
                // Object.Class/Name/Outer are native fields, not script storage
                if matches!(self.name_of(*y).to_ascii_lowercase().as_str(), "class" | "name" | "outer") {
                    return self.builtin_prop(*o, *y);
                }
                if let Some(v) = self.obj(*o).props.get(y).or_else(|| self.default_of(*o, *y)) {
                    return v.clone();
                }
                self.builtin_prop(*o, *y)
            }
            Place::Member(inner, y) => match self.read(inner) {
                Value::Struct(s) => s.field(*y).cloned().unwrap_or(Value::Int(0)),
                _ => Value::Int(0),
            },
            Place::Index(inner, i, zero) => match self.read(inner) {
                Value::Array(a) => match a.get(*i) {
                    Some(v) => v.clone(),
                    None => {
                        let f = self.cur_func();
                        self.missing(format!("array index out of bounds in {f}"));
                        zero.clone().unwrap_or(Value::Int(0))
                    }
                },
                _ => Value::Int(0),
            },
            Place::Length(inner, _) => match self.read(inner) {
                Value::Array(a) => Value::Int(a.len() as i32),
                _ => Value::Int(0),
            },
            Place::Temp(v) => v.clone(),
        }
    }

    /// Object built-ins that aren't in the property map (Object.Class/Name/Outer).
    fn builtin_prop(&mut self, o: ObjId, y: Sym) -> Value {
        match self.name_of(y).to_ascii_lowercase().as_str() {
            "class" => {
                let c = self.obj(o).class;
                if c == NONE_CLASS || matches!(self.obj(o).kind, Kind::ClassObj) {
                    return match self.class_by_name("Class") {
                        Some(cc) => Value::Obj(Some(self.class_object(cc))),
                        None => Value::Obj(None),
                    };
                }
                Value::Obj(Some(self.class_object(c)))
            }
            "name" => {
                let n = self.obj(o).name.clone();
                Value::Name(self.sym(&n))
            }
            "outer" => Value::Obj(self.obj(o).outer),
            _ => Value::Int(0),
        }
    }

    pub fn write(&mut self, p: &Place, v: Value) {
        if let Place::Temp(_) = p {
            return;
        }
        if let Place::Length(inner, zero) = p {
            let n = v.int().max(0) as usize;
            let z = zero.clone().unwrap_or(Value::Int(0));
            match self.slot_mut(inner) {
                Some(Value::Array(a)) => a.resize(n, z),
                // an unset array property: make it
                Some(slot) => *slot = Value::Array(vec![z; n]),
                None => {}
            }
            return;
        }
        if let Some(slot) = self.slot_mut(p) {
            let v = match slot {
                Value::Float(_) => Value::Float(v.float()),
                Value::Int(_) if !matches!(v, Value::Int(_)) && matches!(v, Value::Byte(_) | Value::Float(_) | Value::Bool(_)) => Value::Int(v.int()),
                Value::Byte(_) if matches!(v, Value::Int(_)) => Value::Byte(v.int() as u8),
                Value::Bool(_) if !matches!(v, Value::Bool(_)) => Value::Bool(v.truthy()),
                _ => v,
            };
            *slot = v;
        }
    }

    fn slot_mut(&mut self, p: &Place) -> Option<&mut Value> {
        match p {
            Place::Local(y) => {
                let f = self.frames.last_mut()?;
                Some(f.locals.entry(*y).or_insert(Value::Int(0)))
            }
            Place::Obj(o, y) => {
                if !self.obj(*o).props.contains_key(y) {
                    // copy-on-write from the class defaults (light objects)
                    let d = self.default_of(*o, *y).cloned().unwrap_or(Value::Int(0));
                    self.objects[*o as usize].props.insert(*y, d);
                }
                self.objects[*o as usize].props.get_mut(y)
            }
            Place::Member(inner, y) => match self.slot_mut(inner)? {
                Value::Struct(s) => Some(s.field_mut(*y)),
                _ => None,
            },
            Place::Index(inner, i, zero) => {
                let zero = zero.clone();
                match self.slot_mut(inner)? {
                    Value::Array(a) => {
                        if *i >= a.len() {
                            if let Some(z) = zero {
                                a.resize(*i + 1, z);
                            }
                        }
                        a.get_mut(*i)
                    }
                    _ => None,
                }
            }
            Place::Temp(_) | Place::Length(..) => None,
        }
    }

    // --- expressions ----------------------------------------------------------------------------

    pub fn eval(&mut self, e: &Expr, ctx: Option<ObjId>) -> Value {
        use Expr::*;
        match e {
            LocalVariable(_) | LocalOutVariable(_) | InstanceVariable(_) | DefaultVariable(_) | StateVariable(_) | ArrayElement(..)
            | DynArrayElement(..) | StructMember { .. } | BoolVariable(_) => {
                let p = self.place(e, ctx);
                self.read(&p)
            }
            Context { obj, expr, .. } => match self.eval(obj, ctx).obj() {
                Some(o) => self.eval(expr, Some(o)),
                None => {
                    let f = self.cur_func();
                    self.missing(format!("Accessed None in {f}"));
                    Value::Int(0)
                }
            },
            ClassContext { obj, expr, .. } => {
                let v = self.eval(obj, ctx);
                match self.as_class(&v) {
                    Some(c) => {
                        let cdo = self.cdo(c);
                        self.eval(expr, Some(cdo))
                    }
                    None => {
                        let f = self.cur_func();
                        self.missing(format!("Accessed None (class) in {f}"));
                        Value::Int(0)
                    }
                }
            }
            InterfaceContext(x) | EatReturnValue(_, x) | Skip(_, x) => self.eval(x, ctx),
            ReturnNothing(_) => self.ret_local().unwrap_or(Value::Int(0)),
            VirtualFunction(n, a) | GlobalFunction(n, a) => {
                let y = self.sym(n);
                self.call_virtual(ctx, y, a, matches!(e, GlobalFunction(..)))
            }
            FinalFunction(r, a) => {
                let pkg = self.cur_pkg();
                match self.resolve(pkg, *r).and_then(|fr| self.load_func(fr)) {
                    Some(f) => self.call_with(ctx, f, a),
                    None => {
                        let n = self.reg.packages[pkg].obj_path(*r);
                        self.missing(format!("function {n} (unresolved)"));
                        Value::Int(0)
                    }
                }
            }
            NativeFunction(i, a) => match self.native_func(*i) {
                Some(f) => self.call_with(ctx, f, a),
                None => {
                    self.missing(format!("native index {i}"));
                    Value::Int(0)
                }
            },
            DelegateFunction { local, prop, name, args } => {
                let y = self.rsym(*prop);
                let pl = if *local != 0 { Place::Local(y) } else { ctx.map(|o| Place::Obj(o, y)).unwrap_or(Place::Temp(Value::Int(0))) };
                let target = match self.read(&pl) {
                    Value::Delegate(Some(o), f) if f.0 != 0 => Some((o, f)),
                    _ => None,
                };
                let ny = self.sym(name);
                match target {
                    Some((o, f)) => self.call_virtual(Some(o), f, args, false),
                    None => self.call_virtual(ctx, ny, args, false),
                }
            }
            DelegateProperty(n, prop) => {
                if *prop != 0 {
                    let y = self.rsym(*prop);
                    if let Some(o) = ctx {
                        return self.read(&Place::Obj(o, y));
                    }
                }
                let y = self.sym(n);
                Value::Delegate(self.this(), y)
            }
            InstanceDelegate(n) => {
                let y = self.sym(n);
                Value::Delegate(ctx, y)
            }
            EmptyDelegate => Value::Delegate(None, Sym(0)),
            EqualEqualDelDel(a, b) | EqualEqualDelFunc(a, b) | NotEqualDelDel(a, b) | NotEqualDelFunc(a, b) => {
                let t = self.this();
                let (x, y) = (self.eval(a, t), self.eval(b, t));
                let eq = values_equal(&x, &y);
                Value::Bool(if matches!(e, EqualEqualDelDel(..) | EqualEqualDelFunc(..)) { eq } else { !eq })
            }
            IntConst(i) => Value::Int(*i),
            FloatConst(f) => Value::Float(*f),
            StringConst(s) | UnicodeStringConst(s) => Value::Str(s.clone()),
            ObjectConst(r) => {
                let pkg = self.cur_pkg();
                Value::Obj(self.obj_for_ref(pkg, *r))
            }
            NameConst(n) => Value::Name(self.sym(n)),
            RotationConst(r) => self.rotator(r[0], r[1], r[2]),
            VectorConst(v) => self.vector(v[0], v[1], v[2]),
            ByteConst(b) => Value::Byte(*b),
            IntConstByte(b) => Value::Int(*b as i32),
            IntZero => Value::Int(0),
            IntOne => Value::Int(1),
            True => Value::Bool(true),
            False => Value::Bool(false),
            NoObject => Value::Obj(None),
            SelfObj => Value::Obj(self.this()),
            Nothing | EmptyParmValue | EndFunctionParms | EndParmValue => Value::Int(0),
            New { outer, name, class, template, .. } => {
                let t = self.this();
                let outer = self.eval(outer, t).obj();
                let name = self.eval(name, t);
                let cv = self.eval(class, t);
                let tmpl = self.eval(template, t).obj();
                let Some(c) = self.as_class(&cv) else {
                    let f = self.cur_func();
                    self.missing(format!("new with no class in {f}"));
                    return Value::Obj(None);
                };
                let n = match name {
                    Value::Str(s) if !s.is_empty() => Some(s),
                    _ => None,
                };
                let o = self.spawn_object(c, n);
                self.obj_mut(o).outer = outer;
                if let Some(tp) = tmpl {
                    let props = self.obj(tp).props.clone();
                    self.obj_mut(o).props.extend(props);
                }
                if self.trace_calls {
                    let ind = "  ".repeat(self.depth.min(20));
                    let cn = self.class_name(c).to_string();
                    let on = self.obj(o).name.clone();
                    self.say(format!("{ind}new {cn} -> {on}"));
                }
                Value::Obj(Some(o))
            }
            DynamicCast(r, x) | InterfaceCast(r, x) => {
                let v = self.eval(x, ctx);
                let Some(o) = v.obj() else { return Value::Obj(None) };
                let pkg = self.cur_pkg();
                let target = self.resolve(pkg, *r).and_then(|cr| self.load_class(cr));
                // a class from a package that isn't loaded (DLC) can't have instances here
                let Some(t) = target else { return Value::Obj(None) };
                let oc = self.obj(o).class;
                if matches!(e, InterfaceCast(..)) || self.classes[t].is_interface {
                    return if self.implements(oc, t) { v } else { Value::Obj(None) };
                }
                let ok = match self.obj(o).kind {
                    Kind::ClassObj => matches!(self.class_name(t), "Class" | "Object" | "State" | "Struct" | "Field"),
                    _ => self.is_child_of(oc, t),
                };
                if ok { v } else { Value::Obj(None) }
            }
            MetaCast(r, x) => {
                let v = self.eval(x, ctx);
                let pkg = self.cur_pkg();
                let target = self.resolve(pkg, *r).and_then(|cr| self.load_class(cr));
                match (self.as_class(&v), target) {
                    (Some(c), Some(t)) if self.is_child_of(c, t) => v,
                    _ => Value::Obj(None),
                }
            }
            PrimitiveCast(t, x) => {
                let v = self.eval(x, ctx);
                self.prim_cast(*t, v)
            }
            Conditional { cond, a, b, .. } => {
                if self.eval(cond, ctx).truthy() { self.eval(a, ctx) } else { self.eval(b, ctx) }
            }
            StructCmpEq(_, a, b) | StructCmpNe(_, a, b) => {
                let (x, y) = (self.eval(a, ctx), self.eval(b, ctx));
                let eq = values_equal(&x, &y);
                Value::Bool(if matches!(e, StructCmpEq(..)) { eq } else { !eq })
            }
            DynArrayLength(a) => match self.eval(a, ctx) {
                Value::Array(v) => Value::Int(v.len() as i32),
                _ => Value::Int(0),
            },
            DynArrayAdd { arr, count } => {
                let t = self.this();
                let n = self.eval(count, t).int().max(0) as usize;
                let zero = self.elem_zero(arr, ctx).unwrap_or(Value::Int(0));
                let pl = self.place(arr, ctx);
                match self.slot_mut(&pl) {
                    Some(Value::Array(a)) => {
                        let at = a.len();
                        a.resize(at + n, zero);
                        Value::Int(at as i32)
                    }
                    _ => Value::Int(-1),
                }
            }
            DynArrayAddItem { arr, item, .. } => {
                let t = self.this();
                let v = self.eval(item, t);
                let pl = self.place(arr, ctx);
                match self.slot_mut(&pl) {
                    Some(Value::Array(a)) => {
                        a.push(v);
                        Value::Int(a.len() as i32 - 1)
                    }
                    _ => Value::Int(-1),
                }
            }
            DynArrayInsertItem { arr, index, item, .. } => {
                let t = self.this();
                let i = self.eval(index, t).int().max(0) as usize;
                let v = self.eval(item, t);
                let pl = self.place(arr, ctx);
                if let Some(Value::Array(a)) = self.slot_mut(&pl) {
                    let i = i.min(a.len());
                    a.insert(i, v);
                    return Value::Int(i as i32);
                }
                Value::Int(-1)
            }
            DynArrayInsert(arr, index, count) => {
                let t = self.this();
                let i = self.eval(index, t).int().max(0) as usize;
                let n = self.eval(count, t).int().max(0) as usize;
                let zero = self.elem_zero(arr, ctx).unwrap_or(Value::Int(0));
                let pl = self.place(arr, ctx);
                if let Some(Value::Array(a)) = self.slot_mut(&pl) {
                    let i = i.min(a.len());
                    for _ in 0..n {
                        a.insert(i, zero.clone());
                    }
                }
                Value::Int(0)
            }
            DynArrayRemove(arr, index, count) => {
                let t = self.this();
                let i = self.eval(index, t).int().max(0) as usize;
                let n = self.eval(count, t).int().max(0) as usize;
                let pl = self.place(arr, ctx);
                if let Some(Value::Array(a)) = self.slot_mut(&pl) {
                    let end = (i + n).min(a.len());
                    if i < end {
                        a.drain(i..end);
                    }
                }
                Value::Int(0)
            }
            DynArrayRemoveItem { arr, item, .. } => {
                let t = self.this();
                let v = self.eval(item, t);
                let pl = self.place(arr, ctx);
                if let Some(Value::Array(a)) = self.slot_mut(&pl) {
                    a.retain(|x| !values_equal(x, &v));
                }
                Value::Int(0)
            }
            DynArrayFind { arr, value, .. } => {
                let t = self.this();
                let v = self.eval(value, t);
                match self.eval(arr, ctx) {
                    Value::Array(a) => Value::Int(a.iter().position(|x| values_equal(x, &v)).map(|i| i as i32).unwrap_or(-1)),
                    _ => Value::Int(-1),
                }
            }
            DynArrayFindStruct { arr, member, value, .. } => {
                let t = self.this();
                let m = self.eval(member, t);
                let v = self.eval(value, t);
                let Value::Name(my) = m else { return Value::Int(-1) };
                match self.eval(arr, ctx) {
                    Value::Array(a) => Value::Int(
                        a.iter()
                            .position(|x| match x {
                                Value::Struct(s) => s.field(my).is_some_and(|f| values_equal(f, &v)),
                                _ => false,
                            })
                            .map(|i| i as i32)
                            .unwrap_or(-1),
                    ),
                    _ => Value::Int(-1),
                }
            }
            DynArraySort { arr, delegate, .. } => {
                // UE3: the delegate returns < 0 when A and B are out of order (B goes first)
                let t = self.this();
                let d = self.eval(delegate, t);
                let Value::Delegate(target, fname) = d else { return Value::Int(0) };
                let target = target.or(t);
                let pl = self.place(arr, ctx);
                let Value::Array(mut items) = self.read(&pl) else { return Value::Int(0) };
                let Some(o) = target else { return Value::Int(0) };
                let class = self.obj(o).class;
                let Some(f) = self.find_func(class, fname, None) else {
                    let n = self.name_of(fname).to_string();
                    self.missing(format!("sort delegate {n}"));
                    return Value::Int(0);
                };
                // insertion sort: stable, and the compare runs script
                for i in 1..items.len() {
                    let mut j = i;
                    while j > 0 {
                        let (r, _) = self.invoke(Some(o), f, vec![items[j - 1].clone(), items[j].clone()], vec![true, true]);
                        if r.int() < 0 {
                            items.swap(j - 1, j);
                            j -= 1;
                        } else {
                            break;
                        }
                    }
                }
                self.write(&pl, Value::Array(items));
                Value::Int(0)
            }
            Let(..) | LetBool(..) | LetDelegate(..) | Return(_) | Jump(_) | JumpIfNot(..) | Switch { .. } | Case { .. } | Stop | Assert { .. }
            | LabelTable(_) | GotoLabel(_) | Iterator(..) | IteratorPop | IteratorNext | DynArrayIterator { .. } | DefaultParmValue(..)
            | FilterEditorOnly(_) | EndOfScript | NativeParm(_) => {
                let f = self.cur_func();
                self.missing(format!("statement token used as expression in {f}"));
                Value::Int(0)
            }
        }
    }

    pub fn vector(&mut self, x: f32, y: f32, z: f32) -> Value {
        let (xs, ys, zs, n) = (self.sym("X"), self.sym("Y"), self.sym("Z"), self.sym("Vector"));
        Value::Struct(Box::new(StructVal { def: None, name: n, fields: vec![(xs, Value::Float(x)), (ys, Value::Float(y)), (zs, Value::Float(z))] }))
    }

    pub fn rotator(&mut self, p: i32, y: i32, r: i32) -> Value {
        let (ps, ys, rs, n) = (self.sym("Pitch"), self.sym("Yaw"), self.sym("Roll"), self.sym("Rotator"));
        Value::Struct(Box::new(StructVal { def: None, name: n, fields: vec![(ps, Value::Int(p)), (ys, Value::Int(y)), (rs, Value::Int(r))] }))
    }

    pub fn vec3(&mut self, v: &Value) -> [f32; 3] {
        let (xs, ys, zs) = (self.sym("X"), self.sym("Y"), self.sym("Z"));
        match v {
            Value::Struct(s) => [xs, ys, zs].map(|k| s.field(k).map(|f| f.float()).unwrap_or(0.0)),
            _ => [0.0; 3],
        }
    }

    pub fn rot3(&mut self, v: &Value) -> [i32; 3] {
        let (a, b, c) = (self.sym("Pitch"), self.sym("Yaw"), self.sym("Roll"));
        match v {
            Value::Struct(s) => [a, b, c].map(|k| s.field(k).map(|f| f.int()).unwrap_or(0)),
            _ => [0; 3],
        }
    }

    pub fn to_string(&mut self, v: &Value) -> String {
        match v {
            Value::Name(y) => self.name_of(*y).to_string(),
            Value::Obj(Some(o)) => self.obj(*o).name.clone(),
            Value::Obj(None) => "None".into(),
            Value::Struct(s) if self.name_of(s.name).eq_ignore_ascii_case("Vector") => {
                let a = self.vec3(v);
                format!("{:.2},{:.2},{:.2}", a[0], a[1], a[2])
            }
            Value::Struct(s) if self.name_of(s.name).eq_ignore_ascii_case("Rotator") => {
                let a = self.rot3(v);
                format!("{},{},{}", a[0], a[1], a[2])
            }
            other => other.to_string(),
        }
    }

    fn prim_cast(&mut self, t: u8, v: Value) -> Value {
        match upk::script::cast_name(t) {
            "ByteToInt" | "BoolToInt" | "FloatToInt" => Value::Int(v.int()),
            "StringToInt" => Value::Int(v.str().trim().parse::<f32>().map(|f| f as i32).unwrap_or(0)),
            "IntToByte" | "BoolToByte" | "FloatToByte" => Value::Byte(v.int() as u8),
            "StringToByte" => Value::Byte(v.str().trim().parse::<i32>().unwrap_or(0) as u8),
            "ByteToFloat" | "IntToFloat" | "BoolToFloat" => Value::Float(v.float()),
            "StringToFloat" => Value::Float(v.str().trim().parse().unwrap_or(0.0)),
            "ByteToBool" | "IntToBool" | "FloatToBool" | "ObjectToBool" | "InterfaceToBool" => Value::Bool(v.truthy()),
            "NameToBool" => Value::Bool(matches!(v, Value::Name(y) if !self.name_of(y).eq_ignore_ascii_case("None"))),
            "StringToBool" => Value::Bool(v.str().eq_ignore_ascii_case("true") || v.str().parse::<i32>().is_ok_and(|i| i != 0)),
            "VectorToBool" => Value::Bool(self.vec3(&v) != [0.0; 3]),
            "RotatorToBool" => Value::Bool(self.rot3(&v) != [0; 3]),
            "ByteToString" | "IntToString" | "BoolToString" | "FloatToString" | "ObjectToString" | "NameToString" | "VectorToString"
            | "RotatorToString" | "InterfaceToString" | "DelegateToString" => Value::Str(self.to_string(&v)),
            "StringToName" => {
                let s = v.str().to_string();
                Value::Name(self.sym(&s))
            }
            "InterfaceToObject" | "ObjectToInterface" => v,
            "RotatorToVector" => {
                let r = self.rot3(&v);
                let (p, y) = (r[0] as f32 * std::f32::consts::PI / 32768.0, r[1] as f32 * std::f32::consts::PI / 32768.0);
                self.vector(p.cos() * y.cos(), p.cos() * y.sin(), p.sin())
            }
            "VectorToRotator" => {
                let a = self.vec3(&v);
                let yaw = (a[1].atan2(a[0]) * 32768.0 / std::f32::consts::PI) as i32;
                let pitch = (a[2].atan2((a[0] * a[0] + a[1] * a[1]).sqrt()) * 32768.0 / std::f32::consts::PI) as i32;
                self.rotator(pitch, yaw, 0)
            }
            other => {
                self.missing(format!("cast {other}"));
                v
            }
        }
    }
}

pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Float(_), _) | (_, Value::Float(_)) => a.float() == b.float(),
        (Value::Int(_) | Value::Byte(_) | Value::Bool(_), Value::Int(_) | Value::Byte(_) | Value::Bool(_)) => a.int() == b.int(),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Struct(x), Value::Struct(y)) => {
            x.fields.len() == y.fields.len() && x.fields.iter().all(|(k, v)| y.field(*k).is_some_and(|w| values_equal(v, w)))
        }
        _ => a == b,
    }
}

/// Convert a value to a declared type (the compiler casts most, natives may not).
pub fn coerce(v: Value, ty: &Ty) -> Value {
    match (ty, &v) {
        (Ty::Float, Value::Int(_) | Value::Byte(_)) => Value::Float(v.float()),
        (Ty::Int, Value::Float(_) | Value::Byte(_) | Value::Bool(_)) => Value::Int(v.int()),
        (Ty::Byte(_), Value::Int(_) | Value::Float(_)) => Value::Byte(v.int() as u8),
        (Ty::Bool, Value::Int(_) | Value::Byte(_)) => Value::Bool(v.truthy()),
        _ => v,
    }
}

impl Vm {
    /// Start an object's state code at a label (GotoState); no code or label = nothing runs.
    pub fn start_state_code(&mut self, o: ObjId, state: Sym, label: Sym) {
        self.state_runs.remove(&o);
        let mut c = Some(self.obj(o).class);
        let mut code_ref = None;
        while let Some(ci) = c {
            if ci == NONE_CLASS {
                break;
            }
            if let Some(sd) = self.classes[ci].states.get(&state) {
                code_ref = Some(sd.code);
                break;
            }
            c = self.classes[ci].super_;
        }
        let Some(func) = code_ref.and_then(|r| self.load_func(r)) else { return };
        if let Some(pc) = self.label_index(func, label) {
            self.state_runs.insert(o, StateRun { func, pc, sleep: 0.0, wait: None });
        }
    }

    fn label_index(&self, func: usize, label: Sym) -> Option<usize> {
        let code = &self.funcs[func].code;
        let want = self.name_of(label).to_ascii_lowercase();
        code.iter().find_map(|s| match &s.expr {
            Expr::LabelTable(v) => v.iter().find(|(n, _)| n.to_ascii_lowercase() == want).map(|(_, off)| Self::index_of(code, *off)),
            _ => None,
        })
    }

    /// Run every object's state code until it ends, sleeps (latent Sleep) or switches state.
    pub fn tick_states(&mut self, dt: f32) {
        let ids: Vec<ObjId> = self.state_runs.keys().copied().collect();
        for o in ids {
            let Some(mut run) = self.state_runs.remove(&o) else { continue };
            if self.obj(o).destroyed {
                continue;
            }
            if run.sleep > 0.0 {
                run.sleep -= dt;
                if run.sleep > 1e-4 {
                    self.state_runs.insert(o, run);
                    continue;
                }
            }
            if let Some(mut w) = run.wait.take() {
                if !self.latent_done(o, &mut w, dt) {
                    run.wait = Some(w);
                    self.state_runs.insert(o, run);
                    continue;
                }
            }
            self.budget = BUDGET;
            self.aborted = false;
            self.latent_sleep = None;
            self.latent = None;
            let code = self.funcs[run.func].code.clone();
            self.frames.push(Frame { func: run.func, this: Some(o), locals: HashMap::new(), iters: vec![] });
            self.depth += 1;
            let mut pc = run.pc;
            let mut keep = false;
            for _ in 0..10_000 {
                if pc >= code.len() || self.aborted {
                    break;
                }
                let flow = match &code[pc].expr {
                    Expr::Stop | Expr::EndOfScript | Expr::LabelTable(_) => break,
                    Expr::GotoLabel(e) => {
                        let l = self.eval(e, Some(o));
                        let name = match l {
                            Value::Name(y) => y,
                            Value::Str(s) => self.sym(&s),
                            _ => break,
                        };
                        match self.label_index(run.func, name) {
                            Some(i) => Flow::Jump(i),
                            None => break,
                        }
                    }
                    _ => self.stmt(&code, pc, Some(o)),
                };
                match flow {
                    Flow::Next => pc += 1,
                    Flow::Jump(i) => pc = i,
                    Flow::Return(_) => break,
                }
                // GotoState inside the code started a new run
                if self.state_runs.contains_key(&o) {
                    break;
                }
                if let Some(t) = self.latent_sleep.take() {
                    run.pc = pc;
                    run.sleep = t;
                    keep = true;
                    break;
                }
                if let Some(w) = self.latent.take() {
                    run.pc = pc;
                    run.wait = Some(w);
                    keep = true;
                    break;
                }
            }
            self.depth -= 1;
            self.frames.pop();
            if keep && !self.state_runs.contains_key(&o) {
                self.state_runs.insert(o, run);
            }
        }
    }
}

impl Vm {
    /// Whether a latent wait finished (the pawn arrived, landed, or the wait timed out).
    fn latent_done(&mut self, ctrl: ObjId, w: &mut Latent, dt: f32) -> bool {
        let pawn = self.get(ctrl, "Pawn").obj();
        match w {
            Latent::Wait { left } => {
                *left -= dt;
                *left <= 0.0
            }
            Latent::Land { left } => {
                *left -= dt;
                let falling = pawn.is_some_and(|p| self.get(p, "Physics").int() == 2);
                !falling || *left <= 0.0
            }
            Latent::Move { dest, target, dist, left, .. } => {
                *left -= dt;
                if *left <= 0.0 {
                    return true;
                }
                let Some(p) = pawn else { return true };
                if self.obj(p).destroyed || target.is_some_and(|t| self.obj(t).destroyed) {
                    return true;
                }
                let goal = match *target {
                    Some(t) => {
                        let l = self.get(t, "Location");
                        self.vec3(&l)
                    }
                    None => *dest,
                };
                let l = self.get(p, "Location");
                let at = self.vec3(&l);
                let flat = ((goal[0] - at[0]).powi(2) + (goal[1] - at[1]).powi(2)).sqrt();
                flat <= dist.max(40.0)
            }
        }
    }

    /// Where a controller's state code is walking its pawn: (goal, walk), Unreal units.
    pub fn move_goal(&mut self, ctrl: ObjId) -> Option<([f32; 3], bool)> {
        let (dest, target, walk) = match self.state_runs.get(&ctrl)?.wait.as_ref()? {
            Latent::Move { dest, target, walk, .. } => (*dest, *target, *walk),
            _ => return None,
        };
        match target {
            Some(t) => {
                let l = self.get(t, "Location");
                Some((self.vec3(&l), walk))
            }
            None => Some((dest, walk)),
        }
    }
}
