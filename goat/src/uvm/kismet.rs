//! Kismet: the map's sequence objects (SeqEvent/SeqAct/SeqCond/SeqVar) as VM objects, run like
//! UE3's USequence. Events (LevelLoaded, Touch/UnTouch from the host) send impulses down their
//! output links; Engine actions and conditions are implemented here, GoatGame/Workshop actions
//! run their own `Activated` script. Variables are read into the op's properties through its
//! VariableLinks before it runs and written back after.

use super::bridge::{Host, HostEvent};
use super::*;
use std::collections::VecDeque;

#[derive(Default)]
pub struct Kismet {
    pub ops: Vec<ObjId>,
    pub(super) queue: VecDeque<(ObjId, usize)>,
    /// (seconds left, op, input) for ActivateDelay / SeqAct_Delay.
    delayed: Vec<(f32, ObjId, usize)>,
    /// SeqEvent_Touch-like events: (event, originator actor).
    pub touch: Vec<(ObjId, ObjId)>,
    pub activations: u64,
    /// Playing SeqAct_Interp matinees (matinee.rs).
    pub matinee: super::matinee::Matinees,
}

pub(super) fn field<'a>(vm: &Vm, s: &'a StructVal, name: &str) -> Option<&'a Value> {
    s.field(vm.syms_get(name)?)
}

pub(super) fn struct_list(v: &Value) -> Vec<StructVal> {
    match v {
        Value::Array(a) => a.iter().filter_map(|x| if let Value::Struct(s) = x { Some((**s).clone()) } else { None }).collect(),
        _ => vec![],
    }
}

