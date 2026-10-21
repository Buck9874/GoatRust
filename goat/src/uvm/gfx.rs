//! Scaleform GFx in the VM: GFxMoviePlayer / GFxObject natives backed by the `gfxui` crate
//! (Ruffle). A GFxMoviePlayer's MovieInfo (SwfMovie RawData) becomes a Ruffle movie; every
//! GFxObject the scripts get (GetVariableObject, AttachMovie, GetObject, CreateObject...) is a
//! fresh script object of the requested class, Outer = its movie player (GFxObject is
//! `within GFxMoviePlayer`), holding a GC handle to the AS3 value.
//!
//! ActionScript calls back into script three ways, all queued by the bridge and run right after
//! the Ruffle call that caused them (`pump`):
//! * functions made from UnrealScript delegates (ActionScriptSetFunction(On), SetFunction):
//!   CLIK's addEventListener in GFxClikWidget.AddEventListener ends up here; AS arguments are
//!   converted to the delegate's parameter types (structs member by member, `_this` = the object);
//! * Scaleform's CLIK init callback (Extensions.CLIK_addedToStageCallback) -> WidgetInitialized,
//!   and PostWidgetInit after the Advance that initialised widgets;
//! * ExternalInterface.call(name, ...) -> function `name` on the movie's ExternalInterface object.

use super::exec::Call;
use super::*;
use gfxui::{Handle, HostCall, Value as G};
use std::path::PathBuf;

pub struct MovieSlot {
    pub movie: gfxui::Movie,
    /// A widget ran its init callback since the last PostWidgetInit.
    widgets_initialized: bool,
    pub frames: u64,
    /// The CLIK init callback (host function id) and whether any widget announced itself yet.
    init_fn: u64,
    post_init: bool,
}

enum HostFn {
    /// Call `func` on `target` with the AS arguments.
    Delegate { movie: ObjId, target: ObjId, func: Sym },
    WidgetInit { movie: ObjId },
}

#[derive(Default)]
pub struct Gfx {
    pub movies: HashMap<ObjId, MovieSlot>,
    /// Open movies, drawn back to front; input goes to the last one.
    pub order: Vec<ObjId>,
    objs: HashMap<ObjId, (ObjId, Handle)>,
    fns: HashMap<u64, HostFn>,
    next_fn: u64,
    /// Viewport size movies render at.
    pub size: (u32, u32),
    dir: Option<PathBuf>,
    fonts: Option<(Vec<u8>, Vec<(String, String)>)>,
    pub log_calls: bool,
}

// Ruffle players and GC handles hold non-atomic reference counts. The VM (and this state with
// it) is only ever used by one thread at a time: VM calls run inside `Host::with_vm` from systems
// holding `ResMut<Mutators>`, and nothing reads `Gfx` through a shared reference.
unsafe impl Send for Gfx {}
unsafe impl Sync for Gfx {}

const IE_PRESSED: u8 = 0;
const IE_RELEASED: u8 = 1;

