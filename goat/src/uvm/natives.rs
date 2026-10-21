//! Rust implementations of UnrealScript natives (`native function` / `native(N)`), keyed by
//! "class.function" in lower case (or just "function" for ones shared by many classes).
//! Core operators are handled generically from their names (`Add_IntInt`, `Subtract_PreFloat`).

use super::exec::{Call, values_equal};
use super::*;

pub fn register(vm: &mut Vm) {
    let n = &mut vm.natives;
    let mut add = |k: &str, f: NativeFn| {
        n.insert(k.to_ascii_lowercase(), f);
    };
    // logging
    add("object.loginternal", |vm, c| {
        let s = vm.to_string(c.arg(0));
        let tag = if c.given(1) { vm.to_string(c.arg(1)) } else { "ScriptLog".into() };
        vm.say(format!("LOG {tag}: {s}"));
        Value::Int(0)
    });
    add("object.warninternal", |vm, c| {
        let s = vm.to_string(c.arg(0));
        vm.say(format!("WARN: {s}"));
        Value::Int(0)
    });
    add("object.scripttrace", |vm, _| {
        let stack: Vec<String> = vm.frames().iter().map(|f| vm.funcs[f.func].display.clone()).collect();
        vm.say(format!("ScriptTrace: {}", stack.join(" <- ")));
        Value::Int(0)
    });
    // math
    add("object.abs", |_, c| Value::Float(c.float(0).abs()));
    add("object.sin", |_, c| Value::Float(c.float(0).sin()));
    add("object.cos", |_, c| Value::Float(c.float(0).cos()));
    add("object.tan", |_, c| Value::Float(c.float(0).tan()));
    add("object.asin", |_, c| Value::Float(c.float(0).asin()));
    add("object.acos", |_, c| Value::Float(c.float(0).acos()));
    add("object.atan", |_, c| Value::Float(c.float(0).atan()));
    add("object.atan2", |_, c| Value::Float(c.float(0).atan2(c.float(1))));
    add("object.exp", |_, c| Value::Float(c.float(0).exp()));
    add("object.loge", |_, c| Value::Float(c.float(0).ln()));
    add("object.sqrt", |_, c| Value::Float(c.float(0).sqrt()));
    add("object.square", |_, c| Value::Float(c.float(0) * c.float(0)));
    add("object.frand", |_, _| Value::Float(rand01()));
    add("object.rand", |_, c| Value::Int(if c.int(0) > 0 { (rand01() * c.int(0) as f32) as i32 % c.int(0) } else { 0 }));
    add("object.min", |_, c| Value::Int(c.int(0).min(c.int(1))));
    add("object.max", |_, c| Value::Int(c.int(0).max(c.int(1))));
    add("object.clamp", |_, c| Value::Int(c.int(0).max(c.int(1)).min(c.int(2))));
    add("object.fmin", |_, c| Value::Float(c.float(0).min(c.float(1))));
    add("object.fmax", |_, c| Value::Float(c.float(0).max(c.float(1))));
    add("object.fclamp", |_, c| Value::Float(c.float(0).max(c.float(1)).min(c.float(2))));
    add("object.lerp", |_, c| Value::Float(c.float(0) + (c.float(1) - c.float(0)) * c.float(2)));
    add("object.ffloor", |_, c| Value::Float(c.float(0).floor()));
    add("object.fceil", |_, c| Value::Float(c.float(0).ceil()));
    add("object.round", |_, c| Value::Int(c.float(0).round() as i32));
    add("object.randrange", |_, c| Value::Float(c.float(0) + (c.float(1) - c.float(0)) * rand01()));
    add("object.finterpto", |_, c| {
        let (cur, target, dt, speed) = (c.float(0), c.float(1), c.float(2), c.float(3));
        if speed <= 0.0 {
            return Value::Float(target);
        }
        let d = target - cur;
        if d * d < 1e-8 {
            return Value::Float(target);
        }
        Value::Float(cur + d * (dt * speed).clamp(0.0, 1.0))
    });
    // vectors
    add("object.vsize", |vm, c| {
        let a = vm.vec3(c.arg(0));
        Value::Float((a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt())
    });
    add("object.vsize2d", |vm, c| {
        let a = vm.vec3(c.arg(0));
        Value::Float((a[0] * a[0] + a[1] * a[1]).sqrt())
    });
    add("object.vsizesq", |vm, c| {
        let a = vm.vec3(c.arg(0));
        Value::Float(a[0] * a[0] + a[1] * a[1] + a[2] * a[2])
    });
    add("object.normal", |vm, c| {
        let a = vm.vec3(c.arg(0));
        let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        if l < 1e-8 { vm.vector(0.0, 0.0, 0.0) } else { vm.vector(a[0] / l, a[1] / l, a[2] / l) }
    });
    add("object.vrand", |vm, _| {
        let v = [rand01() * 2.0 - 1.0, rand01() * 2.0 - 1.0, rand01() * 2.0 - 1.0];
        let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
        vm.vector(v[0] / l, v[1] / l, v[2] / l)
    });
    add("object.vect", |vm, c| vm.vector(c.float(0), c.float(1), c.float(2)));
    add("object.rot", |vm, c| vm.rotator(c.int(0), c.int(1), c.int(2)));
    // strings
    add("object.len", |_, c| Value::Int(c.str(0).chars().count() as i32));
    // InStr(S, T, optional bSearchFromRight, optional bIgnoreCase, optional StartPos), as
    // FString::InStr: forward from StartPos (clamped), or backward with T ending before StartPos
    // (default: the end); character indices
    add("object.instr", |_, c| {
        let (s, t) = (c.str(0), c.str(1));
        if std::env::var_os("UVM_OLD_INSTR").is_some() {
            let (s, t) = if c.arg(2).truthy() { (s, t) } else { (s.to_lowercase(), t.to_lowercase()) };
            return Value::Int(s.find(&t).map(|i| s[..i].chars().count() as i32).unwrap_or(-1));
        }
        let from_right = c.arg(2).truthy();
        let (s, t) = if c.arg(3).truthy() { (s.to_lowercase(), t.to_lowercase()) } else { (s, t) };
        let s: Vec<char> = s.chars().collect();
        let t: Vec<char> = t.chars().collect();
        let start = if c.given(4) { Some(c.int(4)) } else { None };
        let at = |i: usize| s.len() >= i + t.len() && s[i..i + t.len()] == t[..];
        let found = if !from_right {
            let from = start.map(|p| p.clamp(0, (s.len() as i32 - 1).max(0)) as usize).unwrap_or(0);
            (from..s.len().max(1)).find(|&i| at(i)).filter(|_| !t.is_empty() || from <= s.len())
        } else {
            let start = start.unwrap_or(s.len() as i32);
            let first = start - t.len().max(1) as i32;
            (0..=first.max(-1)).rev().filter(|&i| i >= 0).map(|i| i as usize).find(|&i| at(i))
        };
        Value::Int(found.map(|i| i as i32).unwrap_or(-1))
    });
    add("object.mid", |_, c| {
        let s: Vec<char> = c.str(0).chars().collect();
        let i = c.int(1).max(0) as usize;
        let n = if c.given(2) { c.int(2).max(0) as usize } else { usize::MAX };
        Value::Str(s.iter().skip(i).take(n).collect())
    });
    add("object.left", |_, c| Value::Str(c.str(0).chars().take(c.int(1).max(0) as usize).collect()));
    add("object.right", |_, c| {
        let s: Vec<char> = c.str(0).chars().collect();
        let n = (c.int(1).max(0) as usize).min(s.len());
        Value::Str(s[s.len() - n..].iter().collect())
    });
    add("object.caps", |_, c| Value::Str(c.str(0).to_uppercase()));
    add("object.locs", |_, c| Value::Str(c.str(0).to_lowercase()));
    add("object.chr", |_, c| Value::Str(char::from_u32(c.int(0) as u32).map(|ch| ch.to_string()).unwrap_or_default()));
    add("object.asc", |_, c| Value::Int(c.str(0).chars().next().map(|ch| ch as i32).unwrap_or(0)));
    add("object.repl", |_, c| {
        let (s, a, b) = (c.str(0), c.str(1), c.str(2));
        if a.is_empty() {
            return Value::Str(s);
        }
        if c.arg(3).truthy() { Value::Str(s.replace(&a, &b)) } else { Value::Str(replace_ci(&s, &a, &b)) }
    });
    add("object.split", |_, c| {
        // Split(Text, SplitStr, bOmitSplitStr): the part from the first SplitStr on
        let (s, t) = (c.str(0), c.str(1));
        match s.find(&t) {
            Some(i) => Value::Str(if c.arg(2).truthy() { s[i + t.len()..].to_string() } else { s[i..].to_string() }),
            None => Value::Str(String::new()),
        }
    });
    add("object.parsestringintoarray", |vm, c| {
        // ParseStringIntoArray(BaseString, out Pieces, Delim, bCullEmpty)
        let (s, d) = (c.str(0), c.str(2));
        let cull = c.arg(3).truthy();
        let parts: Vec<Value> = if d.is_empty() { vec![Value::Str(s)] } else { s.split(d.as_str()).filter(|p| !cull || !p.is_empty()).map(|p| Value::Str(p.into())).collect() };
        c.args[1] = Value::Array(parts);
        let _ = vm;
        Value::Int(0)
    });
    // objects / classes
    add("object.isa", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        let Value::Name(n) = c.arg(0) else { return Value::Bool(false) };
        let mut cl = Some(vm.obj(o).class);
        while let Some(ci) = cl {
            if ci == NONE_CLASS {
                break;
            }
            if vm.classes[ci].name == *n {
                return Value::Bool(true);
            }
            cl = vm.classes[ci].super_;
        }
        Value::Bool(false)
    });
    add("object.classischildof", |vm, c| match (vm.as_class(c.arg(0)), vm.as_class(c.arg(1))) {
        (Some(a), Some(b)) => Value::Bool(vm.is_child_of(a, b)),
        _ => Value::Bool(false),
    });
    add("object.getpackagename", |vm, c| {
        let Some(o) = c.this else { return Value::Name(Sym(0)) };
        let cl = vm.obj(o).class;
        let pkg = vm.classes.get(cl).and_then(|k| k.r).map(|r| vm.reg.packages[r.pkg].name.clone()).unwrap_or_default();
        Value::Name(vm.sym(&pkg))
    });
    add("object.getfuncname", |vm, _| {
        let n = vm.frames().last().map(|f| vm.funcs[f.func].name).unwrap_or_default();
        Value::Name(n)
    });
    // Object.PathName: Package.Group.Name (no class quote: "img://" + PathName(Texture) must
    // name the texture); script objects: their outer chain
    add("object.pathname", |vm, c| {
        let Some(o) = c.obj(0) else { return Value::Str("None".into()) };
        if let Kind::Asset { path, .. } = &vm.obj(o).kind {
            return Value::Str(path.clone());
        }
        let mut parts = vec![vm.obj(o).name.clone()];
        let mut cur = vm.obj(o).outer;
        while let Some(x) = cur {
            if parts.len() > 16 {
                break;
            }
            parts.push(vm.obj(x).name.clone());
            cur = vm.obj(x).outer;
        }
        parts.reverse();
        Value::Str(parts.join("."))
    });
    add("object.dynamicloadobject", |vm, c| {
        let path = c.str(0);
        let found = vm.reg.find_path(&path).or_else(|| {
            let (_, rest) = path.split_once('.')?;
            vm.reg.find_path(rest)
        });
        let o = found.and_then(|r| vm.obj_for(r));
        vm.say(format!("DynamicLoadObject({path}) -> {}", if o.is_some() { "found" } else { "None" }));
        Value::Obj(o)
    });
    add("object.findobject", |vm, c| {
        let path = c.str(0);
        Value::Obj(vm.reg.find_path(&path).and_then(|r| vm.obj_for(r)))
    });
    add("object.isinstate", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        let Value::Name(n) = c.arg(0) else { return Value::Bool(false) };
        Value::Bool(vm.obj(o).state == Some(*n))
    });
    add("object.getstatename", |vm, c| Value::Name(c.this.and_then(|o| vm.obj(o).state).unwrap_or_else(|| vm.sym("None"))));
    add("object.gotostate", |vm, c| {
        let Some(o) = c.this else { return Value::Int(0) };
        let st = match c.arg(0) {
            Value::Name(n) if !vm.name_of(*n).eq_ignore_ascii_case("None") => Some(*n),
            _ => None,
        };
        let old = vm.obj(o).state;
        if old != st {
            if old.is_some() {
                vm.call_named(o, "EndState", vec![Value::Name(st.unwrap_or_default())]);
            }
            vm.obj_mut(o).state = st;
            let who = vm.obj(o).name.clone();
            let sn = st.map(|s| vm.name_of(s).to_string()).unwrap_or_else(|| "None".into());
            vm.say(format!("{who} -> state {sn}"));
            if st.is_some() {
                vm.call_named(o, "BeginState", vec![Value::Name(old.unwrap_or_default())]);
            }
        }
        // state code from the label (default Begin)
        match st {
            Some(s) => {
                let label = match c.arg(1) {
                    Value::Name(n) if c.given(1) && !vm.name_of(*n).eq_ignore_ascii_case("None") => *n,
                    _ => vm.sym("Begin"),
                };
                vm.start_state_code(o, s, label);
            }
            None => {
                vm.state_runs.remove(&o);
            }
        }
        Value::Int(0)
    });
    // latent functions: Sleep suspends state code; the movement/animation waits finish at once
    add("actor.sleep", |vm, c| {
        vm.latent_sleep = Some(c.float(0).max(0.0));
        Value::Int(0)
    });
    for k in ["actor.finishanim", "actor.finishinterpolation"] {
        add(k, |_, _| Value::Int(0));
    }
    // movement waits: state code resumes when the host has walked the pawn there (see npcs.rs)
    add("controller.finishrotation", |vm, _| {
        vm.latent = Some(super::exec::Latent::Wait { left: 0.3 });
        Value::Int(0)
    });
    add("controller.waitforlanding", |vm, c| {
        let t = if c.passed.first().copied().unwrap_or(false) { c.float(0) } else { 4.0 };
        vm.latent = Some(super::exec::Latent::Land { left: t.max(0.1) });
        Value::Int(0)
    });
    // MoveTo(NewDestination, ViewFocus, DestinationOffset, bShouldWalk)
    add("controller.moveto", |vm, c| {
        let dest = vm.vec3(c.arg(0));
        let walk = c.arg(3).truthy();
        vm.latent = Some(super::exec::Latent::Move { dest, target: None, dist: 0.0, walk, left: 15.0 });
        Value::Int(0)
    });
    add("controller.movetodirectnonpathpos", |vm, c| {
        let dest = vm.vec3(c.arg(0));
        let dist = c.float(2).max(0.0);
        let walk = c.arg(3).truthy();
        vm.latent = Some(super::exec::Latent::Move { dest, target: None, dist, walk, left: 15.0 });
        Value::Int(0)
    });
    // MoveToward(NewTarget, ViewFocus, DestinationOffset, bUseStrafing, bShouldWalk)
    add("controller.movetoward", |vm, c| {
        let Some(t) = c.obj(0) else { return Value::Int(0) };
        let dist = c.float(2).max(0.0);
        let walk = c.arg(4).truthy();
        vm.latent = Some(super::exec::Latent::Move { dest: [0.0; 3], target: Some(t), dist, walk, left: 15.0 });
        Value::Int(0)
    });
    add("controller.stoplatentexecution", |vm, c| {
        if let Some(r) = c.this.and_then(|o| vm.state_runs.get_mut(&o)) {
            r.wait = None;
        }
        Value::Int(0)
    });
    add("object.isutracking", |_, _| Value::Bool(false));
    add("object.saveconfig", |_, _| Value::Int(0));
    add("object.staticsaveconfig", |_, _| Value::Int(0));
    // actors
    add("actor.destroy", |vm, c| {
        if let Some(o) = c.this {
            vm.call_named(o, "Destroyed", vec![]);
            vm.obj_mut(o).destroyed = true;
            let n = vm.obj(o).name.clone();
            vm.say(format!("{n} destroyed"));
        }
        Value::Bool(true)
    });
    add("actor.spawn", |vm, c| {
        let Some(cl) = vm.as_class(c.arg(0)) else { return Value::Obj(None) };
        let o = vm.spawn_actor(cl, c.obj(1).or(c.this));
        if c.given(3) {
            let v = c.arg(3).clone();
            vm.set(o, "Location", v);
        }
        Value::Obj(Some(o))
    });
    add("actor.settimer", |vm, c| {
        let Some(o) = c.this else { return Value::Int(0) };
        let rate = c.float(0);
        let looping = c.arg(1).truthy();
        let func = match c.arg(2) {
            Value::Name(n) if c.given(2) && !vm.name_of(*n).eq_ignore_ascii_case("None") => *n,
            _ => vm.sym("Timer"),
        };
        let obj = c.obj(3).unwrap_or(o);
        vm.timers.retain(|t| !(t.obj == obj && t.func == func));
        if rate > 0.0 {
            vm.timers.push(Timer { obj, func, rate, left: rate, looping });
        }
        Value::Int(0)
    });
    add("actor.cleartimer", |vm, c| {
        let Some(o) = c.this else { return Value::Int(0) };
        let func = match c.arg(0) {
            Value::Name(n) if c.given(0) => *n,
            _ => vm.sym("Timer"),
        };
        let obj = c.obj(1).unwrap_or(o);
        vm.timers.retain(|t| !(t.obj == obj && t.func == func));
        Value::Int(0)
    });
    add("actor.istimeractive", |vm, c| {
        let Some(o) = c.this else { return Value::Bool(false) };
        let func = match c.arg(0) {
            Value::Name(n) if c.given(0) => *n,
            _ => vm.sym("Timer"),
        };
        let obj = c.obj(1).unwrap_or(o);
        Value::Bool(vm.timers.iter().any(|t| t.obj == obj && t.func == func))
    });
    add("actor.gettimerrate", |vm, c| {
        let o = c.this;
        let func = match c.arg(0) {
            Value::Name(n) if c.given(0) => *n,
            _ => vm.sym("Timer"),
        };
        Value::Float(vm.timers.iter().find(|t| Some(t.obj) == o && t.func == func).map(|t| t.rate).unwrap_or(0.0))
    });
    add("actor.setlocation", |vm, c| {
        if let Some(o) = c.this {
            let v = c.arg(0).clone();
            vm.set(o, "Location", v);
        }
        Value::Bool(true)
    });
    add("actor.setrotation", |vm, c| {
        if let Some(o) = c.this {
            let v = c.arg(0).clone();
            vm.set(o, "Rotation", v);
        }
        Value::Bool(true)
    });
    add("actor.setphysics", |vm, c| {
        if let Some(o) = c.this {
            vm.set(o, "Physics", Value::Byte(c.int(0) as u8));
        }
        Value::Int(0)
    });
    add("actor.setbase", |vm, c| {
        if let Some(o) = c.this {
            let v = Value::Obj(c.obj(0));
            vm.set(o, "Base", v);
        }
        Value::Int(0)
    });
    add("actor.sethidden", |vm, c| {
        if let Some(o) = c.this {
            let hidden = c.arg(0).truthy();
            vm.set(o, "bHidden", Value::Bool(hidden));
            // a map actor (quest zone highlights...): its meshes in the level follow
            if vm.map_pkg.is_some() && vm.ref_of.get(&o).is_some_and(|r| Some(r.pkg) == vm.map_pkg) {
                let name = vm.obj(o).name.clone();
                vm.events.push(bridge::HostEvent::MapActorHidden { name, hidden });
            }
        }
        Value::Int(0)
    });
    add("actor.setdrawscale", |vm, c| {
        if let Some(o) = c.this {
            vm.set(o, "DrawScale", Value::Float(c.float(0)));
        }
        Value::Int(0)
    });
    add("actor.setcollision", |_, _| Value::Int(0));
    add("actor.setcollisionsize", |vm, c| {
        if let Some(o) = c.this {
            vm.set(o, "CollisionRadius", Value::Float(c.float(0)));
            vm.set(o, "CollisionHeight", Value::Float(c.float(1)));
        }
        Value::Int(0)
    });
    add("actor.getworldinfo", |vm, _| Value::Obj(vm.globals.get("worldinfo").copied()));
    add("worldinfo.getworldinfo", |vm, _| Value::Obj(vm.globals.get("worldinfo").copied()));
    add("worldinfo.isdemobuild", |_, _| Value::Bool(false));
    add("worldinfo.isconsolebuild", |_, _| Value::Bool(false));
    add("worldinfo.isplayinmobilepreview", |_, _| Value::Bool(false));
    // KActor.GetKActorPhysMaterial: the component's PhysMaterialOverride, else the mesh's
    // BodySetup PhysMaterial
    add("kactor.getkactorphysmaterial", |vm, c| {
        let Some(this) = c.this else { return Value::Obj(None) };
        let Some(comp) = vm.get(this, "StaticMeshComponent").obj() else { return Value::Obj(None) };
        if let Some(o) = vm.get(comp, "PhysMaterialOverride").obj() {
            return Value::Obj(Some(o));
        }
        let mesh = vm.get(comp, "StaticMesh").obj();
        let bs = mesh.and_then(|m| vm.get(m, "BodySetup").obj());
        Value::Obj(bs.and_then(|b| vm.get(b, "PhysMaterial").obj()))
    });
    // WorldInfo.GetMapInfo: the map's own WorldInfo.MyMapInfo (GGMapInfo: music, etc.)
    add("worldinfo.getmapinfo", |vm, _| {
        if let Some(&o) = vm.globals.get("mapinfo") {
            return Value::Obj(Some(o));
        }
        let Some(mp) = vm.map_pkg else { return Value::Obj(None) };
        let p = &vm.reg.packages[mp];
        let idx = (0..p.exports.len()).find(|&i| {
            let c = p.class_name(i as i32 + 1);
            (c == "GGMapInfo" || c.ends_with("MapInfo")) && p.obj_path(i as i32 + 1).contains(".PersistentLevel.")
        });
        let o = idx.and_then(|i| vm.obj_for_ref(mp, i as i32 + 1));
        if let Some(o) = o {
            vm.globals.insert("mapinfo", o);
        }
        Value::Obj(o)
    });
    // the map package's name ("Level_01"), what quests' mValidOnMaps compare against (Locs)
    add("worldinfo.getmapname", |vm, _| {
        Value::Str(
            vm.globals
                .get("mapname")
                .map(|o| vm.obj(*o).name.clone())
                .or_else(|| vm.map_pkg.map(|mp| vm.reg.packages[mp].name.clone()))
                .unwrap_or_default(),
        )
    });
    add("worldinfo.netmode", |_, _| Value::Byte(0));
    add("engine.getengine", |vm, _| Value::Obj(vm.globals.get("engine").copied()));
    add("gamegngine.getonlinesubsystem", |_, _| Value::Obj(None));
    add("gameengine.getonlinesubsystem", |_, _| Value::Obj(None));
    add("engine.iseditor", |_, _| Value::Bool(false));
    add("engine.isgame", |_, _| Value::Bool(true));
    add("engine.getcurrentworldinfo", |vm, _| Value::Obj(vm.globals.get("worldinfo").copied()));
    // components
    add("meshcomponent.setmaterial", |vm, c| {
        let owner = c.this.and_then(|o| vm.obj(o).outer).map(|o| vm.obj(o).name.clone()).unwrap_or_default();
        let mat = vm.describe(c.arg(1));
        vm.say(format!("SetMaterial({}, {mat}) on {owner}.{}", c.int(0), c.this.map(|o| vm.obj(o).name.clone()).unwrap_or_default()));
        if let Some(o) = c.this {
            let y = vm.sym("Materials");
            let i = c.int(0).max(0) as usize;
            let entry = vm.obj_mut(o).props.entry(y).or_insert(Value::Array(vec![]));
            if let Value::Array(a) = entry {
                if a.len() <= i {
                    a.resize(i + 1, Value::Obj(None));
                }
                a[i] = c.arg(1).clone();
            }
        }
        vm.events.push(bridge::HostEvent::SetMaterial { component: c.this, index: c.int(0), material: mat });
        Value::Int(0)
    });
    add("meshcomponent.getmaterial", |vm, c| {
        let Some(o) = c.this else { return Value::Obj(None) };
        match vm.get(o, "Materials") {
            Value::Array(a) => a.get(c.int(0).max(0) as usize).cloned().unwrap_or(Value::Obj(None)),
            _ => Value::Obj(None),
        }
    });
    // skeleton queries: the script goat has no skeleton, so every bone/control "exists"
    add("skeletalmeshcomponent.matchrefbone", |_, _| Value::Int(0));
    add("skeletalmeshcomponent.findskelcontrol", |_, _| Value::Obj(None));
    add("skeletalmeshcomponent.findanimnode", |_, _| Value::Obj(None));
    add("skeletalmeshcomponent.getbonelocation", |vm, c| {
        let owner = c.this.and_then(|o| vm.obj(o).outer);
        match owner {
            Some(o) => vm.get(o, "Location"),
            None => vm.vector(0.0, 0.0, 0.0),
        }
    });
    add("pawn.isplayerpawn", |vm, c| {
        let ctl = c.this.and_then(|o| vm.get(o, "Controller").obj());
        Value::Bool(ctl.is_some_and(|k| vm.get(k, "bIsPlayer").truthy()))
    });
    add("controller.isplayercontroller", |vm, c| {
        let pc = vm.class_by_name("PlayerController");
        Value::Bool(c.this.is_some_and(|o| pc.is_some_and(|p| vm.is_child_of(vm.obj(o).class, p))))
    });
    add("controller.islocalplayercontroller", |vm, c| {
        let pc = vm.class_by_name("PlayerController");
        Value::Bool(c.this.is_some_and(|o| pc.is_some_and(|p| vm.is_child_of(vm.obj(o).class, p))))
    });
    add("skeletalmeshcomponent.setskeletalmesh", |vm, c| {
        let m = vm.describe(c.arg(0));
        let owner = c.this.and_then(|o| vm.obj(o).outer).map(|o| vm.obj(o).name.clone()).unwrap_or_default();
        vm.say(format!("SetSkeletalMesh({m}) on {owner}"));
        if let Some(o) = c.this {
            let v = c.arg(0).clone();
            vm.set(o, "SkeletalMesh", v);
        }
        vm.events.push(bridge::HostEvent::SetSkeletalMesh { component: c.this, mesh: m });
        Value::Int(0)
    });
    add("gggameinfobase.registerdesiredevents", |vm, c| {
        if let (Some(g), Some(l)) = (c.this, c.obj(0)) {
            super::sync::register_desired_events(vm, g, l, true);
        }
        Value::Int(0)
    });
    add("gggameinfobase.unregisterdesiredevents", |vm, c| {
        if let (Some(g), Some(l)) = (c.this, c.obj(0)) {
            super::sync::register_desired_events(vm, g, l, false);
        }
        Value::Int(0)
    });
    add("primitivecomponent.setrbcollideswithchannel", |_, _| Value::Int(0));
    add("actorcomponent.setcomponentrbfixed", |_, _| Value::Int(0));
    add("actor.attachcomponent", |vm, c| {
        let what = vm.describe(&Value::Obj(c.obj(0)));
        let who = c.this.map(|o| vm.obj(o).name.clone()).unwrap_or_default();
        vm.say(format!("AttachComponent({what}) on {who}"));
        Value::Int(0)
    });
    add("actor.playsound", |vm, c| {
        let s = vm.describe(c.arg(0));
        vm.say(format!("PlaySound({s})"));
        Value::Int(0)
    });
    add("actor.clientmessage", |vm, c| {
        let s = vm.to_string(c.arg(0));
        vm.say(format!("ClientMessage: {s}"));
        vm.events.push(bridge::HostEvent::Message(s));
        Value::Int(0)
    });
}