/// "<font color='#C800C8'>MICHAEL BAY!" / "[TRYHARD1]" -> plain text.
pub fn plain_trick_name(s: &str) -> String {
    let mut out = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            '[' | ']' if !tag => {}
            c if !tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

impl Host {
    fn is(&mut self, o: ObjId, class: &str) -> bool {
        match self.vm.class_by_name(class) {
            Some(c) => self.vm.is_child_of(self.vm.obj(o).class, c),
            None => false,
        }
    }

    /// A map actor by its export index in the map package.
    pub fn map_actor(&mut self, export: usize) -> Option<ObjId> {
        let mp = self.vm.map_pkg?;
        self.vm.obj_for_ref(mp, export as i32 + 1)
    }

    /// Fire the events of `class` whose Originator is `actor` (Actor.TriggerEventClass).
    pub fn kismet_actor_event(&mut self, actor: ObjId, class: &str) {
        let ops = self.kismet.ops.clone();
        let goat = self.goat;
        let mut fired = false;
        for o in ops {
            if self.is(o, class) && self.vm.get(o, "Originator").obj() == Some(actor) {
                fired |= self.trigger_event(o, 0, goat);
            }
        }
        if fired {
            self.kismet_run();
        }
    }

    /// Load every sequence object of the map and fire the level-loaded events.
    pub fn kismet_start(&mut self) {
        let Some(mp) = self.vm.map_pkg else { return };
        let Some(seq_obj) = self.vm.class_by_name("SequenceObject") else { return };
        let p = &self.vm.reg.packages[mp];
        let refs: Vec<i32> = (0..p.exports.len()).map(|i| i as i32 + 1).filter(|&i| p.obj_path(i).contains(".PersistentLevel.") && !p.obj_name(i).starts_with("Default__")).collect();
        let mut ops = vec![];
        for idx in refs {
            let cn = self.vm.reg.packages[mp].class_name(idx);
            if !(cn.starts_with("Seq") || cn.starts_with("GGSeq") || cn == "Sequence" || cn.contains("Seq")) {
                continue;
            }
            let Some(o) = self.vm.obj_for_ref(mp, idx) else { continue };
            if matches!(self.vm.obj(o).kind, Kind::Instance) && self.vm.is_child_of(self.vm.obj(o).class, seq_obj) {
                ops.push(o);
            }
        }
        let mut loaded = vec![];
        let mut touch = vec![];
        for &o in &ops {
            if self.is(o, "SeqEvent_LevelLoaded") || self.is(o, "GGSeqEvent_StartIntroMovie") {
                loaded.push(o);
            } else if self.is(o, "SeqEvent_Touch") {
                if let Some(orig) = self.vm.get(o, "Originator").obj() {
                    touch.push((o, orig));
                }
            }
        }
        // UVM_KISMET_DUMP=<object name>: print its properties (debugging link decoding)
        if let Ok(want) = std::env::var("UVM_KISMET_DUMP") {
            for &o in &ops {
                if self.vm.obj(o).name.eq_ignore_ascii_case(&want) {
                    let keys: Vec<Sym> = self.vm.obj(o).props.keys().copied().collect();
                    for k in keys {
                        let v = self.vm.obj(o).props[&k].clone();
                        println!("  {} = {}", self.vm.name_of(k), self.vm.describe(&v));
                    }
                }
            }
        }
        let n = ops.len();
        self.vm.seq_objects = ops.clone();
        self.kismet.ops = ops;
        self.kismet.touch = touch;
        self.vm.say(format!("kismet: {n} sequence objects, {} touch events, {} level-loaded events", self.kismet.touch.len(), loaded.len()));
        let goat = self.goat;
        for e in loaded {
            self.trigger_event(e, 0, goat);
        }
        self.kismet_run();
    }

    /// SequenceEvent.CheckActivate: enabled, trigger count, retrigger delay; then the output.
    pub fn trigger_event(&mut self, ev: ObjId, output: usize, instigator: Option<ObjId>) -> bool {
        let enabled = match self.vm.get(ev, "bEnabled") {
            Value::Bool(b) => b,
            _ => true,
        };
        let max = self.vm.get(ev, "MaxTriggerCount").int();
        let count = self.vm.get(ev, "TriggerCount").int();
        if !enabled || (max > 0 && count >= max) {
            return false;
        }
        self.vm.set(ev, "TriggerCount", Value::Int(count + 1));
        // the Instigator variable link
        let links = struct_list(&self.vm.get(ev, "VariableLinks"));
        for l in links {
            let desc = field(&self.vm, &l, "LinkDesc").map(|v| v.str().to_string()).unwrap_or_default();
            if desc.eq_ignore_ascii_case("Instigator") {
                if let Some(Value::Array(vars)) = field(&self.vm, &l, "LinkedVariables") {
                    for v in vars.iter().filter_map(|x| x.obj()) {
                        self.vm.set(v, "ObjValue", Value::Obj(instigator));
                    }
                }
            }
        }
        let name = self.vm.obj(ev).name.clone();
        self.vm.say(format!("kismet: {name} fired (output {output})"));
        self.fire_output(ev, output);
        true
    }

    pub(super) fn fire_output(&mut self, op: ObjId, output: usize) {
        let outs = struct_list(&self.vm.get(op, "OutputLinks"));
        let Some(out) = outs.get(output) else { return };
        if field(&self.vm, out, "bDisabled").is_some_and(|v| v.truthy()) {
            return;
        }
        let delay = field(&self.vm, out, "ActivateDelay").map(|v| v.float()).unwrap_or(0.0);
        let links = field(&self.vm, out, "Links").cloned().unwrap_or(Value::Array(vec![]));
        for l in struct_list(&links) {
            let Some(target) = field(&self.vm, &l, "LinkedOp").and_then(|v| v.obj()) else { continue };
            let input = field(&self.vm, &l, "InputLinkIdx").map(|v| v.int().max(0) as usize).unwrap_or(0);
            if delay > 0.0 {
                self.kismet.delayed.push((delay, target, input));
            } else {
                self.kismet.queue.push_back((target, input));
            }
        }
    }

    /// Values of the variables linked to a variable link (SeqVar_* -> Value).
    fn linked_values(&mut self, l: &StructVal) -> Vec<(ObjId, Value)> {
        let vars = match field(&self.vm, l, "LinkedVariables") {
            Some(Value::Array(a)) => a.iter().filter_map(|x| x.obj()).collect::<Vec<_>>(),
            _ => vec![],
        };
        let mut out = vec![];
        for v in vars {
            let val = if self.is(v, "SeqVar_Player") {
                Value::Obj(self.goat)
            } else if self.is(v, "SeqVar_Object") {
                self.vm.get(v, "ObjValue")
            } else if self.is(v, "SeqVar_Int") {
                self.vm.get(v, "IntValue")
            } else if self.is(v, "SeqVar_Bool") {
                Value::Bool(self.vm.get(v, "bValue").truthy())
            } else if self.is(v, "SeqVar_Float") {
                self.vm.get(v, "FloatValue")
            } else if self.is(v, "SeqVar_String") {
                self.vm.get(v, "StrValue")
            } else if self.is(v, "SeqVar_Vector") {
                self.vm.get(v, "VectValue")
            } else {
                continue;
            };
            out.push((v, val));
        }
        out
    }

    /// PublishLinkedVariableValues: linked variables into the op's properties.
    fn publish_vars(&mut self, op: ObjId) {
        let links = struct_list(&self.vm.get(op, "VariableLinks"));
        for l in links {
            let prop = match field(&self.vm, &l, "PropertyName") {
                Some(Value::Name(y)) if !self.vm.name_of(*y).eq_ignore_ascii_case("None") => *y,
                _ => continue,
            };
            let vals: Vec<Value> = self.linked_values(&l).into_iter().map(|(_, v)| v).collect();
            let class = self.vm.obj(op).class;
            let is_array = matches!(self.vm.find_prop(class, prop).map(|p| p.ty.clone()), Some(Ty::Array(_)));
            if is_array {
                self.vm.obj_mut(op).props.insert(prop, Value::Array(vals));
            } else if let Some(v) = vals.into_iter().next() {
                self.vm.obj_mut(op).props.insert(prop, v);
            }
        }
    }

    /// PopulateLinkedVariableValues for writeable links (Set* actions write their Target).
    fn write_var(&mut self, op: ObjId, link_desc: &str, value: Value) {
        let links = struct_list(&self.vm.get(op, "VariableLinks"));
        for l in links {
            let desc = field(&self.vm, &l, "LinkDesc").map(|v| v.str().to_string()).unwrap_or_default();
            if !desc.eq_ignore_ascii_case(link_desc) {
                continue;
            }
            for (v, _) in self.linked_values(&l) {
                let key = if self.is(v, "SeqVar_Int") {
                    "IntValue"
                } else if self.is(v, "SeqVar_Bool") {
                    "bValue"
                } else if self.is(v, "SeqVar_Float") {
                    "FloatValue"
                } else if self.is(v, "SeqVar_Object") {
                    "ObjValue"
                } else if self.is(v, "SeqVar_String") {
                    "StrValue"
                } else {
                    continue;
                };
                self.vm.set(v, key, value.clone());
            }
        }
    }

    fn var_value(&mut self, op: ObjId, link_desc: &str) -> Option<Value> {
        let links = struct_list(&self.vm.get(op, "VariableLinks"));
        for l in links {
            let desc = field(&self.vm, &l, "LinkDesc").map(|v| v.str().to_string()).unwrap_or_default();
            if desc.eq_ignore_ascii_case(link_desc) {
                return self.linked_values(&l).into_iter().map(|(_, v)| v).next();
            }
        }
        None
    }

    /// Run queued activations (bounded per frame).
    pub fn kismet_run(&mut self) {
        let mut budget = 2000;
        while let Some((op, input)) = self.kismet.queue.pop_front() {
            budget -= 1;
            if budget == 0 {
                self.vm.say("kismet: activation budget hit this frame".into());
                break;
            }
            if self.vm.obj(op).destroyed {
                continue;
            }
            self.kismet.activations += 1;
            self.activate(op, input);
        }
    }

    pub fn kismet_tick(&mut self, dt: f32) {
        // events script activated (GGGameInfo.NotifyKismetEventClass, GGGoat.NotifyKismet...)
        for (ev, inst) in std::mem::take(&mut self.vm.kismet_activations) {
            self.trigger_event(ev, 0, inst.or(self.goat));
        }
        let mut due = vec![];
        self.kismet.delayed.retain_mut(|(t, op, i)| {
            *t -= dt;
            if *t <= 0.0 {
                due.push((*op, *i));
                false
            } else {
                true
            }
        });
        self.kismet.queue.extend(due);
        self.kismet_run();
        if !self.kismet.matinee.list.is_empty() {
            self.matinee_tick(dt);
            self.kismet_run();
        }
    }

    /// One op receiving an impulse on `input`.
    fn activate(&mut self, op: ObjId, input: usize) {
        // InputLinks[input].bHasImpulse (scripts read it)
        let y = self.vm.sym("InputLinks");
        let hy = self.vm.sym("bHasImpulse");
        if let Some(Value::Array(a)) = self.vm.obj_mut(op).props.get_mut(&y) {
            for (i, l) in a.iter_mut().enumerate() {
                if let Value::Struct(s) = l {
                    *s.field_mut(hy) = Value::Bool(i == input);
                }
            }
        }
        self.publish_vars(op);
        let class = self.vm.class_name(self.vm.obj(op).class).to_string();
        let name = self.vm.obj(op).name.clone();
        if std::env::var_os("UVM_KISMET_TRACE").is_some() {
            println!("kismet: activate {name} ({class}) input {input}");
        }
        // conditions pick one output
        if self.is(op, "SeqCond_CompareInt") || self.is(op, "SeqCond_CompareFloat") {
            let a = self.var_value(op, "A").map(|v| v.float()).unwrap_or_else(|| self.vm.get(op, "ValueA").float());
            let b = self.var_value(op, "B").map(|v| v.float()).unwrap_or_else(|| self.vm.get(op, "ValueB").float());
            // A <= B, A > B, A == B, A < B, A >= B
            for (i, ok) in [a <= b, a > b, a == b, a < b, a >= b].into_iter().enumerate() {
                if ok {
                    self.fire_output(op, i);
                }
            }
            return;
        }
        if self.is(op, "SeqCond_CompareBool") {
            let v = self.var_value(op, "Bool").map(|v| v.truthy()).unwrap_or(false);
            self.fire_output(op, if v { 0 } else { 1 });
            return;
        }
        if self.is(op, "SeqCond_CompareObject") {
            let a = self.var_value(op, "A").and_then(|v| v.obj());
            let b = self.var_value(op, "B").and_then(|v| v.obj());
            self.fire_output(op, if a == b { 0 } else { 1 });
            return;
        }
        if self.is(op, "SeqCond_IsSameTeam") || self.is(op, "SeqCond_GetServerType") {
            self.fire_output(op, 0);
            return;
        }
        if self.is(op, "SeqAct_Gate") {
            // inputs: In, Open, Close, Toggle
            let open = match self.vm.get(op, "bOpen") {
                Value::Bool(b) => b,
                _ => true,
            };
            let open = match input {
                1 => true,
                2 => false,
                3 => !open,
                _ => {
                    if open {
                        self.fire_output(op, 0);
                    }
                    open
                }
            };
            self.vm.set(op, "bOpen", Value::Bool(open));
            return;
        }
        if self.is(op, "SeqAct_Delay") {
            let d = self.vm.get(op, "Duration").float().max(0.0);
            // the Finished output after Duration
            let outs = struct_list(&self.vm.get(op, "OutputLinks"));
            if !outs.is_empty() {
                let links = outs[0].fields.iter().find(|(k, _)| self.vm.name_of(*k).eq_ignore_ascii_case("Links")).map(|(_, v)| v.clone());
                for l in struct_list(&links.unwrap_or(Value::Array(vec![]))) {
                    if let (Some(t), i) = (field(&self.vm, &l, "LinkedOp").and_then(|v| v.obj()), field(&self.vm, &l, "InputLinkIdx").map(|v| v.int().max(0) as usize).unwrap_or(0)) {
                        self.kismet.delayed.push((d, t, i));
                    }
                }
            }
            return;
        }
        if self.is(op, "SeqAct_SetInt") || self.is(op, "SeqAct_SetBool") || self.is(op, "SeqAct_SetFloat") || self.is(op, "SeqAct_SetObject") || self.is(op, "SeqAct_SetString") {
            let v = self.var_value(op, "Value").unwrap_or_else(|| self.vm.get(op, "DefaultValue"));
            self.write_var(op, "Target", v);
            self.fire_output(op, 0);
            return;
        }
        if self.is(op, "SeqAct_AddInt") {
            let a = self.var_value(op, "A").map(|v| v.int()).unwrap_or(0);
            let b = self.var_value(op, "B").map(|v| v.int()).unwrap_or(0);
            self.write_var(op, "IntResult", Value::Int(a + b));
            self.fire_output(op, 0);
            return;
        }
        if self.is(op, "SeqAct_Toggle") {
            // Turn On / Turn Off / Toggle actors' bEnabled (triggers) and linked events
            let targets: Vec<ObjId> = match self.vm.get(op, "Targets") {
                Value::Array(a) => a.iter().filter_map(|v| v.obj()).collect(),
                _ => vec![],
            };
            for t in targets {
                let cur = match self.vm.get(t, "bEnabled") {
                    Value::Bool(b) => b,
                    _ => true,
                };
                let v = match input {
                    0 => true,
                    1 => false,
                    _ => !cur,
                };
                self.vm.set(t, "bEnabled", Value::Bool(v));
                // Emitter actors: activate / deactivate their particle system
                if self.vm.class_by_name("Emitter").is_some_and(|e| self.vm.is_child_of(self.vm.obj(t).class, e)) {
                    let on = match input {
                        0 => Some(true),
                        1 => Some(false),
                        _ => None,
                    };
                    let name = self.vm.obj(t).name.clone();
                    self.vm.events.push(HostEvent::Fx(super::fx::FxCmd::MapEmitter { name, on }));
                }
            }
            let evs = struct_list(&self.vm.get(op, "EventLinks"));
            for l in evs {
                if let Some(Value::Array(a)) = field(&self.vm, &l, "LinkedEvents") {
                    for e in a.iter().filter_map(|x| x.obj()) {
                        let cur = self.vm.get(e, "bEnabled").truthy();
                        let v = match input {
                            0 => true,
                            1 => false,
                            _ => !cur,
                        };
                        self.vm.set(e, "bEnabled", Value::Bool(v));
                    }
                }
            }
            self.fire_output(op, 0);
            return;
        }
        if self.is(op, "SeqAct_PlaySound") {
            let cue = self.vm.get(op, "PlaySound");
            let s = self.vm.describe(&cue);
            if cue.obj().is_some() {
                self.vm.events.push(HostEvent::PlaySound { cue: s });
            }
            self.fire_output(op, 0);
            return;
        }
        if self.is(op, "SeqAct_Interp") {
            // matinee: outputs fire when it ends (matinee.rs)
            let _ = &name;
            self.matinee_input(op, input);
            return;
        }
        if self.is(op, "SeqAct_SetVelocity") {
            // Targets' Velocity = normalised VelocityDir x VelocityMag (Velocity var overrides)
            let dir_v = self.vm.get(op, "VelocityDir");
            let mut dir = self.vm.vec3(&dir_v);
            let mag = self.var_value(op, "VelocityMag").map(|v| v.float()).unwrap_or_else(|| self.vm.get(op, "VelocityMag").float());
            let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            if len > 1e-6 {
                dir = dir.map(|x| x / len);
            }
            let targets: Vec<ObjId> = match self.vm.get(op, "Targets") {
                Value::Array(a) => a.iter().filter_map(|v| v.obj()).collect(),
                _ => vec![],
            };
            let rel = self.vm.get(op, "bVelocityRelativeToActorRotation").truthy();
            for t in targets {
                let mut d = dir;
                if rel {
                    let r = self.vm.get(t, "Rotation");
                    let yaw = self.vm.rot3(&r)[1] as f32 * std::f32::consts::TAU / 65536.0;
                    let (sy, cy) = yaw.sin_cos();
                    d = [d[0] * cy - d[1] * sy, d[0] * sy + d[1] * cy, d[2]];
                }
                let v = self.vm.vector(d[0] * mag, d[1] * mag, d[2] * mag);
                self.vm.set(t, "Velocity", v);
            }
            self.fire_output(op, 0);
            return;
        }
        if self.is(op, "SeqAct_SetCameraTarget") || self.is(op, "GFxAction_OpenMovie") {
            // camera cuts and Flash movies (intro) aren't shown
            self.fire_output(op, 0);
            return;
        }
        if self.is(op, "SeqAct_Log") {
            let s = self.vm.get(op, "LogMessage");
            let s = self.vm.to_string(&s);
            self.vm.say(format!("kismet log: {s}"));
            self.fire_output(op, 0);
            return;
        }
        if self.is(op, "SeqAct_FinishSequence") || self.is(op, "SequenceFrame") {
            return;
        }
        // script actions (GoatGame, Workshop maps): their Activated event
        if self.is(op, "SequenceOp") {
            let c = self.vm.obj(op).class;
            if self.vm.defines(c, "Activated").is_some() {
                self.vm.call_named(op, "Activated", vec![]);
            } else if !self.is(op, "SequenceVariable") && !self.is(op, "Sequence") {
                self.vm.missing(format!("kismet op {class}"));
            }
            if self.vm.defines(c, "DeActivated").is_some() {
                self.vm.call_named(op, "DeActivated", vec![]);
            }
            // bAutoActivateOutputLinks (SequenceAction default): every output fires
            let auto = match self.vm.get(op, "bAutoActivateOutputLinks") {
                Value::Bool(b) => b,
                _ => true,
            };
            let n = struct_list(&self.vm.get(op, "OutputLinks")).len();
            if auto {
                for i in 0..n {
                    self.fire_output(op, i);
                }
            }
        }
    }
}

impl Host {
    /// Whether an event's Touched output reaches a GGSeqAct_GiveScore within a few links
    /// without passing a condition.
    pub fn leads_to_score(&mut self, ev: ObjId) -> bool {
        let mut frontier = vec![(ev, Some(0usize))];
        for _ in 0..5 {
            let mut next = vec![];
            for (op, only) in frontier {
                if self.is(op, "GGSeqAct_GiveScore") {
                    return true;
                }
                // only unconditional paths (conditions depend on other progress)
                if self.is(op, "SequenceCondition") {
                    continue;
                }
                for (i, out) in struct_list(&self.vm.get(op, "OutputLinks")).into_iter().enumerate() {
                    if only.is_some_and(|o| o != i) {
                        continue;
                    }
                    let links = field(&self.vm, &out, "Links").cloned().unwrap_or(Value::Array(vec![]));
                    for l in struct_list(&links) {
                        if let Some(t) = field(&self.vm, &l, "LinkedOp").and_then(|v| v.obj()) {
                            next.push((t, None));
                        }
                    }
                }
            }
            frontier = next;
        }
        false
    }
}

impl Host {
    /// No per-frame script work: no mutators, timers, state code or pending Kismet.
    pub fn idle(&self) -> bool {
        self.mutators.is_empty() && self.vm.timers.is_empty() && self.vm.state_runs.is_empty() && self.kismet.delayed.is_empty() && self.kismet.queue.is_empty() && !self.map_ticks && !self.kismet.matinee.busy() && self.quests.is_none()
    }
}

impl Host {
    /// Actors placed in the map whose class the map itself defines (its own script classes, e.g.
    /// a Workshop map's arcade machines or ball): BeginPlay now, Tick every frame.
    pub fn map_scripts_start(&mut self) {
        let Some(mp) = self.vm.map_pkg else { return };
        let Some(actor) = self.vm.class_by_name("Actor") else { return };
        let p = &self.vm.reg.packages[mp];
        let idx: Vec<i32> = (0..p.exports.len())
            .filter(|&i| p.exports[i].class > 0 && p.class_name(p.exports[i].class) == "Class" && p.obj_path(i as i32 + 1).contains(".PersistentLevel."))
            .map(|i| i as i32 + 1)
            .collect();
        let mut found = vec![];
        for i in idx {
            let Some(o) = self.vm.obj_for_ref(mp, i) else { continue };
            if matches!(self.vm.obj(o).kind, Kind::Instance) && self.vm.is_child_of(self.vm.obj(o).class, actor) {
                found.push(o);
            }
        }
        let trace = self.vm.trace_calls;
        self.vm.trace_calls = false;
        for &o in &found {
            self.vm.call_named(o, "PreBeginPlay", vec![]);
            if !self.vm.obj(o).destroyed {
                self.vm.call_named(o, "PostBeginPlay", vec![]);
            }
        }
        self.vm.trace_calls = trace;
        let mut classes: Vec<String> = found.iter().map(|&o| self.vm.class_name(self.vm.obj(o).class).to_string()).collect();
        classes.sort();
        classes.dedup();
        self.vm.say(format!("map script actors: {} ({})", found.len(), classes.join(", ")));
        self.map_actors = found;
    }

    pub fn map_scripts_tick(&mut self, dt: f32) {
        for o in self.map_actors.clone() {
            if self.vm.obj(o).destroyed {
                continue;
            }
            let c = self.vm.obj(o).class;
            if self.vm.defines(c, "Tick").is_some() {
                self.vm.call_named(o, "Tick", vec![Value::Float(dt)]);
            }
        }
    }

    /// Whether any map script actor ticks (the VM can't idle then).
    pub fn map_scripts_tick_needed(&mut self) -> bool {
        self.map_actors.clone().into_iter().any(|o| {
            let c = self.vm.obj(o).class;
            self.vm.defines(c, "Tick").is_some()
        })
    }
}