pub fn register(vm: &mut Vm) {
    let n = &mut vm.natives;
    let mut add = |k: &str, f: NativeFn| {
        n.insert(k.to_ascii_lowercase(), f);
    };
    // --- GFxMoviePlayer ---------------------------------------------------------------------
    add("gfxmovieplayer.start", |vm, c| Value::Bool(c.this.is_some_and(|m| start(vm, m))));
    add("gfxmovieplayer.advance", |vm, c| {
        if let Some(m) = c.this {
            advance(vm, m, c.float(0));
        }
        Value::Int(0)
    });
    add("gfxmovieplayer.postadvance", |vm, c| {
        if let Some(m) = c.this {
            call_delegate_prop(vm, m, "OnPostAdvance", vec![Value::Float(c.float(0))]);
        }
        Value::Int(0)
    });
    add("gfxmovieplayer.close", |vm, c| {
        if let Some(m) = c.this {
            close(vm, m, c.arg(0).truthy() || !c.given(0));
        }
        Value::Int(0)
    });
    for k in [
        "setpause", "setpriority", "settimingmode", "setviewport", "setviewscalemode", "setalignment", "setview3d", "setperspective3d",
        "setmoviecanreceivefocus", "setmoviecanreceiveinput", "addcapturekey", "clearcapturekeys", "addfocusignorekey",
        "clearfocusignorekeys", "flushplayerinput", "flushplayerinputindex", "setwidgetpathbinding", "updatesplitscreenlayout",
    ] {
        add(&format!("gfxmovieplayer.{k}"), |_, _| Value::Int(0));
    }
    add("gfxmovieplayer.setexternaltexture", |_, _| Value::Bool(true));
    add("gfxmovieplayer.getavmversion", |_, _| Value::Int(2));
    add("gfxmovieplayer.getgameviewportclient", |vm, _| Value::Obj(vm.globals.get("viewport").copied()));
    add("gfxmovieplayer.getvisibleframerect", |_, c| {
        c.args[0] = Value::Float(0.0);
        c.args[1] = Value::Float(0.0);
        c.args[2] = Value::Float(1920.0);
        c.args[3] = Value::Float(1080.0);
        Value::Int(0)
    });
    add("gfxmovieplayer.getvariable", |vm, c| {
        let v = var_path(vm, c.this, &c.str(0));
        ret_struct(vm, c, v)
    });
    add("gfxmovieplayer.getvariablebool", |vm, c| {
        let v = var_path(vm, c.this, &c.str(0));
        from_as(vm, c.this, v, &Ty::Bool, None)
    });
    add("gfxmovieplayer.getvariablenumber", |vm, c| {
        let v = var_path(vm, c.this, &c.str(0));
        from_as(vm, c.this, v, &Ty::Float, None)
    });
    add("gfxmovieplayer.getvariableint", |vm, c| {
        let v = var_path(vm, c.this, &c.str(0));
        from_as(vm, c.this, v, &Ty::Int, None)
    });
    add("gfxmovieplayer.getvariablestring", |vm, c| {
        let v = var_path(vm, c.this, &c.str(0));
        from_as(vm, c.this, v, &Ty::Str, None)
    });
    add("gfxmovieplayer.getvariableobject", |vm, c| {
        let v = var_path(vm, c.this, &c.str(0));
        let class = vm.as_class(c.arg(1));
        from_as(vm, c.this, v, &Ty::Object, class)
    });
    for k in ["setvariable", "setvariablebool", "setvariablenumber", "setvariableint", "setvariablestring", "setvariableobject"] {
        add(&format!("gfxmovieplayer.{k}"), |vm, c| {
            let Some(m) = c.this else { return Value::Int(0) };
            let v = to_as(vm, m, c.arg(1), None);
            let path = c.str(0);
            let r = with_movie(vm, m, |mv| mv.set_path(&path, v));
            report(vm, "SetVariable", &path, r);
            Value::Int(0)
        });
    }
    add("gfxmovieplayer.getvariablearray", get_variable_array);
    add("gfxmovieplayer.getvariableintarray", get_variable_array);
    add("gfxmovieplayer.getvariablefloatarray", get_variable_array);
    add("gfxmovieplayer.getvariablestringarray", get_variable_array);
    add("gfxmovieplayer.setvariablearray", set_variable_array);
    add("gfxmovieplayer.setvariableintarray", set_variable_array);
    add("gfxmovieplayer.setvariablefloatarray", set_variable_array);
    add("gfxmovieplayer.setvariablestringarray", set_variable_array);
    add("gfxmovieplayer.createobject", |vm, c| {
        let Some(m) = c.this else { return Value::Obj(None) };
        let class = c.str(0);
        let args: Vec<G> = match c.arg(2) {
            Value::Array(a) => a.clone().iter().map(|v| to_as(vm, m, v, None)).collect(),
            _ => vec![],
        };
        let r = with_movie(vm, m, |mv| mv.construct(&class, &args));
        let wanted = vm.as_class(c.arg(1));
        match r {
            Ok(h) => Value::Obj(Some(wrap(vm, m, h, wanted))),
            Err(e) => {
                vm.say(format!("GFx CreateObject({class}): {e}"));
                Value::Obj(None)
            }
        }
    });
    add("gfxmovieplayer.createarray", |vm, c| {
        let Some(m) = c.this else { return Value::Obj(None) };
        match with_movie(vm, m, |mv| mv.construct("Array", &[])) {
            Ok(h) => Value::Obj(Some(wrap(vm, m, h, None))),
            Err(_) => Value::Obj(None),
        }
    });
    add("gfxmovieplayer.actionscriptsetfunction", |vm, c| {
        // (GFxObject Object, string Member): the calling function's delegate parameter
        let (Some(m), Some(target)) = (c.this, c.obj(0)) else { return Value::Int(0) };
        set_function_from_caller(vm, m, target, &c.str(1));
        Value::Int(0)
    });
    add("gfxmovieplayer.invoke", |vm, c| {
        let Some(m) = c.this else { return Value::Int(0) };
        let args = as_value_args(vm, m, c.arg(1));
        let v = call_path(vm, m, &c.str(0), args);
        ret_struct(vm, c, v)
    });
    add("gfxmovieplayer.actionscriptvoid", |vm, c| movie_as_call(vm, c, &Ty::Unknown));
    add("gfxmovieplayer.actionscriptint", |vm, c| movie_as_call(vm, c, &Ty::Int));
    add("gfxmovieplayer.actionscriptfloat", |vm, c| movie_as_call(vm, c, &Ty::Float));
    add("gfxmovieplayer.actionscriptstring", |vm, c| movie_as_call(vm, c, &Ty::Str));
    add("gfxmovieplayer.actionscriptobject", |vm, c| movie_as_call(vm, c, &Ty::Object));
    add("gfxmovieplayer.actionscriptconstructor", |vm, c| {
        let Some(m) = c.this.and_then(|o| movie_of(vm, o)) else { return Value::Obj(None) };
        let args = caller_args(vm, m);
        let class = c.str(0);
        match with_movie(vm, m, |mv| mv.construct(&class, &args)) {
            Ok(h) => Value::Obj(Some(wrap(vm, m, h, None))),
            Err(e) => {
                vm.say(format!("GFx ActionScriptConstructor({class}): {e}"));
                Value::Obj(None)
            }
        }
    });

    // --- GFxObject ----------------------------------------------------------------------------
    add("gfxobject.get", |vm, c| {
        let v = member(vm, c.this, &c.str(0));
        ret_struct(vm, c, v)
    });
    add("gfxobject.getbool", |vm, c| {
        let v = member(vm, c.this, &c.str(0));
        from_as(vm, c.this, v, &Ty::Bool, None)
    });
    add("gfxobject.getfloat", |vm, c| {
        let v = member(vm, c.this, &c.str(0));
        from_as(vm, c.this, v, &Ty::Float, None)
    });
    add("gfxobject.getint", |vm, c| {
        let v = member(vm, c.this, &c.str(0));
        from_as(vm, c.this, v, &Ty::Int, None)
    });
    add("gfxobject.getstring", |vm, c| {
        let v = member(vm, c.this, &c.str(0));
        from_as(vm, c.this, v, &Ty::Str, None)
    });
    add("gfxobject.gettext", |vm, c| {
        let v = member(vm, c.this, "text");
        from_as(vm, c.this, v, &Ty::Str, None)
    });
    add("gfxobject.getobject", |vm, c| {
        let v = member(vm, c.this, &c.str(0));
        let class = vm.as_class(c.arg(1));
        from_as(vm, c.this, v, &Ty::Object, class)
    });
    for k in ["set", "setbool", "setfloat", "setint", "setstring", "setobject"] {
        add(&format!("gfxobject.{k}"), |vm, c| {
            let v = match c.arg(1) {
                Value::Str(s) if s.contains("<Controller:") => Value::Str(controller_tags(vm, s)),
                v => v.clone(),
            };
            set_member(vm, c.this, &c.str(0), &v);
            Value::Int(0)
        });
    }
    add("gfxobject.settext", |vm, c| {
        let s = vm.to_string(c.arg(0));
        let v = Value::Str(if s.contains("<Controller:") { controller_tags(vm, &s) } else { s });
        set_member(vm, c.this, "text", &v);
        Value::Int(0)
    });
    add("gfxobject.translatestring", |vm, c| Value::Str(controller_tags(vm, &c.str(0))));
    add("gfxobject.setfunction", |vm, c| {
        // SetFunction(Member, Object context, name fname)
        let (Some(o), Some(target)) = (c.this, c.obj(1)) else { return Value::Int(0) };
        let Some(m) = movie_of(vm, o) else { return Value::Int(0) };
        let Value::Name(f) = c.arg(2).clone() else { return Value::Int(0) };
        let id = new_fn(vm, HostFn::Delegate { movie: m, target, func: f });
        if let Ok(h) = with_movie(vm, m, |mv| mv.host_function(id)) {
            set_handle_member(vm, o, &c.str(0), G::Object(h));
        }
        Value::Int(0)
    });
    add("gfxobject.actionscriptsetfunction", |vm, c| {
        let Some(o) = c.this else { return Value::Int(0) };
        let Some(m) = movie_of(vm, o) else { return Value::Int(0) };
        set_function_from_caller(vm, m, o, &c.str(0));
        Value::Int(0)
    });
    add("gfxobject.actionscriptsetfunctionon", |vm, c| {
        let (Some(o), Some(target)) = (c.this, c.obj(0)) else { return Value::Int(0) };
        let Some(m) = movie_of(vm, o) else { return Value::Int(0) };
        set_function_from_caller(vm, m, target, &c.str(1));
        Value::Int(0)
    });
    add("gfxobject.invoke", |vm, c| {
        let Some(o) = c.this else { return Value::Int(0) };
        let Some(m) = movie_of(vm, o) else { return Value::Int(0) };
        let args = as_value_args(vm, m, c.arg(1));
        let v = invoke(vm, o, &c.str(0), args);
        ret_struct(vm, c, v)
    });
    add("gfxobject.actionscriptvoid", |vm, c| obj_as_call(vm, c, &Ty::Unknown));
    add("gfxobject.actionscriptint", |vm, c| obj_as_call(vm, c, &Ty::Int));
    add("gfxobject.actionscriptfloat", |vm, c| obj_as_call(vm, c, &Ty::Float));
    add("gfxobject.actionscriptstring", |vm, c| obj_as_call(vm, c, &Ty::Str));
    add("gfxobject.actionscriptobject", |vm, c| obj_as_call(vm, c, &Ty::Object));
    add("gfxobject.actionscriptarray", |vm, c| {
        let Some(o) = c.this else { return Value::Array(vec![]) };
        let Some(m) = movie_of(vm, o) else { return Value::Array(vec![]) };
        let args = caller_args(vm, m);
        let v = invoke(vm, o, &c.str(0), args);
        array_objects(vm, m, v)
    });
    add("gfxobject.gotoandplay", |vm, c| goto(vm, c, true));
    add("gfxobject.gotoandplayi", |vm, c| goto(vm, c, true));
    add("gfxobject.gotoandstop", |vm, c| goto(vm, c, false));
    add("gfxobject.gotoandstopi", |vm, c| goto(vm, c, false));
    add("gfxobject.attachmovie", |vm, c| {
        let Some(o) = c.this else { return Value::Obj(None) };
        let Some((m, parent)) = vm.gfx.objs.get(&o).cloned() else { return Value::Obj(None) };
        let (symbol, name) = (c.str(0), c.str(1));
        let depth = if c.given(2) { c.int(2) } else { -1 };
        let class = vm.as_class(c.arg(3));
        let r = with_movie(vm, m, |mv| mv.attach(&parent, &symbol, &name, depth));
        match r {
            Ok(h) => Value::Obj(Some(wrap(vm, m, h, class))),
            Err(e) => {
                vm.say(format!("GFx AttachMovie({symbol}, {name}): {e}"));
                Value::Obj(None)
            }
        }
    });
    add("gfxobject.createemptymovieclip", |vm, c| {
        let Some(o) = c.this else { return Value::Obj(None) };
        let Some((m, parent)) = vm.gfx.objs.get(&o).cloned() else { return Value::Obj(None) };
        let name = c.str(0);
        let depth = if c.given(1) { c.int(1) } else { -1 };
        let class = vm.as_class(c.arg(2));
        match with_movie(vm, m, |mv| mv.create_empty_movie_clip(&parent, &name, depth)) {
            Ok(h) => Value::Obj(Some(wrap(vm, m, h, class))),
            Err(_) => Value::Obj(None),
        }
    });
    add("gfxobject.setvisible", |vm, c| {
        set_member(vm, c.this, "visible", &Value::Bool(c.arg(0).truthy()));
        Value::Int(0)
    });
    add("gfxobject.setposition", |vm, c| {
        set_member(vm, c.this, "x", &Value::Float(c.float(0)));
        set_member(vm, c.this, "y", &Value::Float(c.float(1)));
        Value::Int(0)
    });
    add("gfxobject.getposition", |vm, c| {
        let x = member(vm, c.this, "x");
        let y = member(vm, c.this, "y");
        c.args[0] = from_as(vm, c.this, x, &Ty::Float, None);
        c.args[1] = from_as(vm, c.this, y, &Ty::Float, None);
        Value::Bool(true)
    });
    add("gfxobject.getdisplayinfo", |vm, c| {
        let o = c.this;
        display_info(vm, c, o)
    });
    add("gfxobject.setdisplayinfo", |vm, c| {
        let d = c.arg(0).clone();
        set_display_info(vm, c.this, &d);
        Value::Int(0)
    });
    add("gfxobject.getcolortransform", |vm, c| ret_zero_struct(vm, c));
    add("gfxobject.setcolortransform", |_, _| Value::Int(0));
    add("gfxobject.getdisplaymatrix", |vm, c| ret_zero_struct(vm, c));
    add("gfxobject.getdisplaymatrix3d", |vm, c| ret_zero_struct(vm, c));
    add("gfxobject.setdisplaymatrix", |_, _| Value::Int(0));
    add("gfxobject.setdisplaymatrix3d", |_, _| Value::Int(0));
    // array elements: the same accessors on member "<index>"
    add("gfxobject.getelement", |vm, c| {
        let v = member(vm, c.this, &c.int(0).to_string());
        ret_struct(vm, c, v)
    });
    add("gfxobject.getelementobject", |vm, c| {
        let v = member(vm, c.this, &c.int(0).to_string());
        let class = vm.as_class(c.arg(1));
        from_as(vm, c.this, v, &Ty::Object, class)
    });
    add("gfxobject.getelementbool", |vm, c| {
        let v = member(vm, c.this, &c.int(0).to_string());
        from_as(vm, c.this, v, &Ty::Bool, None)
    });
    add("gfxobject.getelementfloat", |vm, c| {
        let v = member(vm, c.this, &c.int(0).to_string());
        from_as(vm, c.this, v, &Ty::Float, None)
    });
    add("gfxobject.getelementint", |vm, c| {
        let v = member(vm, c.this, &c.int(0).to_string());
        from_as(vm, c.this, v, &Ty::Int, None)
    });
    add("gfxobject.getelementstring", |vm, c| {
        let v = member(vm, c.this, &c.int(0).to_string());
        from_as(vm, c.this, v, &Ty::Str, None)
    });
    for k in ["setelement", "setelementobject", "setelementbool", "setelementfloat", "setelementint", "setelementstring"] {
        add(&format!("gfxobject.{k}"), |vm, c| {
            let v = c.arg(1).clone();
            set_member(vm, c.this, &c.int(0).to_string(), &v);
            Value::Int(0)
        });
    }
    add("gfxobject.getelementmember", |vm, c| {
        let v = element_member(vm, c.this, c.int(0), &c.str(1));
        ret_struct(vm, c, v)
    });
    add("gfxobject.getelementmemberobject", |vm, c| {
        let v = element_member(vm, c.this, c.int(0), &c.str(1));
        let class = vm.as_class(c.arg(2));
        from_as(vm, c.this, v, &Ty::Object, class)
    });
    add("gfxobject.getelementmemberbool", |vm, c| {
        let v = element_member(vm, c.this, c.int(0), &c.str(1));
        from_as(vm, c.this, v, &Ty::Bool, None)
    });
    add("gfxobject.getelementmemberfloat", |vm, c| {
        let v = element_member(vm, c.this, c.int(0), &c.str(1));
        from_as(vm, c.this, v, &Ty::Float, None)
    });
    add("gfxobject.getelementmemberint", |vm, c| {
        let v = element_member(vm, c.this, c.int(0), &c.str(1));
        from_as(vm, c.this, v, &Ty::Int, None)
    });
    add("gfxobject.getelementmemberstring", |vm, c| {
        let v = element_member(vm, c.this, c.int(0), &c.str(1));
        from_as(vm, c.this, v, &Ty::Str, None)
    });
    for k in ["setelementmember", "setelementmemberobject", "setelementmemberbool", "setelementmemberfloat", "setelementmemberint", "setelementmemberstring"] {
        add(&format!("gfxobject.{k}"), |vm, c| {
            let Some(el) = element(vm, c.this, c.int(0)) else { return Value::Int(0) };
            let v = c.arg(2).clone();
            set_member(vm, Some(el), &c.str(1), &v);
            Value::Int(0)
        });
    }
    add("gfxobject.setelementvisible", |vm, c| {
        if let Some(el) = element(vm, c.this, c.int(0)) {
            set_member(vm, Some(el), "visible", &Value::Bool(c.arg(1).truthy()));
        }
        Value::Int(0)
    });
    add("gfxobject.setelementposition", |vm, c| {
        if let Some(el) = element(vm, c.this, c.int(0)) {
            set_member(vm, Some(el), "x", &Value::Float(c.float(1)));
            set_member(vm, Some(el), "y", &Value::Float(c.float(2)));
        }
        Value::Int(0)
    });
    add("gfxobject.getelementdisplayinfo", |vm, c| {
        let el = element(vm, c.this, c.int(0));
        display_info(vm, c, el)
    });
    add("gfxobject.setelementdisplayinfo", |vm, c| {
        let d = c.arg(1).clone();
        if let Some(el) = element(vm, c.this, c.int(0)) {
            set_display_info(vm, Some(el), &d);
        }
        Value::Int(0)
    });
    add("gfxobject.getelementdisplaymatrix", |vm, c| ret_zero_struct(vm, c));
    add("gfxobject.setelementdisplaymatrix", |_, _| Value::Int(0));
    add("gfxobject.setelementcolortransform", |_, _| Value::Int(0));
}

