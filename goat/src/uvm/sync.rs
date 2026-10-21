//! Per-frame traffic between the Rust game and the script world: the goat's state into the
//! script goat (and what mutator script changed back out), key presses into GGPlayerInput's
//! listeners, and game events through GGGameInfo.OnX -> registered notification listeners.

use super::bridge::Host;
use super::*;

/// The Rust goat in Unreal units/axes.
#[derive(Debug, Clone, Default)]
pub struct GoatState {
    pub location: [f32; 3],
    pub velocity: [f32; 3],
    pub yaw: i32,
    /// Actor.EPhysics: 1 walking, 2 falling, 10 rigid body (ragdoll), 15 wall run.
    pub physics: u8,
    pub ragdoll: bool,
    pub sprinting: bool,
    /// The prop the goat licks: (body key, actor name, its in-game name, location) -> mGrabbedItem.
    pub grabbed: Option<(u64, String, String, [f32; 3])>,
}

/// What mutator script changed on the script goat since `sync_in`.
#[derive(Debug, Clone, Default)]
pub struct GoatChanges {
    pub velocity: Option<[f32; 3]>,
    pub location: Option<[f32; 3]>,
    pub ragdoll: Option<bool>,
    /// Actor.Physics set by script (EPhysics).
    pub physics: Option<u8>,
}