/// UE3 Core operators from their function names ("Add_IntInt", "Subtract_PreFloat",
/// "AddEqual_FloatFloat" ...). Compound assignments also write their first (out) argument.
pub fn operator(vm: &mut Vm, c: &mut Call) -> Option<Value> {
    let full = vm.funcs[c.func].name;
    let name = vm.name_of(full).to_string();
    let (op, types) = name.split_once('_')?;
    let a = c.arg(0).clone();
    let b = c.arg(1).clone();
    if types.starts_with("Pre") || c.args.len() == 1 {
        let post = !types.starts_with("Pre");
        return Some(match op {
            "Not" => Value::Bool(!a.truthy()),
            "Complement" => Value::Int(!a.int()),
            "Subtract" => match &a {
                Value::Float(f) => Value::Float(-f),
                Value::Struct(_) => {
                    let v = vm.vec3(&a);
                    vm.vector(-v[0], -v[1], -v[2])
                }
                _ => Value::Int(a.int().wrapping_neg()),
            },
            "AddAdd" | "SubtractSubtract" => {
                let d = if op == "AddAdd" { 1 } else { -1 };
                let nv = match &a {
                    Value::Byte(x) => Value::Byte(x.wrapping_add(d as u8)),
                    _ => Value::Int(a.int().wrapping_add(d)),
                };
                // the out argument is written back by the caller from args[0]
                set_out(c, nv.clone());
                if post { a } else { nv }
            }
            _ => return None,
        });
    }
    // compound assignment: "AddEqual", "MultiplyEqual"...
    if let Some(base) = op.strip_suffix("Equal").filter(|b| !b.is_empty() && *b != "Equal" && *b != "Not" && *b != "Complement" && *b != "Less" && *b != "Greater") {
        let r = binop(vm, base, &a, &b)?;
        let r = match &a {
            Value::Int(_) => Value::Int(r.int()),
            Value::Byte(_) => Value::Byte(r.int() as u8),
            Value::Float(_) => Value::Float(r.float()),
            _ => r,
        };
        set_out(c, r.clone());
        return Some(r);
    }
    binop(vm, op, &a, &b)
}

