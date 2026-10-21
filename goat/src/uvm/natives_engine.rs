//! Engine natives for components, skeletal controls, rotators, audio and messages: the ones
//! Workshop mutators hit most (see `goat --uvm` "unsupported" list).

use super::bridge::HostEvent;
use super::exec::Call;
use super::*;

fn comp_owner_name(vm: &Vm, c: &Call) -> String {
    c.this.and_then(|o| vm.obj(o).outer).map(|o| vm.obj(o).name.clone()).unwrap_or_default()
}

/// Store a native-only component setting as a property and return nothing.
fn store(vm: &mut Vm, c: &Call, prop: &str, v: Value) -> Value {
    if let Some(o) = c.this {
        vm.set(o, prop, v);
    }
    Value::Int(0)
}

fn rot_axes(r: [i32; 3]) -> [[f32; 3]; 3] {
    let k = std::f32::consts::TAU / 65536.0;
    let (sp, cp) = (r[0] as f32 * k).sin_cos();
    let (sy, cy) = (r[1] as f32 * k).sin_cos();
    let (sr, cr) = (r[2] as f32 * k).sin_cos();
    [
        [cp * cy, cp * sy, sp],
        [sr * sp * cy - cr * sy, sr * sp * sy + cr * cy, -sr * cp],
        [-(cr * sp * cy + sr * sy), cy * sr - cr * sp * sy, cr * cp],
    ]
}

fn norm_axis(a: i32) -> i32 {
    let a = a & 0xFFFF;
    if a > 32767 { a - 65536 } else { a }
}