// --- movies ------------------------------------------------------------------------------------

/// Plain-SWF copies of the movies and decoded img:// textures (shared by all game processes;
/// files are replaced atomically).
fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("goatrust_gfx")
}

/// Write `data` unless the file already holds that many bytes (temp file + rename, so another
/// game process never reads half a file).
fn write_cached(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() == data.len() as u64) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data).map_err(|e| e.to_string())?;
    if std::fs::rename(&tmp, path).is_err() {
        // another process won the race with the same bytes
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

/// SwfMovie RawData by object path ("UI_FrontEnd.FrontEnd_Assets").
fn swf_data(vm: &Vm, path: &str) -> Option<Vec<u8>> {
    let r = vm.reg.find_path(path)?;
    let props = vm.reg.props(r);
    props.into_iter().find(|p| p.name.eq_ignore_ascii_case("RawData")).and_then(|p| match p.value {
        upk::props::Value::Array(_, d) => Some(d),
        _ => None,
    })
}

/// Folder name for a movie package as the movies' relative imports spell it.
fn package_folder(pkg: &str) -> String {
    if pkg.eq_ignore_ascii_case("ui_common") { "UI_Common".into() } else { pkg.to_string() }
}

/// Write the movie and the shared libraries it imports (gfxfontlib) as plain SWFs.
fn write_movie_files(vm: &mut Vm, path: &str) -> Result<(PathBuf, String), String> {
    let dir = vm.gfx.dir.get_or_insert_with(cache_dir).clone();
    let (pkg, name) = path.split_once('.').ok_or(format!("{path}: no package"))?;
    let rel = format!("{}/{name}.swf", package_folder(pkg));
    let data = swf_data(vm, path).ok_or(format!("{path}: no SwfMovie RawData"))?;
    write_cached(&dir.join(&rel), &gfxui::gfx_to_swf(&data))?;
    // shared libraries the movies import (../UI_Common/gfxfontlib.swf)
    for lib in ["ui_common.gfxfontlib"] {
        let (lp, ln) = lib.split_once('.').unwrap();
        if let Some(d) = swf_data(vm, lib) {
            write_cached(&dir.join(package_folder(lp)).join(format!("{ln}.swf")), &gfxui::gfx_to_swf(&d))?;
        }
    }
    Ok((dir, rel))
}

/// Font library + `$Alias=Real Name` pairs from Localization/INT/GfxUI.int.
fn fonts(vm: &mut Vm) -> Option<(Vec<u8>, Vec<(String, String)>)> {
    if vm.gfx.fonts.is_none() {
        let ini = std::fs::read(upk::game_dir().join("GoatGame/Localization/INT/GfxUI.int")).ok().map(|d| decode_text(&d)).unwrap_or_default();
        let mut section = String::new();
        let mut lib = "UI_Common.fonts_en".to_string();
        let mut aliases = vec![];
        for line in ini.lines().map(|l| l.trim()) {
            if line.starts_with(';') || line.is_empty() {
                continue;
            }
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_string();
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            match section.as_str() {
                "FontLib" if k.eq_ignore_ascii_case("FontLib") => lib = v.trim().to_string(),
                "Fonts" => aliases.push((format!("${}", k.trim()), v.trim().to_string())),
                _ => {}
            }
        }
        let data = swf_data(vm, &lib).map(|d| gfxui::gfx_to_swf(&d))?;
        vm.gfx.fonts = Some((data, aliases));
    }
    vm.gfx.fonts.clone()
}

fn decode_text(d: &[u8]) -> String {
    if d.len() >= 2 && d[0] == 0xFF && d[1] == 0xFE {
        let u: Vec<u16> = d[2..].chunks(2).map(|c| u16::from_le_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
        String::from_utf16_lossy(&u)
    } else {
        String::from_utf8_lossy(d).to_string()
    }
}

/// GFxMoviePlayer.Start: load MovieInfo into a Ruffle player.
pub fn start(vm: &mut Vm, m: ObjId) -> bool {
    if vm.gfx.movies.contains_key(&m) {
        if !vm.gfx.order.contains(&m) {
            vm.gfx.order.push(m);
        }
        return true;
    }
    let Some(info) = vm.get(m, "MovieInfo").obj() else {
        vm.say(format!("GFx Start: {} has no MovieInfo", vm.obj(m).name));
        return false;
    };
    let path = match &vm.obj(info).kind {
        Kind::Asset { path, .. } => path.clone(),
        _ => vm.ref_of.get(&info).map(|r| vm.reg.path(*r)).unwrap_or_else(|| vm.obj(info).name.clone()),
    };
    let t = std::time::Instant::now();
    let (dir, rel) = match write_movie_files(vm, &path) {
        Ok(x) => x,
        Err(e) => {
            vm.say(format!("GFx Start {path}: {e}"));
            return false;
        }
    };
    if vm.gfx.size.0 == 0 {
        vm.gfx.size = (1280, 720);
    }
    let (w, h) = vm.gfx.size;
    let images = dir.join("img");
    let _ = std::fs::create_dir_all(&images);
    let movie = match gfxui::Movie::load_file_with_images(&dir, &rel, w, h, Some(&images)) {
        Ok(mv) => mv,
        Err(e) => {
            vm.say(format!("GFx Start {path}: {e:#}"));
            return false;
        }
    };
    movie.set_transparent();
    if let Some((lib, aliases)) = fonts(vm) {
        let al: Vec<(&str, &str)> = aliases.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        if let Err(e) = movie.register_fonts(&lib, &al) {
            vm.say(format!("GFx fonts: {e:#}"));
        }
    }
    gfxui::enable_host_calls(true);
    // Scaleform's CLIK init callback (-> WidgetInitialized) must be in place before the root
    // timeline places its widgets (the popup's mPopupHeader/mPopupBody announce themselves on
    // frame 1), so it is installed as soon as the movie's classes exist, before each early frame
    let widget_init_fn = new_fn(vm, HostFn::WidgetInit { movie: m });
    let mut hooked = false;
    let mut hook = |movie: &gfxui::Movie| {
        if let (Ok(ext), Ok(f)) = (movie.definition("scaleform.gfx.Extensions"), movie.host_function(widget_init_fn)) {
            let _ = movie.set(&ext, "CLIK_addedToStageCallback", G::Object(f));
            let _ = movie.set(&ext, "isGFxPlayer", G::Bool(true));
            return true;
        }
        false
    };
    // imports (the font library) load over the first frames; the root clip exists after that
    for _ in 0..30 {
        if !hooked {
            hooked = hook(&movie);
        }
        movie.run_frame();
        if movie.root().is_ok() {
            break;
        }
    }
    if !hooked {
        hook(&movie);
    }
    // widgets placed in the frame that defined the classes asked for the callback before it
    // existed: replay it for every UIComponent with enableInitCallback on the stage
    let _ = replay_widget_init(&movie, widget_init_fn);
    // the menus' software cursor (root.mCursor, gg.menu.Cursor) follows the pointer; Scaleform
    // never picks it, Ruffle would (it then takes every press from the buttons under it)
    if let Ok(G::Object(cursor)) = movie.path("root.mCursor") {
        let _ = movie.set(&cursor, "mouseEnabled", G::Bool(false));
        let _ = movie.set(&cursor, "mouseChildren", G::Bool(false));
    }
    vm.say(format!("GFx Start {path} ({w}x{h}) in {:.0} ms", t.elapsed().as_secs_f32() * 1000.0));
    vm.gfx.movies.insert(m, MovieSlot { movie, widgets_initialized: false, frames: 0, init_fn: widget_init_fn, post_init: false });
    vm.gfx.order.retain(|&o| o != m);
    vm.gfx.order.push(m);
    vm.set(m, "bMovieIsOpen", Value::Bool(true));
    pump(vm, m);
    true
}

/// Call the CLIK init callback (host function `id`) for every widget on the stage that has
/// enableInitCallback set, with CLIK's (name, "root1.a.b", widget) arguments.
fn replay_widget_init(movie: &gfxui::Movie, id: u64) -> Result<(), String> {
    let f = movie.host_function(id)?;
    let root = movie.root()?;
    let root_name = match movie.get(&root, "name")? {
        G::String(s) => s,
        _ => "root1".into(),
    };
    let mut queue = vec![(root, root_name, 0)];
    while let Some((o, path, depth)) = queue.pop() {
        if depth > 0 && matches!(movie.get(&o, "enableInitCallback"), Ok(G::Bool(true))) {
            let name = match movie.get(&o, "name")? {
                G::String(s) => s,
                _ => String::new(),
            };
            movie.call(&f, &G::Null, &[G::String(name), G::String(path.clone()), G::Object(o.clone())])?;
        }
        if depth < 12 {
            let n = movie.get(&o, "numChildren").map(|v| as_number(&v) as usize).unwrap_or(0);
            for i in 0..n.min(400) {
                if let Ok(G::Object(c)) = movie.invoke(&o, "getChildAt", &[G::Number(i as f64)]) {
                    let cn = match movie.get(&c, "name") {
                        Ok(G::String(s)) => s,
                        _ => String::new(),
                    };
                    queue.push((c, format!("{path}.{cn}"), depth + 1));
                }
            }
        }
    }
    Ok(())
}

/// Advance the movie by `dt` seconds; fires PostWidgetInit when widgets initialised.
pub fn advance(vm: &mut Vm, m: ObjId, dt: f32) {
    let Some(slot) = vm.gfx.movies.get_mut(&m) else { return };
    gfxui::enable_host_calls(true);
    if dt > 0.0 {
        slot.movie.tick(dt as f64);
    } else if slot.frames == 0 {
        slot.movie.run_frame();
    }
    slot.frames += 1;
    // widgets attached by script before the movie's first real frame (the Inventory mutator's
    // HUD widgets on tick 0) can leave every CLIK init callback unheard: when not one widget has
    // announced itself by frame 5, re-announce the widgets on the stage once, as at Start
    if !slot.post_init && slot.frames == 5 {
        let _ = replay_widget_init(&slot.movie, slot.init_fn);
        slot.widgets_initialized = true;
        vm.say("GFx: no widget init by frame 5; replayed".into());
    }
    pump(vm, m);
    let init = vm.gfx.movies.get_mut(&m).map(|s| std::mem::take(&mut s.widgets_initialized)).unwrap_or(false);
    if init {
        if let Some(s) = vm.gfx.movies.get_mut(&m) {
            s.post_init = true;
        }
        if std::env::var_os("GOAT_TRACE_HUD").is_some() {
            println!("    GFx PostWidgetInit");
        }
        let trace = std::mem::replace(&mut vm.trace_calls, std::env::var_os("GOAT_TRACE_HUD").is_some());
        vm.call_named(m, "PostWidgetInit", vec![]);
        vm.trace_calls = trace;
    }
}

pub fn close(vm: &mut Vm, m: ObjId, unload: bool) {
    if !vm.gfx.order.contains(&m) && !vm.gfx.movies.contains_key(&m) {
        return;
    }
    vm.gfx.order.retain(|&o| o != m);
    vm.set(m, "bMovieIsOpen", Value::Bool(false));
    vm.call_named(m, "OnClose", vec![]);
    if unload {
        vm.gfx.movies.remove(&m);
        vm.gfx.objs.retain(|_, (mv, _)| *mv != m);
        vm.gfx.fns.retain(|_, f| match f {
            HostFn::Delegate { movie, .. } | HostFn::WidgetInit { movie } => *movie != m,
        });
    }
}

/// Run `f` on the movie; then deliver the calls ActionScript made into script.
pub(crate) fn with_movie<R>(vm: &mut Vm, m: ObjId, f: impl FnOnce(&gfxui::Movie) -> Result<R, String>) -> Result<R, String> {
    let r = {
        let Some(slot) = vm.gfx.movies.get(&m) else { return Err("movie not started".into()) };
        gfxui::enable_host_calls(true);
        f(&slot.movie)
    };
    pump(vm, m);
    r
}

fn report<T>(vm: &mut Vm, what: &str, name: &str, r: Result<T, String>) {
    if let Err(e) = r {
        vm.missing(format!("GFx {what}({name}): {e}"));
    }
}

fn new_fn(vm: &mut Vm, f: HostFn) -> u64 {
    vm.gfx.next_fn += 1;
    let id = vm.gfx.next_fn;
    vm.gfx.fns.insert(id, f);
    id
}

/// The movie player a GFxObject (or the player itself) belongs to.
fn movie_of(vm: &Vm, o: ObjId) -> Option<ObjId> {
    if vm.gfx.movies.contains_key(&o) {
        return Some(o);
    }
    if let Some((m, _)) = vm.gfx.objs.get(&o) {
        return Some(*m);
    }
    // a GFxObject subclass instance never bound (or its outer chain)
    let mut cur = vm.obj(o).outer;
    while let Some(c) = cur {
        if vm.gfx.movies.contains_key(&c) {
            return Some(c);
        }
        cur = vm.obj(c).outer;
    }
    None
}

/// A new script GFxObject (of `class`, default GFxObject) for an AS3 value.
fn wrap(vm: &mut Vm, m: ObjId, h: Handle, class: Option<usize>) -> ObjId {
    let base = vm.class_by_name("GFxObject").unwrap_or(NONE_CLASS);
    let class = class.filter(|&c| base == NONE_CLASS || vm.is_child_of(c, base)).unwrap_or(base);
    let o = vm.spawn_object(class, None);
    vm.obj_mut(o).outer = Some(m);
    vm.gfx.objs.insert(o, (m, h));
    o
}

// --- value conversion ------------------------------------------------------------------------------

fn as_value_struct_name(vm: &Vm, v: &StructVal) -> bool {
    vm.name_of(v.name).eq_ignore_ascii_case("ASValue")
}

/// A script value as an AS3 value.
fn to_as(vm: &mut Vm, m: ObjId, v: &Value, caller: Option<ObjId>) -> G {
    match v {
        Value::Str(s) => {
            ensure_image(vm, s);
            G::String(s.clone())
        }
        Value::Name(y) => G::String(vm.name_of(*y).to_string()),
        Value::Float(f) => G::Number(*f as f64),
        Value::Int(i) => G::Number(*i as f64),
        Value::Byte(b) => G::Number(*b as f64),
        Value::Bool(b) => G::Bool(*b),
        Value::Obj(Some(o)) => match vm.gfx.objs.get(o) {
            Some((_, h)) => G::Object(h.clone()),
            None => G::Null,
        },
        Value::Obj(None) => G::Null,
        Value::Delegate(t, f) if f.0 != 0 => {
            let Some(target) = t.or(caller) else { return G::Null };
            let id = new_fn(vm, HostFn::Delegate { movie: m, target, func: *f });
            match vm.gfx.movies.get(&m).map(|s| s.movie.host_function(id)) {
                Some(Ok(h)) => G::Object(h),
                _ => G::Null,
            }
        }
        Value::Struct(s) if as_value_struct_name(vm, s) => {
            let get = |n: &str| s.fields.iter().find(|f| vm.name_of(f.0).eq_ignore_ascii_case(n)).map(|f| f.1.clone());
            let t = get("Type").map(|v| v.int()).unwrap_or(0);
            match t {
                1 => G::Null,
                2 => G::Number(get("n").map(|v| v.float()).unwrap_or(0.0) as f64),
                3 => G::Number(get("i").map(|v| v.int()).unwrap_or(0) as f64),
                4 => {
                    let s = get("s").map(|v| v.str().to_string()).unwrap_or_default();
                    ensure_image(vm, &s);
                    G::String(s)
                }
                5 => G::Bool(get("b").is_some_and(|v| v.truthy())),
                _ => G::Undefined,
            }
        }
        _ => G::Undefined,
    }
}

fn num_string(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { format!("{n}") }
}

/// An AS3 value as a script value of type `ty` (`class`: GFxObject subclass for objects).
fn from_as(vm: &mut Vm, m: Option<ObjId>, v: G, ty: &Ty, class: Option<usize>) -> Value {
    let movie = m.and_then(|o| movie_of(vm, o));
    match ty {
        Ty::Str => Value::Str(match v {
            G::String(s) => s,
            G::Number(n) => num_string(n),
            G::Bool(b) => b.to_string(),
            G::Null => "null".into(),
            G::Undefined => String::new(),
            G::Object(h) => movie.and_then(|mm| vm.gfx.movies.get(&mm)).map(|s| s.movie.describe_value(&G::Object(h))).unwrap_or_default(),
        }),
        Ty::Float => Value::Float(as_number(&v) as f32),
        Ty::Int => Value::Int(as_number(&v) as i64 as i32),
        Ty::Byte(_) => Value::Byte(as_number(&v) as i64 as u8),
        Ty::Bool => Value::Bool(match v {
            G::Bool(b) => b,
            G::Number(n) => n != 0.0 && !n.is_nan(),
            G::String(s) => !s.is_empty(),
            G::Object(_) => true,
            _ => false,
        }),
        Ty::Name => {
            let s = match v {
                G::String(s) => s,
                G::Number(n) => num_string(n),
                _ => String::new(),
            };
            Value::Name(vm.sym(&s))
        }
        Ty::Object | Ty::Interface => match (v, movie) {
            (G::Object(h), Some(mm)) => Value::Obj(Some(wrap(vm, mm, h, class))),
            _ => Value::Obj(None),
        },
        Ty::Struct(sd) => {
            let sd = *sd;
            let mut s = vm.struct_zero(sd);
            if vm.name_of(vm.structs[sd].name).eq_ignore_ascii_case("ASValue") {
                fill_as_value(vm, &mut s, &v);
                return Value::Struct(Box::new(s));
            }
            let G::Object(h) = v else { return Value::Struct(Box::new(s)) };
            let fields = vm.structs[sd].fields.clone();
            for f in fields {
                let fname = vm.name_of(f.name).to_string();
                let val = if fname.eq_ignore_ascii_case("_this") {
                    from_as(vm, m, G::Object(h.clone()), &f.ty, None)
                } else {
                    let mv = movie.and_then(|mm| vm.gfx.movies.get(&mm));
                    let got = mv.map(|s| s.movie.get(&h, &fname)).and_then(|r| r.ok()).unwrap_or(G::Undefined);
                    if matches!(got, G::Undefined) {
                        continue;
                    }
                    from_as(vm, m, got, &f.ty, None)
                };
                *s.field_mut(f.name) = val;
            }
            Value::Struct(Box::new(s))
        }
        _ => Value::Int(0),
    }
}

pub(crate) fn as_number(v: &G) -> f64 {
    match v {
        G::Number(n) => *n,
        G::Bool(b) => *b as i32 as f64,
        G::String(s) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn fill_as_value(vm: &mut Vm, s: &mut StructVal, v: &G) {
    let (t, field, val) = match v {
        G::Undefined | G::Object(_) => (0u8, None, Value::Int(0)),
        G::Null => (1, None, Value::Int(0)),
        G::Number(n) => (2, Some("n"), Value::Float(*n as f32)),
        G::String(x) => (4, Some("s"), Value::Str(x.clone())),
        G::Bool(b) => (5, Some("b"), Value::Bool(*b)),
    };
    let ty = vm.sym("Type");
    *s.field_mut(ty) = Value::Byte(t);
    if let Some(f) = field {
        let y = vm.sym(f);
        *s.field_mut(y) = val;
    }
}

/// Return an AS value as the native's declared struct return type (ASValue...).
fn ret_struct(vm: &mut Vm, c: &Call, v: G) -> Value {
    match vm.funcs[c.func].ret.clone() {
        Some(r) => from_as(vm, c.this, v, &r.ty, None),
        None => Value::Int(0),
    }
}

fn ret_zero_struct(vm: &mut Vm, c: &Call) -> Value {
    match vm.funcs[c.func].ret.clone() {
        Some(r) => vm.zero(&r.ty, r.dim),
        None => Value::Int(0),
    }
}

/// `array<ASValue>` arguments.
fn as_value_args(vm: &mut Vm, m: ObjId, v: &Value) -> Vec<G> {
    match v {
        Value::Array(a) => a.clone().iter().map(|x| to_as(vm, m, x, None)).collect(),
        _ => vec![],
    }
}

/// The calling script function's parameters as AS arguments (ActionScriptVoid & co. pass the
/// wrapper function's own parameters).
fn caller_args(vm: &mut Vm, m: ObjId) -> Vec<G> {
    let Some(f) = vm.frames.last() else { return vec![] };
    let this = f.this;
    let vals: Vec<Value> = vm.funcs[f.func].params.iter().map(|p| f.locals.get(&p.name).cloned().unwrap_or(Value::Int(0))).collect();
    vals.iter().map(|v| to_as(vm, m, v, this)).collect()
}

/// The calling function's first delegate parameter as an AS function, set as `member` of
/// `target` (ActionScriptSetFunction / ActionScriptSetFunctionOn).
fn set_function_from_caller(vm: &mut Vm, m: ObjId, target: ObjId, member_name: &str) {
    let Some(f) = vm.frames.last() else { return };
    let this = f.this;
    let func = f.func;
    let d = vm.funcs[func].params.iter().find(|p| p.ty == Ty::Delegate).and_then(|p| vm.frames.last().and_then(|f| f.locals.get(&p.name).cloned()));
    let Some(Value::Delegate(t, fname)) = d else {
        vm.missing(format!("GFx ActionScriptSetFunction: no delegate parameter in {}", vm.funcs[func].display));
        return;
    };
    if fname.0 == 0 {
        return;
    }
    let Some(dt) = t.or(this) else { return };
    let id = new_fn(vm, HostFn::Delegate { movie: m, target: dt, func: fname });
    let r = with_movie(vm, m, |mv| mv.host_function(id));
    if let Ok(h) = r {
        set_handle_member(vm, target, member_name, G::Object(h));
    }
}

fn set_handle_member(vm: &mut Vm, o: ObjId, name: &str, v: G) {
    let Some((m, h)) = vm.gfx.objs.get(&o).cloned() else { return };
    let r = with_movie(vm, m, |mv| mv.set(&h, name, v));
    report(vm, "Set", name, r);
}

// --- object access ---------------------------------------------------------------------------------

fn member(vm: &mut Vm, o: Option<ObjId>, name: &str) -> G {
    let Some((m, h)) = o.and_then(|o| vm.gfx.objs.get(&o).cloned()) else { return G::Undefined };
    match with_movie(vm, m, |mv| mv.get(&h, name)) {
        Ok(v) => v,
        Err(e) => {
            vm.missing(format!("GFx Get({name}): {e}"));
            G::Undefined
        }
    }
}

fn set_member(vm: &mut Vm, o: Option<ObjId>, name: &str, v: &Value) {
    let Some((m, h)) = o.and_then(|o| vm.gfx.objs.get(&o).cloned()) else { return };
    let g = to_as(vm, m, v, o);
    let r = with_movie(vm, m, |mv| mv.set(&h, name, g));
    report(vm, "Set", name, r);
}

fn element(vm: &mut Vm, o: Option<ObjId>, i: i32) -> Option<ObjId> {
    let m = o.and_then(|o| movie_of(vm, o))?;
    match member(vm, o, &i.to_string()) {
        G::Object(h) => Some(wrap(vm, m, h, None)),
        _ => None,
    }
}

fn element_member(vm: &mut Vm, o: Option<ObjId>, i: i32, name: &str) -> G {
    match element(vm, o, i) {
        Some(el) => member(vm, Some(el), name),
        None => G::Undefined,
    }
}

fn invoke(vm: &mut Vm, o: ObjId, method: &str, args: Vec<G>) -> G {
    let Some((m, h)) = vm.gfx.objs.get(&o).cloned() else { return G::Undefined };
    if vm.gfx.log_calls {
        vm.say(format!("GFx {}.{method}({} args)", vm.obj(o).name, args.len()));
    }
    match with_movie(vm, m, |mv| mv.invoke(&h, method, &args)) {
        Ok(v) => v,
        Err(e) => {
            vm.missing(format!("GFx Invoke({method}): {e}"));
            G::Undefined
        }
    }
}

/// Resolve a variable path from the root ("root.mRootObject", "_root.a.b").
fn var_path(vm: &mut Vm, m: Option<ObjId>, path: &str) -> G {
    let Some(m) = m.and_then(|o| movie_of(vm, o)) else { return G::Undefined };
    match with_movie(vm, m, |mv| mv.path(path)) {
        Ok(v) => v,
        Err(e) => {
            vm.missing(format!("GFx GetVariable({path}): {e}"));
            G::Undefined
        }
    }
}

/// Call a function by path from the root ("root.foo", "_root.menu.show").
fn call_path(vm: &mut Vm, m: ObjId, path: &str, args: Vec<G>) -> G {
    let (head, method) = path.rsplit_once('.').unwrap_or(("root", path));
    let target = with_movie(vm, m, |mv| mv.path(head));
    match target {
        Ok(G::Object(h)) => match with_movie(vm, m, |mv| mv.invoke(&h, method, &args)) {
            Ok(v) => v,
            Err(e) => {
                vm.missing(format!("GFx call {path}: {e}"));
                G::Undefined
            }
        },
        _ => {
            vm.missing(format!("GFx call {path}: no object {head}"));
            G::Undefined
        }
    }
}

fn movie_as_call(vm: &mut Vm, c: &mut Call, ret: &Ty) -> Value {
    let Some(m) = c.this.and_then(|o| movie_of(vm, o)) else { return Value::Int(0) };
    let args = caller_args(vm, m);
    let v = call_path(vm, m, &c.str(0), args);
    from_as(vm, Some(m), v, ret, None)
}

fn obj_as_call(vm: &mut Vm, c: &mut Call, ret: &Ty) -> Value {
    let Some(o) = c.this else { return Value::Int(0) };
    let Some(m) = movie_of(vm, o) else { return Value::Int(0) };
    let args = caller_args(vm, m);
    let v = invoke(vm, o, &c.str(0), args);
    from_as(vm, Some(o), v, ret, None)
}

fn array_objects(vm: &mut Vm, m: ObjId, v: G) -> Value {
    let G::Object(arr) = v else { return Value::Array(vec![]) };
    let n = with_movie(vm, m, |mv| mv.get(&arr, "length")).map(|v| as_number(&v) as usize).unwrap_or(0);
    let mut out = vec![];
    for i in 0..n.min(10_000) {
        if let Ok(G::Object(h)) = with_movie(vm, m, |mv| mv.get(&arr, &i.to_string())) {
            out.push(Value::Obj(Some(wrap(vm, m, h, None))));
        } else {
            out.push(Value::Obj(None));
        }
    }
    Value::Array(out)
}

fn goto(vm: &mut Vm, c: &mut Call, play: bool) -> Value {
    let Some(o) = c.this else { return Value::Int(0) };
    let frame = match c.arg(0) {
        Value::Str(s) => G::String(s.clone()),
        v => G::Number(v.int() as f64),
    };
    invoke(vm, o, if play { "gotoAndPlay" } else { "gotoAndStop" }, vec![frame]);
    Value::Int(0)
}

fn get_variable_array(vm: &mut Vm, c: &mut Call) -> Value {
    let Some(m) = c.this.and_then(|o| movie_of(vm, o)) else { return Value::Bool(false) };
    let G::Object(arr) = var_path(vm, Some(m), &c.str(0)) else { return Value::Bool(false) };
    let start = c.int(1).max(0) as usize;
    let n = with_movie(vm, m, |mv| mv.get(&arr, "length")).map(|v| as_number(&v) as usize).unwrap_or(0);
    let elem_ty = match vm.funcs[c.func].params.get(2).map(|p| p.ty.clone()) {
        Some(Ty::Array(t)) => *t,
        _ => Ty::Unknown,
    };
    let mut out = vec![];
    for i in start..n.min(start + 10_000) {
        let v = with_movie(vm, m, |mv| mv.get(&arr, &i.to_string())).unwrap_or(G::Undefined);
        out.push(from_as(vm, Some(m), v, &elem_ty, None));
    }
    c.args[2] = Value::Array(out);
    Value::Bool(true)
}

fn set_variable_array(vm: &mut Vm, c: &mut Call) -> Value {
    let Some(m) = c.this.and_then(|o| movie_of(vm, o)) else { return Value::Bool(false) };
    let G::Object(arr) = var_path(vm, Some(m), &c.str(0)) else { return Value::Bool(false) };
    let start = c.int(1).max(0) as usize;
    let Value::Array(items) = c.arg(2).clone() else { return Value::Bool(false) };
    for (k, it) in items.iter().enumerate() {
        let g = to_as(vm, m, it, None);
        let _ = with_movie(vm, m, |mv| mv.set(&arr, &(start + k).to_string(), g));
    }
    Value::Bool(true)
}

fn display_info(vm: &mut Vm, c: &mut Call, o: Option<ObjId>) -> Value {
    let Some(r) = vm.funcs[c.func].ret.clone() else { return Value::Int(0) };
    let mut s = match vm.zero(&r.ty, r.dim) {
        Value::Struct(s) => *s,
        other => return other,
    };
    let props = [("X", "x", 1.0), ("Y", "y", 1.0), ("Z", "z", 1.0), ("Rotation", "rotation", 1.0), ("XScale", "scaleX", 100.0), ("YScale", "scaleY", 100.0), ("Alpha", "alpha", 100.0)];
    for (field, member_name, k) in props {
        let v = as_number(&member(vm, o, member_name)) * k;
        let y = vm.sym(field);
        *s.field_mut(y) = Value::Float(v as f32);
        let hy = vm.sym(&format!("has{field}"));
        *s.field_mut(hy) = Value::Bool(true);
    }
    let vis = matches!(member(vm, o, "visible"), G::Bool(true));
    let y = vm.sym("Visible");
    *s.field_mut(y) = Value::Bool(vis);
    let y = vm.sym("hasVisible");
    *s.field_mut(y) = Value::Bool(true);
    Value::Struct(Box::new(s))
}

fn set_display_info(vm: &mut Vm, o: Option<ObjId>, d: &Value) {
    let Value::Struct(s) = d else { return };
    let get = |vm: &Vm, n: &str| s.fields.iter().find(|f| vm.name_of(f.0).eq_ignore_ascii_case(n)).map(|f| f.1.clone());
    let props = [("X", "x", 1.0), ("Y", "y", 1.0), ("Z", "z", 1.0), ("Rotation", "rotation", 1.0), ("XRotation", "rotationX", 1.0), ("YRotation", "rotationY", 1.0), ("XScale", "scaleX", 0.01), ("YScale", "scaleY", 0.01), ("ZScale", "scaleZ", 0.01), ("Alpha", "alpha", 0.01)];
    for (field, member_name, k) in props {
        if get(vm, &format!("has{field}")).is_some_and(|v| v.truthy()) {
            let v = get(vm, field).map(|v| v.float()).unwrap_or(0.0) * k;
            set_member(vm, o, member_name, &Value::Float(v));
        }
    }
    if get(vm, "hasVisible").is_some_and(|v| v.truthy()) {
        let v = get(vm, "Visible").is_some_and(|v| v.truthy());
        set_member(vm, o, "visible", &Value::Bool(v));
    }
}

// --- callbacks from ActionScript ------------------------------------------------------------------

/// Deliver the calls ActionScript made into the host (listeners, init callbacks, ExternalInterface).
pub fn pump(vm: &mut Vm, m: ObjId) {
    for _ in 0..16 {
        let calls = gfxui::take_calls();
        if calls.is_empty() {
            return;
        }
        for c in calls {
            dispatch(vm, m, c);
        }
    }
}

fn dispatch(vm: &mut Vm, current: ObjId, c: HostCall) {
    match c {
        HostCall::Function { id, args } => match vm.gfx.fns.get(&id) {
            Some(HostFn::WidgetInit { movie }) => {
                let movie = *movie;
                if let Some(s) = vm.gfx.movies.get_mut(&movie) {
                    s.widgets_initialized = true;
                    s.post_init = true;
                }
                // (name, path, widget): WidgetBindings pick the GFxObject class by widget name
                let name = match args.first() {
                    Some(G::String(s)) => s.clone(),
                    _ => String::new(),
                };
                let path = match args.get(1) {
                    Some(G::String(s)) => s.clone(),
                    _ => String::new(),
                };
                if vm.gfx.log_calls {
                    vm.say(format!("GFx WidgetInitialized {name} ({path})"));
                }
                if std::env::var_os("GOAT_TRACE_HUD").is_some() {
                    println!("    GFx WidgetInitialized {name} ({path})");
                }
                let class = widget_class(vm, movie, &name);
                let Some(G::Object(h)) = args.get(2).cloned() else { return };
                let w = wrap(vm, movie, h, class);
                let (ny, py) = (vm.sym(&name), vm.sym(&path));
                vm.call_named(movie, "WidgetInitialized", vec![Value::Name(ny), Value::Name(py), Value::Obj(Some(w))]);
            }
            Some(HostFn::Delegate { movie, target, func }) => {
                let (movie, target, func) = (*movie, *target, *func);
                call_script(vm, movie, target, func, args);
            }
            None if id >= 1_000_000 => probe_call(vm, current, &args),
            None => {}
        },
        HostCall::External { name, args } => {
            // UE3: the ExternalInterface object, else the movie player itself
            let ei = vm.get(current, "ExternalInterface").obj().unwrap_or(current);
            let f = vm.sym(&name);
            call_script(vm, current, ei, f, args);
        }
    }
}

fn widget_class(vm: &mut Vm, movie: ObjId, name: &str) -> Option<usize> {
    let Value::Array(bindings) = vm.get(movie, "WidgetBindings") else { return None };
    let wn = vm.sym("WidgetName");
    let wc = vm.sym("WidgetClass");
    for b in bindings {
        let Value::Struct(s) = b else { continue };
        if let Some(Value::Name(n)) = s.field(wn) {
            if vm.name_of(*n).eq_ignore_ascii_case(name) {
                return s.field(wc).and_then(|c| vm.as_class(c));
            }
        }
    }
    None
}

/// Call `func` on `target` with AS arguments converted to its parameter types.
fn call_script(vm: &mut Vm, movie: ObjId, target: ObjId, func: Sym, args: Vec<G>) {
    if vm.obj(target).destroyed {
        return;
    }
    let class = vm.obj(target).class;
    let state = vm.obj(target).state;
    let Some(f) = vm.find_func(class, func, state) else {
        let n = format!("{}.{}", vm.class_name(class), vm.name_of(func));
        vm.missing(format!("GFx callback {n} not found"));
        return;
    };
    let params = vm.funcs[f].params.clone();
    if vm.gfx.log_calls {
        let d = vm.funcs[f].display.clone();
        let ev = match (args.first(), vm.gfx.movies.get(&movie)) {
            (Some(G::Object(h)), Some(s)) => format!("{} type {:?}", s.movie.describe_value(&G::Object(h.clone())), s.movie.get(h, "type").ok()),
            (a, _) => format!("{a:?}"),
        };
        vm.say(format!("GFx -> {d} on {} ({ev})", vm.obj(target).name));
    }
    let mut vals = vec![];
    for (i, p) in params.iter().enumerate() {
        let a = args.get(i).cloned().unwrap_or(G::Undefined);
        vals.push(from_as(vm, Some(movie), a, &p.ty, None));
    }
    let top = vm.frames.is_empty();
    if top {
        vm.budget = exec::BUDGET;
        vm.aborted = false;
    }
    let passed = vec![true; vals.len()];
    vm.invoke(Some(target), f, vals, passed);
}

/// Call a delegate property ("OnPostAdvance": `__OnPostAdvance__Delegate`) if it is bound.
pub fn call_delegate_prop(vm: &mut Vm, o: ObjId, name: &str, args: Vec<Value>) {
    for key in [format!("__{name}__Delegate"), name.to_string()] {
        if let Value::Delegate(t, f) = vm.get(o, &key) {
            if f.0 != 0 {
                let target = t.unwrap_or(o);
                let class = vm.obj(target).class;
                let state = vm.obj(target).state;
                if let Some(func) = vm.find_func(class, f, state) {
                    let passed = vec![true; args.len()];
                    vm.invoke(Some(target), func, args, passed);
                }
                return;
            }
        }
    }
}

// --- img:// ------------------------------------------------------------------------------------------

/// `img://Package.Texture`: decode the texture to `<cache>/img/<package.texture>` (lower case),
/// where the movie's navigator serves it from.
fn ensure_image(vm: &mut Vm, s: &str) {
    let Some(path) = s.strip_prefix("img://") else { return };
    let dir = vm.gfx.dir.get_or_insert_with(cache_dir).join("img");
    let file = dir.join(path.to_lowercase());
    if file.exists() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    match decode_texture(vm, path) {
        Some(png) => {
            let _ = write_cached(&file, &png);
        }
        None => vm.missing(format!("GFx image {s}: texture not found")),
    }
}

fn decode_texture(vm: &Vm, path: &str) -> Option<Vec<u8>> {
    let r = vm.reg.find_path(path)?;
    let p = vm.reg.pkg(r);
    let t = upk::texture::Texture::parse(p, r.export).ok()?;
    let img = t.decode(p, r.export, 2048).ok()?;
    let rgba = image::RgbaImage::from_raw(img.width, img.height, img.rgba)?;
    let mut out = std::io::Cursor::new(vec![]);
    rgba.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

// --- host side: frames, input, pictures ----------------------------------------------------------

/// Every open movie: advance, PostWidgetInit, PostAdvance (OnPostAdvance delegate).
pub fn tick(vm: &mut Vm, dt: f32) {
    for m in vm.gfx.order.clone() {
        if !vm.gfx.order.contains(&m) {
            continue;
        }
        advance(vm, m, dt);
        if vm.gfx.order.contains(&m) {
            vm.call_named(m, "PostAdvance", vec![Value::Float(dt)]);
        }
    }
}

/// Render the open movies (back to front).
pub fn capture(vm: &mut Vm) -> Vec<(ObjId, image::RgbaImage)> {
    let mut out = vec![];
    for m in vm.gfx.order.clone() {
        if let Some(s) = vm.gfx.movies.get(&m) {
            if let Some(img) = s.movie.capture() {
                out.push((m, img));
            }
        }
    }
    out
}

/// Render one movie, if `force` or something in it changed since its last capture. New movies
/// render every frame for a while: CLIK lays widgets out on Flash's RENDER event, so a movie that
/// is never rendered keeps unsized widgets (the main menu's news text field covered its buttons
/// and took their clicks).
pub fn capture_if_changed(vm: &mut Vm, m: ObjId, force: bool) -> Option<image::RgbaImage> {
    let s = vm.gfx.movies.get(&m)?;
    (force || s.frames < 120 || s.movie.needs_render()).then(|| s.movie.capture()).flatten()
}

pub fn set_size(vm: &mut Vm, w: u32, h: u32) {
    vm.gfx.size = (w, h);
    for s in vm.gfx.movies.values_mut() {
        s.movie.set_size(w, h);
    }
}

/// A movie that takes input (menus). The in-game HUD (bCaptureInput False) is only drawn.
pub fn captures_input(vm: &Vm, m: ObjId) -> bool {
    let Some(y) = vm.syms_get("bCaptureInput") else { return false };
    vm.obj(m).props.get(&y).or_else(|| vm.default_of(m, y)).is_some_and(|v| v.truthy())
}

/// A menu (an input-taking movie) is open.
pub fn menu_open(vm: &Vm) -> bool {
    vm.gfx.order.iter().any(|&m| captures_input(vm, m))
}

/// The movie that gets input (the newest open one that captures input).
pub fn top(vm: &Vm) -> Option<ObjId> {
    vm.gfx.order.iter().rev().copied().find(|&m| captures_input(vm, m))
}

pub fn mouse_move(vm: &mut Vm, x: f32, y: f32) {
    let Some(m) = top(vm) else { return };
    let _ = with_movie(vm, m, |mv| {
        mv.mouse_move(x as f64, y as f64);
        Ok(())
    });
}

pub fn mouse_button(vm: &mut Vm, x: f32, y: f32, down: bool) {
    let Some(m) = top(vm) else { return };
    let _ = with_movie(vm, m, |mv| {
        mv.mouse_move(x as f64, y as f64);
        mv.mouse_button(x as f64, y as f64, down);
        Ok(())
    });
}

pub fn mouse_wheel(vm: &mut Vm, lines: f32) {
    let Some(m) = top(vm) else { return };
    let _ = with_movie(vm, m, |mv| {
        mv.mouse_wheel(lines as f64);
        Ok(())
    });
}

/// A key (Unreal key name) for the top movie: its FilterButtonInput first (Escape -> back),
/// then the movie itself (Flash key code) unless the script trapped it.
pub fn key(vm: &mut Vm, ue_key: &str, down: bool) {
    let Some(m) = top(vm) else { return };
    let y = vm.sym(ue_key);
    let ev = if down { IE_PRESSED } else { IE_RELEASED };
    let trapped = vm.call_named(m, "FilterButtonInput", vec![Value::Int(0), Value::Name(y), Value::Byte(ev)]).is_some_and(|v| v.truthy());
    if trapped || !vm.gfx.movies.contains_key(&m) {
        return;
    }
    let code = flash_key_code(ue_key);
    if code != 0 {
        let _ = with_movie(vm, m, |mv| {
            mv.key(code, down);
            Ok(())
        });
    }
}

fn flash_key_code(k: &str) -> u32 {
    match k {
        "Enter" => 13,
        "Escape" => 27,
        "Tab" => 9,
        "SpaceBar" => 32,
        "Left" => 37,
        "Up" => 38,
        "Right" => 39,
        "Down" => 40,
        "BackSpace" => 8,
        _ => 0,
    }
}

/// Global position of a GFxObject (for tests: where to click), in viewport pixels.
pub fn screen_rect(vm: &mut Vm, o: ObjId) -> Option<[f32; 4]> {
    let (m, h) = vm.gfx.objs.get(&o).cloned()?;
    let (vw, vh) = vm.gfx.size;
    let r = with_movie(vm, m, |mv| {
        let b = mv.invoke(&h, "getBounds", &[G::Object(mv.root()?)])?;
        let G::Object(b) = b else { return Err("no bounds".into()) };
        let f = |n: &str| mv.get(&b, n).map(|v| as_number(&v));
        Ok([f("x")?, f("y")?, f("width")?, f("height")?])
    })
    .ok()?;
    // the 1920x1080 stage is shown whole, centred (ShowAll)
    let s = (vw as f64 / 1920.0).min(vh as f64 / 1080.0);
    let (ox, oy) = ((vw as f64 - 1920.0 * s) / 2.0, (vh as f64 - 1080.0 * s) / 2.0);
    Some([(ox + r[0] * s) as f32, (oy + r[1] * s) as f32, (r[2] * s) as f32, (r[3] * s) as f32])
}

/// Debug (UVM_GFX_PROBE): print the targets of the stage's mouse events into the log.
pub fn probe_mouse_events(vm: &mut Vm) {
    let Some(m) = top(vm) else { return };
    let r = with_movie(vm, m, |mv| {
        let root = mv.root()?;
        let stage = match mv.get(&root, "stage")? {
            G::Object(s) => s,
            _ => return Err("no stage".into()),
        };
        for (i, ev) in ["mouseDown", "mouseUp", "rollOut", "rollOver", "click", "mouseOver", "mouseOut"].iter().enumerate() {
            let f = mv.host_function(1_000_000 + i as u64)?;
            mv.invoke(&stage, "addEventListener", &[G::String(ev.to_string()), G::Object(f), G::Bool(true), G::Number(0.0), G::Bool(false)])?;
        }
        Ok(())
    });
    vm.say(format!("GFx probe installed: {r:?}"));
}

/// Debug: log a probe call (target of a mouse event).
fn probe_call(vm: &mut Vm, m: ObjId, args: &[G]) {
    let Some(s) = vm.gfx.movies.get(&m) else { return };
    let Some(G::Object(e)) = args.first() else { return };
    let ty = s.movie.get(e, "type").ok();
    let t = s.movie.get(e, "target").ok().map(|t| s.movie.describe_value(&t));
    let ct = s.movie.get(e, "currentTarget").ok().map(|t| s.movie.describe_value(&t));
    let line = format!("GFx probe {ty:?} target {t:?} current {ct:?}");
    vm.say(line);
}

/// Debug (`goat --uvm-menu` with UVM_GFX_IMG=Package.Texture): the HUD movie's CraftingHelp
/// widget loads `img://<path>` through its own SetTexture (a flash.display.Loader), as
/// GGUICraftingHelp does; returns the loaded image's size.
pub fn probe_image(vm: &mut Vm, path: &str) -> Result<(f64, f64), String> {
    let class = vm.class_by_name("GFxMoviePlayer").ok_or("no GFxMoviePlayer")?;
    let m = vm.spawn_object(class, Some("ImageProbe".into()));
    let r = vm.reg.find_path("UI_InGame.UI_HUD").ok_or("no UI_InGame.UI_HUD")?;
    let info = vm.obj_for(r);
    vm.set(m, "MovieInfo", Value::Obj(info));
    if !start(vm, m) {
        return Err("HUD movie did not start".into());
    }
    let url = format!("img://{path}");
    ensure_image(vm, &url);
    let help = with_movie(vm, m, |mv| {
        let root = mv.root()?;
        let help = mv.attach(&root, "CraftingHelp", "probe", -1)?;
        mv.invoke(&help, "SetTexture", &[G::String(url.clone())])?;
        Ok(help)
    })?;
    for _ in 0..10 {
        advance(vm, m, 1.0 / 30.0);
    }
    let r = with_movie(vm, m, |mv| {
        let G::Object(loader) = mv.get(&help, "mLoader")? else { return Err("no mLoader".into()) };
        let G::Object(info) = mv.get(&loader, "contentLoaderInfo")? else { return Err("no contentLoaderInfo".into()) };
        let w = as_number(&mv.get(&info, "width")?);
        let h = as_number(&mv.get(&info, "height")?);
        Ok((w, h))
    });
    close(vm, m, true);
    r
}


/// GGControllerTag.Translate for the keyboard: `<font:ControllerFont><Controller:GBA_X/>...</font>`
/// (the glyph font plus input commands) becomes the keys bound to each command
/// (PlayerInput.Bindings: Name -> Command), e.g. "<Controller:GBA_ToggleInventory/>" -> "I".
pub fn controller_tags(vm: &mut Vm, s: &str) -> String {
    if std::env::var_os("GOAT_NO_TAGS").is_some() {
        return s.to_string();
    }
    let pi_class = vm.class_by_name("PlayerInput");
    let pi = pi_class.and_then(|pc| (0..vm.objects.len() as ObjId).find(|&o| !vm.obj(o).name.starts_with("Default__") && matches!(vm.obj(o).kind, Kind::Instance) && vm.is_child_of(vm.obj(o).class, pc)));
    let (ns, cs) = (vm.sym("Name"), vm.sym("Command"));
    let binds: Vec<(String, String)> = match pi.map(|p| vm.get(p, "Bindings")) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|b| match b {
                Value::Struct(st) => {
                    let n = match st.field(ns)? {
                        Value::Name(n) => vm.name_of(*n).to_string(),
                        _ => return None,
                    };
                    let c = match st.field(cs)? {
                        Value::Str(c) => c.clone(),
                        _ => return None,
                    };
                    Some((n, c))
                }
                _ => None,
            })
            .collect(),
        _ => vec![],
    };
    let key_of = |cmd: &str| -> String {
        // a binding may run several commands ("GBA_Back | GBA_VehicleBrake")
        let k = binds
            .iter()
            .find(|(n, c)| c.split('|').any(|p| p.trim().eq_ignore_ascii_case(cmd)) && !n.starts_with("Xbox") && !n.starts_with("GBA_"))
            .map(|(n, _)| n.as_str())
            .unwrap_or("?");
        match k {
            "one" => "1".into(),
            "two" => "2".into(),
            "SpaceBar" => "Space".into(),
            "LeftShift" => "Shift".into(),
            "LeftMouseButton" => "LMB".into(),
            "RightMouseButton" => "RMB".into(),
            "MouseScrollUp" => "Wheel Up".into(),
            "MouseScrollDown" => "Wheel Down".into(),
            k => k.to_string(),
        }
    };
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<Controller:") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + "<Controller:".len()..];
        let Some(j) = tail.find("/>") else { break };
        if !out.ends_with(' ') && out.ends_with('>') && !out.ends_with("ControllerFont>") {
            out.push(' ');
        }
        out.push_str(&key_of(tail[..j].trim()));
        rest = &tail[j + 2..];
        if rest.starts_with("<Controller:") {
            out.push(' ');
        }
    }
    out.push_str(rest);
    // the glyph font isn't there: the keys are plain text
    while let Some(i) = out.find("<font:ControllerFont>") {
        out.replace_range(i..i + "<font:ControllerFont>".len(), "");
        if let Some(j) = out[i..].find("</font>") {
            out.replace_range(i + j..i + j + "</font>".len(), "");
        }
    }
    out
}