fn set_out(c: &mut Call, v: Value) {
    if let Some(a) = c.args.first_mut() {
        *a = v;
    }
}

fn binop(vm: &mut Vm, op: &str, a: &Value, b: &Value) -> Option<Value> {
    use Value::*;
    let is_vec = |v: &Value, vm: &Vm| matches!(v, Struct(s) if vm.name_of(s.name).eq_ignore_ascii_case("Vector"));
    let is_rot = |v: &Value, vm: &Vm| matches!(v, Struct(s) if vm.name_of(s.name).eq_ignore_ascii_case("Rotator"));
    // strings
    if matches!(a, Str(_)) || matches!(b, Str(_)) || op == "Concat" || op == "At" {
        let (x, y) = (vm.to_string(a), vm.to_string(b));
        return Some(match op {
            "Concat" => Str(x + &y),
            "At" => Str(format!("{x} {y}")),
            "EqualEqual" => Bool(x == y),
            "NotEqual" => Bool(x != y),
            "ComplementEqual" => Bool(x.eq_ignore_ascii_case(&y)),
            "Less" => Bool(x < y),
            "Greater" => Bool(x > y),
            "LessEqual" => Bool(x <= y),
            "GreaterEqual" => Bool(x >= y),
            "Subtract" => Str(x.replace(&y, "")),
            _ => return None,
        });
    }
    if is_vec(a, vm) && is_rot(b, vm) && (op == "GreaterGreater" || op == "LessLess") {
        let v = vm.vec3(a);
        let r = vm.rot3(b);
        let k = std::f32::consts::TAU / 65536.0;
        let (sp, cp) = (r[0] as f32 * k).sin_cos();
        let (sy, cy) = (r[1] as f32 * k).sin_cos();
        let (sr, cr) = (r[2] as f32 * k).sin_cos();
        let ax = [
            [cp * cy, cp * sy, sp],
            [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp],
            [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp],
        ];
        let out = if op == "GreaterGreater" {
            // local -> world
            [0, 1, 2].map(|i| v[0] * ax[0][i] + v[1] * ax[1][i] + v[2] * ax[2][i])
        } else {
            [0, 1, 2].map(|i| v[0] * ax[i][0] + v[1] * ax[i][1] + v[2] * ax[i][2])
        };
        return Some(vm.vector(out[0], out[1], out[2]));
    }
    if is_vec(a, vm) || is_vec(b, vm) {
        let va = if is_vec(a, vm) { Some(vm.vec3(a)) } else { None };
        let vb = if is_vec(b, vm) { Some(vm.vec3(b)) } else { None };
        return Some(match (op, va, vb) {
            ("Add", Some(x), Some(y)) => vm.vector(x[0] + y[0], x[1] + y[1], x[2] + y[2]),
            ("Subtract", Some(x), Some(y)) => vm.vector(x[0] - y[0], x[1] - y[1], x[2] - y[2]),
            ("Multiply", Some(x), Some(y)) => vm.vector(x[0] * y[0], x[1] * y[1], x[2] * y[2]),
            ("Multiply", Some(x), None) => {
                let f = b.float();
                vm.vector(x[0] * f, x[1] * f, x[2] * f)
            }
            ("Multiply", None, Some(y)) => {
                let f = a.float();
                vm.vector(y[0] * f, y[1] * f, y[2] * f)
            }
            ("Divide", Some(x), None) => {
                let f = b.float();
                vm.vector(x[0] / f, x[1] / f, x[2] / f)
            }
            ("Dot", Some(x), Some(y)) => Float(x[0] * y[0] + x[1] * y[1] + x[2] * y[2]),
            ("Cross", Some(x), Some(y)) => vm.vector(x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]),
            ("EqualEqual", Some(x), Some(y)) => Bool(x == y),
            ("NotEqual", Some(x), Some(y)) => Bool(x != y),
            _ => return None,
        });
    }
    if is_rot(a, vm) || is_rot(b, vm) {
        let ra = if is_rot(a, vm) { Some(vm.rot3(a)) } else { None };
        let rb = if is_rot(b, vm) { Some(vm.rot3(b)) } else { None };
        return Some(match (op, ra, rb) {
            ("Add", Some(x), Some(y)) => vm.rotator(x[0].wrapping_add(y[0]), x[1].wrapping_add(y[1]), x[2].wrapping_add(y[2])),
            ("Subtract", Some(x), Some(y)) => vm.rotator(x[0].wrapping_sub(y[0]), x[1].wrapping_sub(y[1]), x[2].wrapping_sub(y[2])),
            ("Multiply", Some(x), None) | ("Multiply", None, Some(x)) => {
                let f = if ra.is_some() { b.float() } else { a.float() };
                vm.rotator((x[0] as f32 * f) as i32, (x[1] as f32 * f) as i32, (x[2] as f32 * f) as i32)
            }
            ("Divide", Some(x), None) => {
                let f = b.float();
                if f == 0.0 { vm.rotator(0, 0, 0) } else { vm.rotator((x[0] as f32 / f) as i32, (x[1] as f32 / f) as i32, (x[2] as f32 / f) as i32) }
            }
            ("EqualEqual", Some(x), Some(y)) => Bool(x == y),
            ("NotEqual", Some(x), Some(y)) => Bool(x != y),
            _ => return None,
        });
    }
    // objects, names, bools, delegates
    if matches!(a, Obj(_) | Name(_) | Delegate(..)) || matches!(b, Obj(_) | Name(_) | Delegate(..)) {
        return Some(match op {
            "EqualEqual" => Bool(values_equal(a, b)),
            "NotEqual" => Bool(!values_equal(a, b)),
            _ => return None,
        });
    }
    if matches!(a, Bool(_)) && matches!(b, Bool(_)) {
        let (x, y) = (a.truthy(), b.truthy());
        return Some(match op {
            "EqualEqual" => Bool(x == y),
            "NotEqual" => Bool(x != y),
            "AndAnd" => Bool(x && y),
            "OrOr" => Bool(x || y),
            "XorXor" => Bool(x ^ y),
            _ => return None,
        });
    }
    let float = matches!(a, Float(_)) || matches!(b, Float(_));
    if float {
        let (x, y) = (a.float(), b.float());
        return Some(match op {
            "Add" => Float(x + y),
            "Subtract" => Float(x - y),
            "Multiply" => Float(x * y),
            "Divide" => Float(if y == 0.0 { 0.0 } else { x / y }),
            "Percent" => Float(if y == 0.0 { 0.0 } else { x % y }),
            "MultiplyMultiply" => Float(x.powf(y)),
            "Less" => Bool(x < y),
            "Greater" => Bool(x > y),
            "LessEqual" => Bool(x <= y),
            "GreaterEqual" => Bool(x >= y),
            "EqualEqual" => Bool(x == y),
            "NotEqual" => Bool(x != y),
            "ComplementEqual" => Bool((x - y).abs() < 1e-4),
            _ => return None,
        });
    }
    let (x, y) = (a.int(), b.int());
    let r = match op {
        "Add" => Int(x.wrapping_add(y)),
        "Subtract" => Int(x.wrapping_sub(y)),
        "Multiply" => Int(x.wrapping_mul(y)),
        "Divide" => Int(if y == 0 { 0 } else { x.wrapping_div(y) }),
        "Percent" => Int(if y == 0 { 0 } else { x.wrapping_rem(y) }),
        "LessLess" => Int(x.wrapping_shl(y as u32)),
        "GreaterGreater" => Int(x.wrapping_shr(y as u32)),
        "GreaterGreaterGreater" => Int(((x as u32) >> (y as u32 & 31)) as i32),
        "And" => Int(x & y),
        "Or" => Int(x | y),
        "Xor" => Int(x ^ y),
        "Less" => Bool(x < y),
        "Greater" => Bool(x > y),
        "LessEqual" => Bool(x <= y),
        "GreaterEqual" => Bool(x >= y),
        "EqualEqual" => Bool(x == y),
        "NotEqual" => Bool(x != y),
        _ => return None,
    };
    // byte op byte stays a byte
    Some(match (a, b, &r) {
        (Byte(_), Byte(_), Int(i)) => Byte(*i as u8),
        _ => r,
    })
}