pub fn register(vm: &mut Vm) {
    let n = &mut vm.natives;
    let mut add = |k: &str, f: NativeFn| {
        n.insert(k.to_ascii_lowercase(), f);
    };
    // --- rotators / vectors ---------------------------------------------------------------
    add("object.iszero", |vm, c| Value::Bool(vm.vec3(c.arg(0)) == [0.0; 3]));
    add("object.getaxes", |vm, c| {
        let r = vm.rot3(c.arg(0));
        let ax = rot_axes(r);
        for i in 0..3 {
            c.args[i + 1] = vm.vector(ax[i][0], ax[i][1], ax[i][2]);
        }
        Value::Int(0)
    });
    add("object.getunaxes", |vm, c| {
        let r = vm.rot3(c.arg(0));
        let ax = rot_axes(r);
        for i in 0..3 {
            c.args[i + 1] = vm.vector(ax[0][i], ax[1][i], ax[2][i]);
        }
        Value::Int(0)
    });
    add("object.orthorotation", |vm, c| {
        let (x, y, z) = (vm.vec3(c.arg(0)), vm.vec3(c.arg(1)), vm.vec3(c.arg(2)));
        let k = 32768.0 / std::f32::consts::PI;
        let pitch = x[2].atan2((x[0] * x[0] + x[1] * x[1]).sqrt());
        let yaw = x[1].atan2(x[0]);
        // roll from the Y/Z axes of the yaw/pitch frame
        let sy0 = rot_axes([(pitch * k) as i32, (yaw * k) as i32, 0]);
        let roll = (z[0] * sy0[1][0] + z[1] * sy0[1][1] + z[2] * sy0[1][2]).atan2(z[0] * sy0[2][0] + z[1] * sy0[2][1] + z[2] * sy0[2][2]);
        let _ = y;
        vm.rotator((pitch * k) as i32, (yaw * k) as i32, (-roll * k) as i32)
    });
    add("object.normalize", |vm, c| {
        let r = vm.rot3(c.arg(0));
        vm.rotator(norm_axis(r[0]), norm_axis(r[1]), norm_axis(r[2]))
    });
    add("object.normalizerotaxis", |_, c| Value::Int(norm_axis(c.int(0))));
    add("object.rlerp", |vm, c| {
        let (a, b, t) = (vm.rot3(c.arg(0)), vm.rot3(c.arg(1)), c.float(2));
        let short = c.arg(3).truthy();
        let f = |x: i32, y: i32| {
            let d = if short { norm_axis(y - x) } else { y - x };
            x + (d as f32 * t) as i32
        };
        vm.rotator(f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]))
    });
    add("object.rinterpto", |vm, c| {
        let (a, b, dt, speed) = (vm.rot3(c.arg(0)), vm.rot3(c.arg(1)), c.float(2), c.float(3));
        if speed <= 0.0 {
            return c.arg(1).clone();
        }
        let alpha = (dt * speed).clamp(0.0, 1.0);
        let f = |x: i32, y: i32| x + (norm_axis(y - x) as f32 * alpha) as i32;
        vm.rotator(f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2]))
    });
    add("object.vlerp", |vm, c| {
        let (a, b, t) = (vm.vec3(c.arg(0)), vm.vec3(c.arg(1)), c.float(2));
        vm.vector(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t)
    });
    add("object.vinterpto", |vm, c| {
        let (a, b, dt, speed) = (vm.vec3(c.arg(0)), vm.vec3(c.arg(1)), c.float(2), c.float(3));
        let t = if speed <= 0.0 { 1.0 } else { (dt * speed).clamp(0.0, 1.0) };
        vm.vector(a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t)
    });
    add("object.rsize", |vm, c| {
        let r = vm.rot3(c.arg(0));
        let f = |x: i32| norm_axis(x) as f32;
        Value::Int((f(r[0]).powi(2) + f(r[1]).powi(2) + f(r[2]).powi(2)).sqrt() as i32)
    });
    add("object.clampvector", |vm, c| {
        let (v, lo, hi) = (vm.vec3(c.arg(0)), vm.vec3(c.arg(1)), vm.vec3(c.arg(2)));
        vm.vector(v[0].clamp(lo[0], hi[0]), v[1].clamp(lo[1], hi[1]), v[2].clamp(lo[2], hi[2]))
    });
    // --- components -------------------------------------------------------------------------
    add("primitivecomponent.setactorcollision", |vm, c| store(vm, c, "CollideActors", Value::Bool(c.arg(0).truthy())));
    add("primitivecomponent.setblockrigidbody", |vm, c| store(vm, c, "BlockRigidBody", Value::Bool(c.arg(0).truthy())));
    add("primitivecomponent.settraceblocking", |_, _| Value::Int(0));
    add("primitivecomponent.setrotation", |vm, c| store(vm, c, "Rotation", c.arg(0).clone()));
    add("primitivecomponent.settranslation", |vm, c| store(vm, c, "Translation", c.arg(0).clone()));
    add("primitivecomponent.setscale", |vm, c| store(vm, c, "Scale", Value::Float(c.float(0))));
    add("primitivecomponent.setscale3d", |vm, c| store(vm, c, "Scale3D", c.arg(0).clone()));
    add("primitivecomponent.sethidden", |vm, c| store(vm, c, "HiddenGame", Value::Bool(c.arg(0).truthy())));
    add("primitivecomponent.setownernosee", |vm, c| store(vm, c, "bOwnerNoSee", Value::Bool(c.arg(0).truthy())));
    add("primitivecomponent.setonlyownersee", |vm, c| store(vm, c, "bOnlyOwnerSee", Value::Bool(c.arg(0).truthy())));
    add("primitivecomponent.setlightenvironment", |vm, c| store(vm, c, "LightEnvironment", c.arg(0).clone()));
    add("primitivecomponent.setphysmaterialoverride", |vm, c| store(vm, c, "PhysMaterialOverride", c.arg(0).clone()));
    add("primitivecomponent.getrotation", |vm, c| match c.this {
        Some(o) => match vm.get(o, "Rotation") {
            v @ Value::Struct(_) => v,
            _ => vm.rotator(0, 0, 0),
        },
        None => vm.rotator(0, 0, 0),
    });
    add("primitivecomponent.getposition", |vm, c| {
        let owner = c.this.and_then(|o| vm.obj(o).outer);
        match owner {
            Some(o) => vm.get(o, "Location"),
            None => vm.vector(0.0, 0.0, 0.0),
        }
    });
    add("primitivecomponent.setrblinearvelocity", |vm, c| {
        // the owning actor's velocity is what the host applies to its body
        if let Some(owner) = c.this.and_then(|o| vm.obj(o).outer) {
            let v = c.arg(0).clone();
            vm.set(owner, "Velocity", v);
        }
        store(vm, c, "RBLinearVelocity", c.arg(0).clone())
    });
    add("primitivecomponent.getrblinearvelocity", |vm, c| match c.this.map(|o| vm.get(o, "RBLinearVelocity")) {
        Some(v @ Value::Struct(_)) => v,
        _ => vm.vector(0.0, 0.0, 0.0),
    });
    add("primitivecomponent.addimpulse", |vm, c| {
        let who = comp_owner_name(vm, c);
        let v = vm.vec3(c.arg(0));
        vm.say(format!("AddImpulse({:.0},{:.0},{:.0}) on {who}", v[0], v[1], v[2]));
        if let Some(owner) = c.this.and_then(|o| vm.obj(o).outer) {
            vm.events.push(HostEvent::Impulse { actor: owner, impulse: v, velocity_change: c.arg(3).truthy() });
        }
        Value::Int(0)
    });
    add("primitivecomponent.setrbangularvelocity", |vm, c| store(vm, c, "RBAngularVelocity", c.arg(0).clone()));
    add("primitivecomponent.setrbchannel", |_, _| Value::Int(0));
    add("primitivecomponent.setrbcollidewithchannel", |_, _| Value::Int(0));
    add("primitivecomponent.setnotifyrigidbodycollision", |_, _| Value::Int(0));
    add("primitivecomponent.initrbphys", |_, _| Value::Int(0));
    add("primitivecomponent.wakerigidbody", |_, _| Value::Int(0));
    add("primitivecomponent.putrigidbodytosleep", |_, _| Value::Int(0));
    // material instances made by script (CreateAndSetMaterialInstanceConstant + parameters)
    add("meshcomponent.createandsetmaterialinstanceconstant", |vm, c| {
        let Some(comp) = c.this else { return Value::Obj(None) };
        let Some(class) = vm.class_by_name("MaterialInstanceConstant") else { return Value::Obj(None) };
        let i = c.int(0).max(0) as usize;
        let mic = vm.spawn_object(class, None);
        let parent = match vm.get(comp, "Materials") {
            Value::Array(a) => a.get(i).cloned().unwrap_or(Value::Obj(None)),
            _ => Value::Obj(None),
        };
        vm.set(mic, "Parent", parent);
        let y = vm.sym("Materials");
        let entry = vm.obj_mut(comp).props.entry(y).or_insert(Value::Array(vec![]));
        if let Value::Array(a) = entry {
            if a.len() <= i {
                a.resize(i + 1, Value::Obj(None));
            }
            a[i] = Value::Obj(Some(mic));
        }
        Value::Obj(Some(mic))
    });
    add("materialinstanceconstant.setparent", |vm, c| store(vm, c, "Parent", c.arg(0).clone()));
    for k in ["materialinstanceconstant.setvectorparametervalue", "materialinstanceconstant.setscalarparametervalue", "materialinstanceconstant.settextureparametervalue"] {
        add(k, |vm, c| {
            let Some(mic) = c.this else { return Value::Int(0) };
            let Value::Name(n) = c.arg(0).clone() else { return Value::Int(0) };
            let key = format!("uvmParam_{}", vm.name_of(n));
            let v = c.arg(1).clone();
            vm.set(mic, &key, v);
            vm.events.push(HostEvent::MaterialParam(mic));
            Value::Int(0)
        });
    }
    add("actor.getboundingcylinder", |vm, c| {
        let (r, h) = match c.this {
            Some(o) => (vm.get(o, "CollisionRadius").float(), vm.get(o, "CollisionHeight").float()),
            None => (0.0, 0.0),
        };
        c.args[0] = Value::Float(if r > 0.0 { r } else { 34.0 });
        c.args[1] = Value::Float(if h > 0.0 { h } else { 78.0 });
        Value::Int(0)
    });
    // ragdoll: the host starts the real ragdoll when the script goat's mIsRagdoll turns on
    add("pawn.initragdoll", |_, _| Value::Bool(true));
    add("pawn.termragdoll", |_, _| Value::Bool(true));
    add("skeletalmeshcomponent.forceskelupdate", |_, _| Value::Int(0));
    add("skeletalmeshcomponent.updaterbbonesfromspacebases", |_, _| Value::Int(0));
    add("skeletalmeshcomponent.sethasphysicsassetinstance", |_, _| Value::Int(0));
    add("skeletalmeshcomponent.setenablephysicsassetinstance", |_, _| Value::Int(0));
    add("skeletalmeshcomponent.lockbonesinlocation", |_, _| Value::Int(0));
    add("skeletalmeshcomponent.getboneaxis", |vm, _| vm.vector(1.0, 0.0, 0.0));
    add("pawn.setdesiredrotation", |vm, c| {
        if let Some(o) = c.this {
            let v = c.arg(0).clone();
            vm.set(o, "DesiredRotation", v);
        }
        Value::Int(0)
    });
    add("pawn.clearpathstep", |_, _| Value::Int(0));
    add("actor.setrelativerotation", |vm, c| {
        if let Some(o) = c.this {
            let v = c.arg(0).clone();
            vm.set(o, "RelativeRotation", v);
        }
        Value::Int(0)
    });
    add("actor.setrelativelocation", |vm, c| {
        if let Some(o) = c.this {
            let v = c.arg(0).clone();
            vm.set(o, "RelativeLocation", v);
        }
        Value::Int(0)
    });
    add("soundcue.getcueduration", |_, _| Value::Float(1.0));
    add("ggsvehiclenative.isinstreaminglevel", |_, _| Value::Bool(false));
    add("svehicle.haswheelsonground", |_, _| Value::Bool(false));
    add("actor.setowner", |vm, c| {
        if let Some(o) = c.this {
            let v = Value::Obj(c.obj(0));
            vm.set(o, "Owner", v);
        }
        Value::Int(0)
    });
    add("controller.setfocalpoint", |vm, c| store(vm, c, "FocalPoint", c.arg(0).clone()));
    add("primitivecomponent.setshadowparent", |_, _| Value::Int(0));
    add("actor.sethardattach", |_, _| Value::Int(0));
    add("primitivecomponent.getrootbodyinstance", |_, _| Value::Obj(None));
    add("actor.makenoise", |_, _| Value::Int(0));
    add("hud.draw3dline", |_, _| Value::Int(0));
    add("hud.drawdebugline", |_, _| Value::Int(0));
    add("actor.drawdebugline", |_, _| Value::Int(0));
    add("actor.drawdebugsphere", |_, _| Value::Int(0));
    add("localplayer.insertpostprocessingchain", |vm, _| {
        vm.say("InsertPostProcessingChain (post-process not applied)".into());
        Value::Bool(true)
    });
    add("localplayer.removeallpostprocessingchains", |_, _| Value::Int(0));
    add("meshcomponent.getnumelements", |vm, c| match c.this.map(|o| vm.get(o, "Materials")) {
        Some(Value::Array(a)) => Value::Int(a.len().max(1) as i32),
        _ => Value::Int(1),
    });
    // --- skeletal meshes --------------------------------------------------------------------
    add("skeletalmeshcomponent.matchrefbone", |vm, c| {
        let Value::Name(n) = c.arg(0) else { return Value::Int(-1) };
        let name = vm.name_of(*n).to_ascii_lowercase();
        if vm.bones.is_empty() {
            return Value::Int(0);
        }
        Value::Int(vm.bones.iter().position(|b| b.eq_ignore_ascii_case(&name)).map(|i| i as i32).unwrap_or(-1))
    });
    add("skeletalmeshcomponent.getbonename", |vm, c| {
        let b = vm.bones.get(c.int(0).max(0) as usize).cloned().unwrap_or_else(|| "None".into());
        Value::Name(vm.sym(&b))
    });
    add("skeletalmeshcomponent.getnumbones", |vm, _| Value::Int(vm.bones.len() as i32));
    add("skeletalmeshcomponent.addskelcontrol", |vm, c| {
        let Some(comp) = c.this else { return Value::Obj(None) };
        let Some(class) = vm.as_class(c.arg(1)) else { return Value::Obj(None) };
        let ctl = vm.spawn_object(class, None);
        vm.obj_mut(ctl).outer = Some(comp);
        let bone = c.arg(0).clone();
        vm.set(ctl, "ControlName", bone.clone());
        vm.set(ctl, "uvmBoneName", bone);
        vm.set(ctl, "ControlStrength", Value::Float(1.0));
        let y = vm.sym("uvmSkelControls");
        let list = vm.obj_mut(comp).props.entry(y).or_insert(Value::Array(vec![]));
        if let Value::Array(a) = list {
            a.push(Value::Obj(Some(ctl)));
        }
        Value::Obj(Some(ctl))
    });
    add("skeletalmeshcomponent.findskelcontrol", |vm, c| {
        let Some(comp) = c.this else { return Value::Obj(None) };
        let want = c.arg(0).clone();
        match vm.get(comp, "uvmSkelControls") {
            Value::Array(a) => Value::Obj(a.iter().filter_map(|v| v.obj()).find(|&k| vm.get(k, "ControlName") == want)),
            _ => Value::Obj(None),
        }
    });
    add("skelcontrolbase.setskelcontrolstrength", |vm, c| store(vm, c, "ControlStrength", Value::Float(c.float(0))));
    add("skelcontrolbase.setskelcontrolactive", |vm, c| store(vm, c, "ControlStrength", Value::Float(if c.arg(0).truthy() { 1.0 } else { 0.0 })));
    add("skeletalmeshcomponent.initskelcontrols", |_, _| Value::Int(0));
    add("skeletalmeshcomponent.setphysicsasset", |vm, c| {
        let a = vm.describe(c.arg(0));
        let who = comp_owner_name(vm, c);
        vm.say(format!("SetPhysicsAsset({a}) on {who}"));
        store(vm, c, "PhysicsAsset", c.arg(0).clone())
    });
    add("skeletalmeshcomponent.setanimtreetemplate", |vm, c| {
        let a = vm.describe(c.arg(0));
        let who = comp_owner_name(vm, c);
        vm.say(format!("SetAnimTreeTemplate({a}) on {who}"));
        store(vm, c, "AnimTreeTemplate", c.arg(0).clone())
    });
    add("skeletalmeshcomponent.attachcomponenttosocket", |vm, c| {
        let what = vm.describe(&Value::Obj(c.obj(0)));
        let sock = vm.to_string(c.arg(1));
        let who = comp_owner_name(vm, c);
        vm.say(format!("AttachComponentToSocket({what}, {sock}) on {who}"));
        vm.events.push(HostEvent::AttachToSocket { component: c.obj(0), mesh: c.this, socket: sock });
        Value::Int(0)
    });
    add("skeletalmeshcomponent.attachcomponent", |vm, c| {
        let what = vm.describe(&Value::Obj(c.obj(0)));
        let bone = vm.to_string(c.arg(1));
        let who = comp_owner_name(vm, c);
        vm.say(format!("AttachComponent({what}, {bone}) on {who}"));
        Value::Int(0)
    });
    add("skeletalmeshcomponent.getsocketworldlocationandrotation", |vm, c| {
        let owner = c.this.and_then(|o| vm.obj(o).outer);
        let loc = owner.map(|o| vm.get(o, "Location")).unwrap_or(Value::Int(0));
        let mut loc = if matches!(loc, Value::Struct(_)) { loc } else { vm.vector(0.0, 0.0, 0.0) };
        // the goat's headSocket: in front of the pawn at the mouth (as goat.rs licks from)
        let is_head = matches!(c.arg(0), Value::Name(n) if vm.name_of(*n).eq_ignore_ascii_case("headSocket"));
        if let (true, Some(o)) = (is_head, owner) {
            let l = vm.vec3(&loc);
            let rot = vm.get(o, "Rotation");
            let yaw = match &rot {
                Value::Struct(s) => s.field(vm.sym("Yaw")).map(|v| v.int()).unwrap_or(0),
                _ => 0,
            } as f32 * std::f32::consts::PI / 32768.0;
            loc = vm.vector(l[0] + yaw.cos() * 40.0, l[1] + yaw.sin() * 40.0, l[2] + 15.0);
        }
        c.args[1] = loc;
        Value::Bool(true)
    });
    // the prop's collision box (props only; others report none)
    add("actor.getcomponentsboundingbox", |vm, c| {
        let b = c.this.and_then(|o| vm.proxy_things.get(&o).cloned()).and_then(|t| vm.bounds.as_ref().and_then(|f| f(&t)));
        let (lo, hi) = b.unwrap_or(([0.0; 3], [0.0; 3]));
        let (min, max) = (vm.vector(lo[0], lo[1], lo[2]), vm.vector(hi[0], hi[1], hi[2]));
        let (mn, mx, iv, bx) = (vm.sym("Min"), vm.sym("Max"), vm.sym("IsValid"), vm.sym("Box"));
        c.args[0] = Value::Struct(Box::new(super::value::StructVal { def: None, name: bx, fields: vec![(mn, min), (mx, max), (iv, Value::Byte(b.is_some() as u8))] }));
        Value::Int(0)
    });
    // WorldInfo.GetGameSequence: the map's Main_Sequence (any Sequence if it has another name)
    add("worldinfo.getgamesequence", |vm, _| {
        let Some(sc) = vm.class_by_name("Sequence") else { return Value::Obj(None) };
        let seqs: Vec<ObjId> = vm.seq_objects.iter().copied().filter(|&o| vm.obj(o).class == sc).collect();
        let main = seqs.iter().copied().find(|&o| vm.obj(o).name.eq_ignore_ascii_case("Main_Sequence")).or(seqs.first().copied());
        Value::Obj(main)
    });
    // Sequence.FindSeqObjectsByClass(class, bRecursive, out array): the map's sequence objects
    // of that class (every nested sequence is searched; the map has one game sequence)
    add("sequence.findseqobjectsbyclass", |vm, c| {
        let Some(cl) = vm.as_class(c.arg(0)) else { return Value::Int(0) };
        let found: Vec<Value> = vm.seq_objects.iter().copied().filter(|&o| !vm.obj(o).destroyed && vm.is_child_of(vm.obj(o).class, cl)).map(|o| Value::Obj(Some(o))).collect();
        if c.args.len() > 2 {
            c.args[2] = Value::Array(found);
        }
        Value::Int(0)
    });
    // SequenceEvent.CheckActivate(originator, instigator, bTest): enabled, bound to that
    // originator (if it has one), trigger count left; the host fires it on its Kismet tick
    add("sequenceevent.checkactivate", |vm, c| {
        let Some(ev) = c.this else { return Value::Bool(false) };
        let enabled = !matches!(vm.get(ev, "bEnabled"), Value::Bool(false));
        let max = vm.get(ev, "MaxTriggerCount").int();
        let count = vm.get(ev, "TriggerCount").int();
        let bound = vm.get(ev, "Originator").obj();
        let originator = c.obj(0);
        let ok = enabled && (max <= 0 || count < max) && (bound.is_none() || bound == originator);
        if ok && !c.arg(2).truthy() {
            let inst = c.obj(1).or(originator);
            vm.kismet_activations.push((ev, inst));
        }
        Value::Bool(ok)
    });
    // GGDownloadableContentManager.OwnsAppID: Steam's licence list isn't readable offline and
    // every install carries the DLC files, so DLC counts as not owned (the base game's
    // behaviour) unless GOAT_OWN_DLC=1
    add("ggdownloadablecontentmanager.ownsappid", |_, _| Value::Bool(std::env::var_os("GOAT_OWN_DLC").is_some()));
    // Object.ToHex: "%08X"
    add("object.tohex", |_, c| Value::Str(format!("{:08X}", c.arg(0).int() as u32)));
    // Object.RDiff: |A.Vector() - B.Vector()|
    add("object.rdiff", |vm, c| {
        let rv = |vm: &mut Vm, v: &Value| -> [f32; 3] {
            let (p, y) = match v {
                Value::Struct(s) => (s.field(vm.sym("Pitch")).map(|x| x.int()).unwrap_or(0), s.field(vm.sym("Yaw")).map(|x| x.int()).unwrap_or(0)),
                _ => (0, 0),
            };
            let k = std::f32::consts::PI / 32768.0;
            let (p, y) = (p as f32 * k, y as f32 * k);
            [p.cos() * y.cos(), p.cos() * y.sin(), p.sin()]
        };
        let (a, b) = (c.arg(0).clone(), c.arg(1).clone());
        let (a, b) = (rv(vm, &a), rv(vm, &b));
        Value::Float(((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt())
    });
    // Object.ClockwiseFrom(A, B): A clockwise from B on the 16-bit circle
    add("object.clockwisefrom_intint", |_, c| {
        let (a, b) = (c.arg(0).int() & 0xffff, c.arg(1).int() & 0xffff);
        Value::Bool(if (a - b).abs() > 0x8000 { a < b } else { a > b })
    });
    // Object.IsChildState(TestState, TestParentState): the parent is TestState or one it extends
    add("object.ischildstate", |vm, c| {
        let (Value::Name(t), Value::Name(p)) = (c.arg(0).clone(), c.arg(1).clone()) else { return Value::Bool(false) };
        if t == p {
            return Value::Bool(true);
        }
        let Some(o) = c.this else { return Value::Bool(false) };
        let class = vm.obj(o).class;
        Value::Bool(vm.state_extends(class, t, p))
    });
    add("worldinfo.isinseamlesstravel", |_, _| Value::Bool(false));
    add("playercontroller.hasclientloadedcurrentworld", |_, _| Value::Bool(true));
    add("playercontroller.getplayernetworkaddress", |_, _| Value::Str(String::new()));
    add("gameinfo.shouldstartincinematicmode", |_, _| Value::Bool(false));
    // the view target is the pawn (the game camera follows the goat or its vehicle)
    add("playercontroller.getviewtarget", |vm, c| Value::Obj(c.this.and_then(|o| vm.get(o, "ViewTarget").obj().or_else(|| vm.get(o, "Pawn").obj()))));
    add("playercontroller.setviewtarget", |vm, c| {
        if let Some(o) = c.this {
            let t = c.arg(0).clone();
            vm.set(o, "ViewTarget", t);
        }
        Value::Int(0)
    });
    // no effect here: LOD, tick, rendering and streaming switches the Rust side owns
    for k in [
        "ggpawnbase.setlodonggkactorattachments",
        "culldistancevolume.forceupdate",
        "onlinestatswrite.setintstat",
        "rb_handle.releasecomponent",
        "primitivecomponent.setdepthprioritygroup",
        "pawn.lockdesiredrotation",
        "particlesystemcomponent.setlodlevel",
        "ggmapinfo.streaminlevels",
        "actor.setonlyownersee",
        "actor.settickisdisabled",
        "seqact_interp.addplayertodirectortracks",
    ] {
        add(k, |_, _| Value::Int(0));
    }
    // InternetLink.Resolve: no network here, so the lookup fails like an offline game's would
    // (GGTcpLinkNewsFeed.ResolveFailed -> ShowNews with its "Failed To Load News!" text). It is
    // delivered half a second later by the menu tick (menus.rs; the world's timers are paused
    // under the menu), after the main menu's widgets initialised, as a real lookup would arrive.
    add("internetlink.resolve", |_, c| {
        if let (Some(o), Ok(mut q)) = (c.this, RESOLVE_FAILED.lock()) {
            q.push((o, 0.5));
        }
        Value::Int(0)
    });
    // the race jockey is a Rust rig on the goat's RideSocket (rider.rs)
    vm.overrides.insert("gggoat.spawngoatrider".into(), |_, _| {
        crate::rider::request(true);
        Value::Int(0)
    });
    vm.overrides.insert("gggoat.removegoatrider".into(), |_, _| {
        crate::rider::request(false);
        Value::Int(0)
    });
    // the in-game name of a prop the goat licked (sync_in), else the actor's own name
    vm.overrides.insert("ggkactorbase.getactorname".into(), |vm, c| {
        let Some(t) = c.this else { return Value::Str(String::new()) };
        match vm.get(t, "uvmActorName") {
            Value::Str(s) if !s.is_empty() => Value::Str(s),
            _ => Value::Str(vm.obj(t).name.clone()),
        }
    });
    // SetLocation already moves the proxy's body (proxies.rs)
    add("primitivecomponent.setrbposition", |_, _| Value::Int(0));
    add("skeletalmeshcomponent.getsocketbyname", |_, _| Value::Obj(None));
    add("skeletalmeshcomponent.detachcomponent", |vm, c| {
        if let Some(o) = c.obj(0) {
            vm.events.push(HostEvent::Detach(o));
        }
        Value::Int(0)
    });
    add("actor.detachcomponent", |vm, c| {
        if let Some(o) = c.obj(0) {
            vm.events.push(HostEvent::Detach(o));
        }
        Value::Int(0)
    });
    add("staticmeshcomponent.setstaticmesh", |vm, c| {
        let m = c.arg(0).clone();
        store(vm, c, "StaticMesh", m);
        Value::Bool(true)
    });
    add("skeletalmeshcomponent.playanim", |vm, c| {
        let a = vm.to_string(c.arg(0));
        vm.say(format!("PlayAnim({a})"));
        Value::Int(0)
    });
    add("skeletalmeshcomponent.stopanim", |_, _| Value::Int(0));
    // --- world / engine -------------------------------------------------------------------
    add("worldinfo.getdetailmode", |_, _| Value::Byte(2));
    add("ggkactorbase.isinstreaminglevel", |_, _| Value::Bool(false));
    add("gameengine.getdlcmanager", |vm, _| {
        if let Some(&o) = vm.globals.get("dlcmanager") {
            return Value::Obj(Some(o));
        }
        let Some(c) = vm.class_by_name("GGDownloadableContentManager") else { return Value::Obj(None) };
        let o = vm.spawn_object(c, None);
        vm.globals.insert("dlcmanager", o);
        Value::Obj(Some(o))
    });
    // GGEngine.CSSDLCManager (GGCSSDLCManager, config(DLC): the maps and mutators of the base
    // game and every installed DLC)
    add("ggengine.getcssdlcmanager", |vm, _| {
        if let Some(&o) = vm.globals.get("cssdlcmanager") {
            return Value::Obj(Some(o));
        }
        let Some(c) = vm.class_by_name("GGCSSDLCManager") else { return Value::Obj(None) };
        let o = vm.spawn_object(c, None);
        vm.globals.insert("cssdlcmanager", o);
        if let Some(&e) = vm.globals.get("engine") {
            vm.set(e, "CSSDLCManager", Value::Obj(Some(o)));
        }
        Value::Obj(Some(o))
    });
    // a map/package by file name in CookedPC (or Maps/), any extension
    add("ggengine.doespackagefileexist", |_, c| {
        let name = c.str(0).to_ascii_lowercase();
        let dir = upk::cooked_dir();
        let found = [dir.clone(), dir.join("Maps")].iter().any(|d| {
            std::fs::read_dir(d).into_iter().flatten().flatten().any(|e| {
                let f = e.file_name().to_string_lossy().to_ascii_lowercase();
                f.rsplit_once('.').map(|(stem, _)| stem == name).unwrap_or(false) || f == name
            })
        });
        Value::Bool(found)
    });
    add("engine.getaudiodevice", |_, _| Value::Obj(None));
    add("actor.getalocalplayercontroller", |vm, _| {
        let g = vm.globals.get("goat").copied();
        Value::Obj(g.and_then(|g| vm.get(g, "Controller").obj()))
    });
    add("actor.consolecommand", |vm, c| {
        let s = c.str(0);
        Value::Str(console_command(vm, &s))
    });
    add("playercontroller.consolecommand", |vm, c| {
        let s = c.str(0);
        Value::Str(console_command(vm, &s))
    });
    add("object.ispendingkill", |vm, c| Value::Bool(c.this.is_some_and(|o| vm.obj(o).destroyed)));
    add("audiocomponent.isplaying", |vm, c| Value::Bool(c.this.is_some_and(|o| vm.get(o, "uvmPlaying").truthy())));
    add("localplayer.overridepostprocesssettings", |vm, _| {
        vm.say("OverridePostProcessSettings (post-process not applied)".into());
        Value::Int(0)
    });
    add("localplayer.clearpostprocesssettingsoverride", |_, _| Value::Int(0));
    add("worldinfo.myemitterpool", |_, _| Value::Obj(None));
    // traces against the game's physics world (vm.tracer); nothing hit -> HitLocation = TraceEnd
    add("actor.trace", |vm, c| {
        let end = vm.vec3(c.arg(2));
        let start = if c.given(3) { vm.vec3(c.arg(3)) } else { c.this.map(|o| vm.get(o, "Location")).map(|l| vm.vec3(&l)).unwrap_or_default() };
        match vm.tracer.as_ref().and_then(|t| t(start, end)) {
            Some(hit) => {
                c.args[0] = vm.vector(hit.location[0], hit.location[1], hit.location[2]);
                c.args[1] = vm.vector(hit.normal[0], hit.normal[1], hit.normal[2]);
                // NPCs and props get their script actor the first time a trace finds them
                let actor = hit.actor.or_else(|| hit.thing.as_ref().map(|t| vm.proxy_for(t, hit.location)));
                Value::Obj(actor.or_else(|| vm.globals.get("worldinfo").copied()))
            }
            None => {
                c.args[0] = c.arg(2).clone();
                c.args[1] = vm.vector(0.0, 0.0, 1.0);
                Value::Obj(None)
            }
        }
    });
    add("actor.tracecomponent", |_, _| Value::Bool(false));
    add("actor.fasttrace", |vm, c| {
        let end = vm.vec3(c.arg(0));
        let start = if c.given(1) { vm.vec3(c.arg(1)) } else { c.this.map(|o| vm.get(o, "Location")).map(|l| vm.vec3(&l)).unwrap_or_default() };
        Value::Bool(vm.tracer.as_ref().and_then(|t| t(start, end)).filter(|h| h.actor.is_none()).is_none())
    });
    // audio
    add("actor.createaudiocomponent", |vm, c| {
        let Some(class) = vm.class_by_name("AudioComponent") else { return Value::Obj(None) };
        let a = vm.spawn_object(class, None);
        vm.obj_mut(a).outer = c.this;
        let cue = c.arg(0).clone();
        vm.set(a, "SoundCue", cue);
        Value::Obj(Some(a))
    });
    add("audiocomponent.play", |vm, c| {
        if let Some(o) = c.this {
            vm.set(o, "uvmPlaying", Value::Bool(true));
        }
        let cue = c.this.map(|o| vm.get(o, "SoundCue")).unwrap_or(Value::Obj(None));
        let s = vm.describe(&cue);
        vm.say(format!("AudioComponent.Play({s})"));
        // music tracks stream through ambient.rs (GGMusicManager's queue), not as one-shots
        if !is_music_track(vm, &cue) {
            vm.events.push(HostEvent::PlaySound { cue: s });
        }
        Value::Int(0)
    });
    add("audiocomponent.fadein", |vm, c| {
        let cue = c.this.map(|o| vm.get(o, "SoundCue")).unwrap_or(Value::Obj(None));
        let s = vm.describe(&cue);
        vm.events.push(HostEvent::PlaySound { cue: s });
        Value::Int(0)
    });
    add("audiocomponent.stop", |vm, c| store(vm, c, "uvmPlaying", Value::Bool(false)));
    add("audiocomponent.fadeout", |_, _| Value::Int(0));
}

/// Script functions replaced by Rust (messages the player should see).
/// InternetLink objects whose Resolve fails after the given seconds of menu time.
pub static RESOLVE_FAILED: std::sync::Mutex<Vec<(ObjId, f32)>> = std::sync::Mutex::new(Vec::new());

pub fn register_overrides(vm: &mut Vm) {
    super::leaderboard::register(vm);
    // GGSVehicle.TryToDrive on a script-spawned vehicle (the Shopping Goat cart): vehicle.rs
    // builds it from its class defaults and puts the goat in
    vm.overrides.insert("ggsvehicle.trytodrive".into(), |vm, c| {
        let Some(t) = c.this else { return Value::Bool(false) };
        let loc = vm.get(t, "Location");
        let l = vm.vec3(&loc);
        crate::vehicle::vm_try_to_drive(crate::vehicle::ScriptDrive {
            class: vm.class_name(vm.obj(t).class).to_string(),
            name: vm.obj(t).name.clone(),
            loc: crate::convert::pos(l),
            locked: vm.get(t, "mUsedByMutator").truthy(),
        });
        Value::Bool(true)
    });
    let o = &mut vm.overrides;
    // GGTrickKismet: a GiveScore action whose targets include the goat (or none) scores a trick
    o.insert("gggameinfo.onkismetactivated".into(), |vm, c| {
        let Some(op) = c.obj(0) else { return Value::Int(0) };
        let Some(gs) = vm.class_by_name("GGSeqAct_GiveScore") else { return Value::Int(0) };
        if !vm.is_child_of(vm.obj(op).class, gs) {
            return Value::Int(0);
        }
        let goat = vm.globals.get("goat").copied();
        let targets: Vec<Option<ObjId>> = match vm.get(op, "Targets") {
            Value::Array(a) => a.iter().map(|v| v.obj()).collect(),
            _ => vec![],
        };
        if !targets.is_empty() && !targets.contains(&goat) {
            return Value::Int(0);
        }
        let name = vm.get(op, "mTrickName");
        let name = super::kismet::plain_trick_name(name.str());
        let score = vm.get(op, "mScore").int();
        let special = vm.get(op, "mIsSpecial").truthy();
        vm.say(format!("kismet trick {name:?} +{score}"));
        vm.events.push(HostEvent::KismetScore { name, score, special });
        Value::Int(0)
    });
    o.insert("gameinfo.broadcast".into(), |vm, c| {
        let s = c.str(1);
        vm.say(format!("Broadcast: {s}"));
        vm.events.push(HostEvent::Message(s));
        Value::Int(0)
    });
    for k in ["playercontroller.teammessage", "ggplayercontrollergame.teammessage", "ggplayercontrollerbase.teammessage"] {
        o.insert(k.into(), |vm, c| {
            let s = c.str(1);
            vm.say(format!("TeamMessage: {s}"));
            vm.events.push(HostEvent::Message(s));
            Value::Int(0)
        });
    }
}

/// Console commands from script: "set <Class> <Property> <Value>" sets the property on the
/// class default and every live object of that class (like UE3's `set`); others are logged.
pub fn console_command(vm: &mut Vm, cmd: &str) -> String {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    if parts.len() >= 4 && parts[0].eq_ignore_ascii_case("set") {
        let Some(class) = vm.class_by_name(parts[1]) else {
            vm.say(format!("ConsoleCommand({cmd:?}): no class {}", parts[1]));
            return String::new();
        };
        let prop = vm.sym(parts[2]);
        let Some(pd) = vm.find_prop(class, prop).cloned() else {
            vm.say(format!("ConsoleCommand({cmd:?}): no property {}", parts[2]));
            return String::new();
        };
        let text = parts[3..].join(" ");
        let v = vm.import_text(&text, &pd.ty);
        let cdo = vm.cdo(class);
        let mut n = 0;
        for o in 0..vm.objects.len() as ObjId {
            let ob = vm.obj(o);
            if matches!(ob.kind, Kind::Instance) && !ob.destroyed && (o == cdo || vm.is_child_of(ob.class, class)) {
                vm.obj_mut(o).props.insert(prop, v.clone());
                n += 1;
            }
        }
        vm.say(format!("ConsoleCommand({cmd:?}): set on {n} object(s)"));
        return String::new();
    }
    vm.say(format!("ConsoleCommand({cmd:?}) (not run)"));
    vm.events.push(HostEvent::Console(cmd.to_string()));
    String::new()
}

/// Is `cue` one of GGMusicManager's queued tracks?
fn is_music_track(vm: &mut Vm, cue: &Value) -> bool {
    let Some(&mm) = vm.globals.get("musicmanager") else { return false };
    let Value::Array(tracks) = vm.get(mm, "mTracks") else { return false };
    let k = vm.sym("Cue");
    tracks.iter().any(|t| matches!(t, Value::Struct(s) if s.field(k) == Some(cue)))
}