/// Game events the Rust side reports (GGGameInfo.OnX names).
#[derive(Debug, Clone)]
pub enum GameEvent {
    Landed,
    Airborne(f32),
    Ground,
    Ragdoll(bool),
    WallRunning(bool),
    WallJump,
    Baa,
    /// GGAbility class name (GGAbilityHorn, GGAbilityBite, GGAbilityKick).
    UseAbility(&'static str),
    /// GGTrickBase subclass name.
    TrickMade(&'static str),
    PlayerRespawn,
    /// GGGameInfo.OnManual(pawn, isDoingManual, wasSuccessful).
    Manual(bool, bool),
    /// A map actor (export) exploded / fractured (GGGameInfo.OnExplosion / OnFractured + Kismet).
    Exploded(usize, bool),
    Fractured(usize, bool),
}

impl Host {
    fn vec(&mut self, v: [f32; 3]) -> Value {
        self.vm.vector(v[0], v[1], v[2])
    }

    /// Write the Rust goat into the script goat (before mutator ticks).
    pub fn sync_in(&mut self, s: GoatState) {
        let Some(g) = self.goat else { return };
        let loc = self.vec(s.location);
        let vel = self.vec(s.velocity);
        let rot = self.vm.rotator(0, s.yaw, 0);
        self.vm.set(g, "Location", loc);
        self.vm.set(g, "Velocity", vel);
        self.vm.set(g, "Rotation", rot);
        self.vm.set(g, "Physics", Value::Byte(s.physics));
        self.vm.set(g, "mIsRagdoll", Value::Bool(s.ragdoll));
        self.vm.set(g, "mIsSprinting", Value::Bool(s.sprinting));
        let grabbed = s.grabbed.as_ref().map(|(body, name, shown, l)| {
            let o = self.vm.proxy_for(&super::WorldThing::Prop { body: *body, name: name.clone() }, *l);
            // GGKActorBase.GetActorName reads the physical material's name; the game knows it
            self.vm.set(o, "uvmActorName", Value::Str(shown.clone()));
            o
        });
        self.vm.set(g, "mGrabbedItem", Value::Obj(grabbed));
        self.written = Some(s);
    }

    /// Changes mutator script made to the script goat since `sync_in`.
    pub fn sync_out(&mut self) -> GoatChanges {
        let (Some(g), Some(w)) = (self.goat, self.written.clone()) else { return GoatChanges::default() };
        let mut out = GoatChanges::default();
        let vel = self.vm.get(g, "Velocity");
        let v = self.vm.vec3(&vel);
        if v.iter().zip(w.velocity).any(|(a, b)| (a - b).abs() > 0.01) {
            out.velocity = Some(v);
        }
        let loc = self.vm.get(g, "Location");
        let l = self.vm.vec3(&loc);
        if l.iter().zip(w.location).any(|(a, b)| (a - b).abs() > 0.01) {
            out.location = Some(l);
        }
        let r = self.vm.get(g, "mIsRagdoll").truthy();
        if r != w.ragdoll {
            out.ragdoll = Some(r);
        }
        let p = self.vm.get(g, "Physics").int() as u8;
        if p != w.physics {
            out.physics = Some(p);
        }
        out
    }

    /// Keys held this frame (Unreal key names): KS_Pressed every frame, KS_Down/KS_Up on
    /// change, like GGPlayerInput.PlayerInput, plus the deprecated KeyDown/KeyUp/KeyPressed lists.
    pub fn keys(&mut self, held: &[&str]) {
        let Some(pi) = self.player_input() else { return };
        let names: Vec<Sym> = held.iter().map(|k| self.vm.sym(k)).collect();
        let pressed = Value::Array(names.iter().map(|&n| Value::Name(n)).collect());
        self.vm.set(pi, "PressedKeys", pressed);
        let prev = std::mem::take(&mut self.prev_keys);
        self.downs = names.iter().copied().filter(|k| !prev.contains(k)).collect();
        let listeners = |h: &mut Host, list: &str| h.vm.get(pi, list);
        for &k in &names {
            let l = listeners(self, "mKeyPressed");
            self.vm.call_named(pi, "NotifyKeyListeners", vec![Value::Name(k), l]);
            self.vm.call_named(pi, "NotifyKeyStateListeners", vec![Value::Name(k), Value::Byte(0)]);
            if !prev.contains(&k) {
                let l = listeners(self, "mKeyDown");
                self.vm.call_named(pi, "NotifyKeyListeners", vec![Value::Name(k), l]);
                self.vm.call_named(pi, "NotifyKeyStateListeners", vec![Value::Name(k), Value::Byte(1)]);
            }
        }
        for &k in &prev {
            if !names.contains(&k) {
                let l = listeners(self, "mKeyUp");
                self.vm.call_named(pi, "NotifyKeyListeners", vec![Value::Name(k), l]);
                self.vm.call_named(pi, "NotifyKeyStateListeners", vec![Value::Name(k), Value::Byte(2)]);
            }
        }
        self.prev_keys = names;
    }

    pub fn player_input(&mut self) -> Option<ObjId> {
        let g = self.goat?;
        let pc = self.vm.get(g, "Controller").obj()?;
        self.vm.get(pc, "PlayerInput").obj()
    }

    /// GGGameInfo.OnX(...) as GGGoat calls it.
    pub fn event(&mut self, e: &GameEvent) {
        let (Some(game), Some(g)) = (self.game, self.goat) else { return };
        let goat = Value::Obj(Some(g));
        let (name, args) = match e {
            GameEvent::Landed => ("OnLanded", vec![goat, Value::Obj(None)]),
            GameEvent::Airborne(h) => ("OnAirborne", vec![goat, Value::Float(*h)]),
            GameEvent::Ground => ("OnGround", vec![goat, Value::Obj(None)]),
            GameEvent::Ragdoll(r) => ("OnRagdoll", vec![goat, Value::Bool(*r)]),
            GameEvent::WallRunning(r) => ("OnWallRunning", vec![goat, Value::Bool(*r)]),
            GameEvent::WallJump => ("OnWallJump", vec![goat]),
            GameEvent::Baa => ("OnBaa", vec![goat]),
            GameEvent::UseAbility(class) => {
                let ab = self.helper_object(class);
                ("OnUseAbility", vec![goat, Value::Obj(ab), Value::Obj(None)])
            }
            GameEvent::TrickMade(class) => {
                let t = self.vm.class_by_name(class).map(|c| self.vm.spawn_object(c, None));
                // trickMade.mOwner.mGoatController.Pawn is the trickster (GGAIController.OnTrickMade)
                if let (Some(t), Some(tm)) = (t, self.helper_object("GGTrickManager")) {
                    let pc = self.vm.get(g, "Controller");
                    self.vm.set(tm, "mGoatController", pc);
                    self.vm.set(t, "mOwner", Value::Obj(Some(tm)));
                }
                ("OnTrickMade", vec![Value::Obj(t)])
            }
            GameEvent::Manual(doing, ok) => ("OnManual", vec![goat, Value::Bool(*doing), Value::Bool(*ok)]),
            GameEvent::PlayerRespawn => {
                let pc = self.vm.get(g, "Controller");
                ("OnPlayerRespawn", vec![pc, Value::Bool(true)])
            }
            GameEvent::Exploded(export, _) => {
                let actor = self.map_actor(*export);
                // Explode(): TriggerEventClass(GGSeqEvent_Explosion, self) then OnExplosion
                if let Some(a) = actor {
                    self.kismet_actor_event(a, "GGSeqEvent_Explosion");
                }
                ("OnExplosion", vec![Value::Obj(actor)])
            }
            GameEvent::Fractured(export, by_goat) => {
                let actor = self.map_actor(*export);
                ("OnFractured", vec![Value::Obj(actor), if *by_goat { goat } else { Value::Obj(None) }])
            }
        };
        if self.vm.trace_calls || !matches!(e, GameEvent::Airborne(_) | GameEvent::Ground) {
            let what = format!("{e:?}");
            self.vm.say(format!("== event {what}"));
        }
        self.vm.call_named(game, name, args);
    }

    /// One shared instance of a helper class (abilities passed with OnUseAbility).
    fn helper_object(&mut self, class: &str) -> Option<ObjId> {
        if let Some(&o) = self.helpers.get(class) {
            return Some(o);
        }
        let c = self.vm.class_by_name(class)?;
        let o = self.vm.spawn_object(c, None);
        self.helpers.insert(class.to_string(), o);
        Some(o)
    }
}

/// GGGameInfoBase.RegisterDesiredEvents: add the listener to every mOnX list whose OnX it
/// implements with a body (NotifyOnPossess/NotifyOnUnpossess -> mOnPossess/mOnUnpossess).
pub fn register_desired_events(vm: &mut Vm, game: ObjId, listener: ObjId, add: bool) {
    let class = vm.obj(listener).class;
    // every mOnX array the game class declares (not only the ones already written)
    let mut names: Vec<String> = vec![];
    let mut c = Some(vm.obj(game).class);
    while let Some(k) = c.filter(|&k| k != super::NONE_CLASS) {
        for p in &vm.classes[k].props {
            let n = vm.name_of(p.name);
            if n.starts_with("mOn") && matches!(p.ty, super::Ty::Array(_)) && !names.iter().any(|x| x == n) {
                names.push(n.to_string());
            }
        }
        c = vm.classes[k].super_;
    }
    let lists: Vec<(String, String)> = names.into_iter().map(|n| {
        let func = match n.as_str() {
            "mOnPossess" => "NotifyOnPossess".to_string(),
            "mOnUnpossess" => "NotifyOnUnpossess".to_string(),
            other => other[1..].to_string(),
        };
        (n, func)
    }).collect();
    let mut joined = vec![];
    for (list, func) in lists {
        if vm.defines(class, &func).is_none() {
            continue;
        }
        let mut a = match vm.get(game, &list) {
            Value::Array(a) => a,
            _ => Default::default(),
        };
        let v = Value::Obj(Some(listener));
        a.retain(|x| *x != v);
        if add {
            a.push(v);
        }
        vm.set(game, &list, Value::Array(a));
        joined.push(func);
    }
    if add && !joined.is_empty() {
        let n = vm.obj(listener).name.clone();
        vm.say(format!("{n} listens to {}", joined.join(", ")));
    }
}

impl Host {
    /// The game camera (Unreal units) into PlayerCamera's Location/Rotation and CameraCache.POV.
    pub fn sync_camera(&mut self, location: [f32; 3], rotation: [i32; 3]) {
        let Some(&cam) = self.vm.globals.get("camera") else { return };
        let loc = self.vec(location);
        let rot = self.vm.rotator(rotation[0], rotation[1], rotation[2]);
        self.vm.set(cam, "Location", loc.clone());
        self.vm.set(cam, "Rotation", rot.clone());
        let (cache, pov, l, r) = (self.vm.sym("CameraCache"), self.vm.sym("POV"), self.vm.sym("Location"), self.vm.sym("Rotation"));
        if let Some(Value::Struct(cc)) = self.vm.obj_mut(cam).props.get_mut(&cache) {
            if let Value::Struct(p) = cc.field_mut(pov) {
                *p.field_mut(l) = loc;
                *p.field_mut(r) = rot;
            }
        }
    }
}