/// `foreach` natives: rows of out-parameter values.
pub fn iterator(vm: &mut Vm, key: &str, ctx: Option<ObjId>, args: &[Value]) -> Option<Vec<Vec<Value>>> {
    let short = key.rsplit('.').next().unwrap_or(key);
    let base = args.first().and_then(|v| vm.as_class(v));
    let actors = |vm: &mut Vm, base: Option<usize>| -> Vec<Vec<Value>> {
        let actor = vm.class_by_name("Actor");
        (0..vm.objects.len() as ObjId)
            .filter(|&o| {
                let ob = vm.obj(o);
                matches!(ob.kind, Kind::Instance)
                    && !ob.destroyed
                    && ob.template_of.is_none()
                    && !ob.name.starts_with("Default__")
                    && actor.is_some_and(|a| vm.is_child_of(ob.class, a))
                    && base.is_none_or(|b| vm.is_child_of(ob.class, b))
            })
            .map(|o| vec![Value::Obj(Some(o))])
            .collect()
    };
    Some(match short {
        "allactors" | "dynamicactors" | "touchingactors" | "basedactors" => actors(vm, base),
        // (class, out actor, radius, location, ...): actors whose Location is within the radius
        "visibleactors" | "visiblecollidingactors" | "collidingactors" | "overlappingactors" => {
            let radius = args.get(2).map(|v| v.float()).unwrap_or(0.0);
            let center = match args.get(3) {
                Some(v @ Value::Struct(_)) => vm.vec3(v),
                _ => ctx.map(|o| vm.get(o, "Location")).map(|l| vm.vec3(&l)).unwrap_or_default(),
            };
            // game objects in range get their proxies first (NPCs exist already; props on demand)
            let things = vm.overlap.as_ref().map(|f| f(center, radius.max(1.0))).unwrap_or_default();
            for (t, l) in things {
                vm.proxy_for(&t, l);
            }
            let rows = actors(vm, base);
            rows.into_iter()
                .filter(|r| {
                    let Some(o) = r[0].obj() else { return false };
                    if Some(o) == ctx {
                        return false;
                    }
                    let l = vm.get(o, "Location");
                    let p = vm.vec3(&l);
                    let d2 = (p[0] - center[0]).powi(2) + (p[1] - center[1]).powi(2) + (p[2] - center[2]).powi(2);
                    radius <= 0.0 || d2 <= radius * radius
                })
                .collect()
        }
        "childactors" => actors(vm, base).into_iter().filter(|r| r[0].obj().is_some_and(|o| vm.get(o, "Owner").obj() == ctx)).collect(),
        "allcontrollers" | "allpawns" => actors(vm, base),
        // (class, out actor, out hitloc, out hitnorm, end, optional start, extent...): the first hit
        "traceactors" => {
            let end = args.get(4).map(|v| vm.vec3(v)).unwrap_or_default();
            let start = match args.get(5) {
                Some(v @ Value::Struct(_)) => vm.vec3(v),
                _ => ctx.map(|o| vm.get(o, "Location")).map(|l| vm.vec3(&l)).unwrap_or_default(),
            };
            let Some(hit) = vm.tracer.as_ref().and_then(|t| t(start, end)) else { return Some(vec![]) };
            let actor = hit.actor.or_else(|| hit.thing.as_ref().map(|t| vm.proxy_for(t, hit.location)));
            let Some(actor) = actor else { return Some(vec![]) };
            if base.is_some_and(|b| !vm.is_child_of(vm.obj(actor).class, b)) {
                return Some(vec![]);
            }
            let l = vm.vector(hit.location[0], hit.location[1], hit.location[2]);
            let n = vm.vector(hit.normal[0], hit.normal[1], hit.normal[2]);
            vec![vec![Value::Obj(Some(actor)), l, n]]
        }
        "localplayercontrollers" => actors(vm, base),
        "allownedcomponents" | "componentlist" => vec![],
        _ => return None,
    })
}

fn replace_ci(s: &str, a: &str, b: &str) -> String {
    let (ls, la) = (s.to_lowercase(), a.to_lowercase());
    let mut out = String::new();
    let mut i = 0;
    while let Some(j) = ls[i..].find(&la) {
        out.push_str(&s[i..i + j]);
        out.push_str(b);
        i += j + a.len();
    }
    out.push_str(&s[i..]);
    out
}

pub fn rand01() -> f32 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0x2545F4914F6CDD1D);
    let mut x = SEED.load(Ordering::Relaxed);
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    SEED.store(x, Ordering::Relaxed);
    (x >> 40) as f32 / (1u64 << 24) as f32
}
