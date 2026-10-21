//! GGNpc people (GGNpc.uc / GGAIController.uc): idle in their placed animation, notice the goat,
//! applaud its tricks, guard their items (and kick the goat away), get knocked into ragdoll,
//! stand up after mStandUpDelay and panic-run away.

use crate::convert::{self, S};
use crate::goat::Goat;
use crate::physics::Physics;
use crate::ragdoll::{ActiveRagdoll, CharacterBody, StartRagdoll, StopRagdoll};
use crate::rig::{self, Animator, Rig, SkelAssets, SkelCache};
use bevy::prelude::*;
use rapier3d::prelude as rp;
use upk::props::Value;
use upk::registry::{self, ObjRef, Registry};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NpcState {
    Idle,
    Ragdoll,
    Panic,
    /// Looking at the goat (notice / pointing) until `timer` runs out.
    Look,
    /// Playing an applaud animation facing the goat.
    Applaud,
    /// ProtectItem: running at the goat to kick it off the guarded item.
    Protect,
    /// Attack animation; the kick lands half way through (`attack_hit_at`).
    Attack,
    /// Angry animation after a kick.
    Angry,
    /// Walking back to where it was placed.
    Return,
    /// GGAIController FollowRoute: walking to the next path node of the scripted route.
    FollowRoute,
    /// Interacting with an item next to a route node (Interacting state).
    Interact,
}

/// GGPawnBase.EScriptedRouteType.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum RouteType {
    Loop,
    Random,
    Standard,
}

/// GGPawnBase.InteractionInfo for an item next to one of the route's path nodes.
#[derive(Clone, Debug)]
pub struct Interact {
    /// Index of InteractionInfo.NearestPathNode in the route.
    pub node: usize,
    pub look_at: Vec3,
    /// SpecifyRotation / InteractionRotation (Bevy yaw).
    pub yaw: Option<f32>,
    pub anims: Vec<String>,
    pub duration: f32,
}

/// The scripted route (GGPawnBase.mGeneratedPath + GGAIController route state).
#[derive(Clone, Debug)]
pub struct Route {
    pub nodes: Vec<Vec3>,
    pub kind: RouteType,
    pub index: usize,
    pub reversed: bool,
    /// mGroundSpeedForward while walking the route (m/s).
    pub speed: f32,
    pub interacts: Vec<Interact>,
    /// SRT_RANDOM: the ArrivedAtPathNode timer (distance / speed).
    pub timeout: f32,
    /// Nodes reached so far (tests).
    pub reached: u32,
    /// Interactions done so far (tests).
    pub interacted: u32,
}

/// GGAIController IntervalInfo: a random cool-down window.
#[derive(Clone, Copy, Debug)]
pub struct Interval {
    pub min: f32,
    pub max: f32,
    pub current: f32,
    pub last: f32,
}

impl Interval {
    const fn new(min: f32, max: f32) -> Self {
        Self { min, max, current: max, last: 0.0 }
    }
    pub fn ready(&self, now: f32) -> bool {
        self.last == 0.0 || now - self.last >= self.current
    }
    fn stamp(&mut self, now: f32, rnd: f32) {
        self.last = now.max(1e-3);
        self.current = self.min + (self.max - self.min) * rnd;
    }
}

/// An item this person guards (GGNpc.mProtectItems).
#[derive(Clone, Copy, Debug)]
pub struct Protect {
    pub item: Vec3,
    pub radius: f32,
}

/// Animation name lists and reaction settings read from the NPC's properties.
#[derive(Clone, Debug, Default)]
pub struct Reactions {
    pub applaud: Vec<String>,
    pub angry: Vec<String>,
    pub attack: Vec<String>,
    pub run: Vec<String>,
    pub run_speed: f32,
    pub sight_radius: f32,
    pub applaud_and_notice: bool,
    pub attack_range: f32,
    pub attack_momentum: f32,
}

#[derive(Component)]
pub struct Npc {
    pub state: NpcState,
    pub timer: f32,
    pub default_anim: String,
    pub panic_anims: Vec<String>,
    pub panic_speed: f32,
    pub stand_up_delay: f32,
    pub yaw: f32,
    pub times_knocked: u32,
    /// GGNpc.GetScore / GetActorName (material 0's physical material property).
    pub score: i32,
    pub actor_name: String,
    pub home: Vec3,
    pub home_yaw: f32,
    pub react: Reactions,
    pub protect: Option<Protect>,
    /// Has noticed the goat already (mIgnoreSameGoat).
    pub seen_goat: bool,
    pub visibility: Interval,
    pub notice: Interval,
    pub applaud: Interval,
    pub interaction: Interval,
    pub attack: Interval,
    pub attack_hit_at: f32,
    /// Why the current reaction started ("notice", "applaud", "wallrun").
    pub reason: &'static str,
    pub route: Option<Route>,
    /// Driven by its AI controller script in the VM (uvm::npcs) instead of the code below.
    pub script: Option<ScriptDrive>,
    rng: u32,
}

/// What an NPC's script controller wants this frame (set by uvm::npcs).
#[derive(Default, Clone, Debug)]
pub struct ScriptDrive {
    /// MoveTo / MoveToward destination (Bevy space).
    pub goal: Option<Vec3>,
    /// Metres per second.
    pub speed: f32,
    /// SetRotation target yaw (cleared once reached).
    pub yaw: Option<f32>,
    /// (animation, rate, loop) from SetAnimationInfoStruct, with a change counter.
    pub anim: Option<(String, f32, bool)>,
    pub anim_serial: u32,
    played: u32,
    /// GGNpc.StandUp succeeded in script: end the ragdoll.
    pub stand_up: bool,
    /// Set on fire: the controller should Panic (GGNpc.OnSetOnFire).
    pub panic: bool,
}

impl Npc {
    fn rand(&mut self) -> f32 {
        // xorshift32
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as f32 / (1u32 << 24) as f32
    }
    fn pick(&mut self, names: &[String], fallback: &str) -> String {
        if names.is_empty() {
            return fallback.to_string();
        }
        let i = ((self.rand() * names.len() as f32) as usize).min(names.len() - 1);
        names[i].clone()
    }
}

/// Set by the goat (headbutt / running into them); handled by `update`.
#[derive(Component)]
pub struct Knocked {
    pub velocity: Vec3,
}

fn first_anim_name(reg: &Registry, props: &[(usize, upk::props::Prop)], key: &str) -> Vec<String> {
    match registry::find(props, key) {
        Some((pk, Value::Struct(_, inner))) => registry::name_array(&reg.packages[pk], upk::props::find(inner, "AnimationNames")),
        _ => vec![],
    }
}

fn anim_speed(props: &[(usize, upk::props::Prop)], key: &str) -> f32 {
    match registry::find(props, key) {
        Some((_, Value::Struct(_, inner))) => match upk::props::find(inner, "MovementSpeed") {
            Some(Value::Float(f)) => *f,
            _ => 700.0,
        },
        _ => 700.0,
    }
}

pub(crate) fn float(props: &[(usize, upk::props::Prop)], key: &str, d: f32) -> f32 {
    match registry::find(props, key) {
        Some((_, Value::Float(f))) => *f,
        _ => d,
    }
}

pub(crate) fn vec3(props: &[(usize, upk::props::Prop)], key: &str, d: [f32; 3]) -> [f32; 3] {
    match registry::find(props, key) {
        Some((_, Value::Vector(v))) => *v,
        _ => d,
    }
}

pub(crate) fn obj(reg: &Registry, props: &[(usize, upk::props::Prop)], key: &str) -> Option<ObjRef> {
    match registry::find(props, key) {
        Some((pk, Value::Object(o))) => reg.resolve(pk, *o),
        _ => None,
    }
}

pub(crate) fn objs(reg: &Registry, props: &[(usize, upk::props::Prop)], key: &str) -> Vec<Option<ObjRef>> {
    match registry::find(props, key) {
        Some((pk, v)) => upk::material::object_array(Some(v)).into_iter().map(|o| reg.resolve(pk, o)).collect(),
        None => vec![],
    }
}

/// mProtectItems[0]: the guarded actor's location and radius.
fn protect_item(reg: &Registry, props: &[(usize, upk::props::Prop)]) -> Option<Protect> {
    let (pk, v) = registry::find(props, "mProtectItems")?;
    let first = upk::material::struct_array(&reg.packages[pk], Some(v)).into_iter().next()?;
    let radius = match upk::props::find(&first, "ProtectRadius") {
        Some(Value::Float(f)) => *f,
        _ => 384.0,
    };
    let item = match upk::props::find(&first, "ProtectItem") {
        Some(Value::Object(o)) => reg.resolve(pk, *o)?,
        _ => return None,
    };
    let ip = reg.merged_props(item);
    Some(Protect { item: convert::pos(vec3(&ip, "Location", [0.0; 3])), radius: radius * S })
}

fn bool_prop(props: &[(usize, upk::props::Prop)], key: &str) -> Option<bool> {
    match registry::find(props, key) {
        Some((_, Value::Bool(b))) => Some(*b),
        _ => None,
    }
}

/// Unreal yaw (rotation units) to our yaw (pawns face +X, characters face -Z at yaw 0).
fn ue_yaw(yaw: i32) -> f32 {
    -(yaw as f32 * std::f32::consts::TAU / 65536.0) - std::f32::consts::FRAC_PI_2
}

/// mGeneratedPath / mScriptedRouteType / mInteractItems (PawnUsesScriptedRoute).
fn load_route(reg: &Registry, props: &[(usize, upk::props::Prop)]) -> Option<Route> {
    if bool_prop(props, "mUseScriptedRoute") != Some(true) || bool_prop(props, "mDelayedRouteStart") == Some(true) {
        // delayed routes wait for a Kismet trigger
        return None;
    }
    let refs: Vec<Option<ObjRef>> = objs(reg, props, "mGeneratedPath");
    let nodes: Vec<Vec3> = refs
        .iter()
        .filter_map(|r| r.map(|r| convert::pos(vec3(&reg.merged_props(r), "Location", [0.0; 3]))))
        .collect();
    if nodes.len() < 2 || nodes.len() != refs.len() {
        return None;
    }
    // PostBeginPlay: the deprecated mLoopScriptedRoute forces SRT_LOOP; the enum defaults to SRT_LOOP
    let kind = match registry::find(props, "mScriptedRouteType") {
        _ if bool_prop(props, "mLoopScriptedRoute") == Some(true) => RouteType::Loop,
        Some((_, Value::Enum(e) | Value::Name(e))) if e.contains("RANDOM") => RouteType::Random,
        Some((_, Value::Enum(e) | Value::Name(e))) if e.contains("STANDARD") => RouteType::Standard,
        Some((_, Value::Byte(1))) => RouteType::Random,
        Some((_, Value::Byte(2))) => RouteType::Standard,
        _ => RouteType::Loop,
    };
    // SetAnimationInfoStruct(mDefaultAnimationInfo): MovementSpeed, else the default GroundSpeed
    // (600); bIsWalking (<= 100) moves at WalkingPct (0.5)
    let mut speed = match registry::find(props, "mDefaultAnimationInfo") {
        Some((_, Value::Struct(_, inner))) => match upk::props::find(inner, "MovementSpeed") {
            Some(Value::Float(f)) if *f > 0.0 => *f,
            _ => 600.0,
        },
        _ => 600.0,
    };
    if speed <= 100.0 {
        speed *= 0.5;
    }
    let mut interacts = Vec::new();
    if let Some((pk, v)) = registry::find(props, "mInteractItems") {
        for info in upk::material::struct_array(&reg.packages[pk], Some(v)) {
            let node = match upk::props::find(&info, "NearestPathNode") {
                Some(Value::Object(o)) => reg.resolve(pk, *o),
                _ => None,
            };
            let Some(node) = node.and_then(|n| refs.iter().position(|r| *r == Some(n))) else { continue };
            let look_at = match upk::props::find(&info, "InteractionActor") {
                Some(Value::Object(o)) => reg.resolve(pk, *o).map(|a| convert::pos(vec3(&reg.merged_props(a), "Location", [0.0; 3]))),
                _ => None,
            }
            .unwrap_or(nodes[node]);
            let yaw = match (upk::props::find(&info, "SpecifyRotation"), upk::props::find(&info, "InteractionRotation")) {
                (Some(Value::Bool(true)), Some(Value::Rotator(r))) => Some(ue_yaw(r[1])),
                _ => None,
            };
            let anims = match upk::props::find(&info, "InteractionAnimationInfo") {
                Some(Value::Struct(_, inner)) => registry::name_array(&reg.packages[pk], upk::props::find(inner, "AnimationNames")),
                _ => vec![],
            };
            let duration = match upk::props::find(&info, "InteractionDuration") {
                Some(Value::Float(f)) => *f,
                _ => 0.0,
            };
            interacts.push(Interact { node, look_at, yaw, anims, duration });
        }
    }
    Some(Route { nodes, kind, index: 0, reversed: false, speed: speed * S, interacts, timeout: 0.0, reached: 0, interacted: 0 })
}

/// ArrivedAtPathNode: pick the next node by route type.
fn advance_route(r: &mut Route, rnd: f32, from: Vec3) {
    let last = r.nodes.len() - 1;
    match r.kind {
        RouteType::Standard => {
            if r.index == last {
                r.reversed = true;
            } else if r.reversed && r.index == 0 {
                r.reversed = false;
            }
            r.index = if r.reversed { r.index - 1 } else { r.index + 1 };
        }
        RouteType::Loop => r.index = if r.index == last { 0 } else { r.index + 1 },
        RouteType::Random => {
            r.index = ((rnd * r.nodes.len() as f32) as usize).min(last);
            // don't get stuck trying to reach a node: arrive after distance / speed anyway
            r.timeout = from.distance(r.nodes[r.index]) / r.speed.max(0.01);
        }
    }
}

/// The closest route node (resuming the route after a ragdoll / panic / chase).
fn nearest_node(r: &Route, at: Vec3) -> usize {
    (0..r.nodes.len()).min_by(|&a, &b| at.distance_squared(r.nodes[a]).total_cmp(&at.distance_squared(r.nodes[b]))).unwrap_or(0)
}

/// Spawn every GGNpc placed in the map (package 0 of the registry).
pub fn spawn_all(reg: &Registry, cache: &mut SkelCache, a: &mut SkelAssets, commands: &mut Commands, phys: &mut Physics) -> usize {
    let map = &reg.packages[0];
    let mut n = 0;
    for i in 0..map.exports.len() {
        let class = map.class_name(i as i32 + 1);
        if !class.starts_with("GGNpc") || !map.obj_path(i as i32 + 1).contains(".PersistentLevel.") {
            continue;
        }
        let actor = ObjRef { pkg: 0, export: i };
        let aps = reg.merged_props(actor);
        let Some(comp) = obj(reg, &aps, "Mesh") else { continue };
        let cps = reg.merged_props(comp);
        let Some(mesh) = obj(reg, &cps, "SkeletalMesh") else { continue };
        let anim = objs(reg, &cps, "AnimSets").into_iter().flatten().next();
        let physasset = obj(reg, &cps, "PhysicsAsset");
        let data = match rig::load(reg, cache, mesh, anim, physasset, a) {
            Ok(d) => d,
            Err(e) => {
                warn!("npc {}: {e:#}", map.obj_path(i as i32 + 1));
                continue;
            }
        };
        let mat0 = objs(reg, &cps, "Materials").into_iter().next().flatten().or_else(|| data.material_refs.first().copied().flatten());
        let (score, actor_name) = mat0
            .and_then(|m| upk::physasset::material_physmat(reg, m))
            .and_then(|pm| upk::physasset::score_info(reg, pm))
            .unwrap_or((10, "Human".into()));
        let overrides: Vec<Option<Handle<StandardMaterial>>> = objs(reg, &cps, "Materials")
            .into_iter()
            .map(|m| m.map(|r| rig::material_for(reg, r, a)))
            .collect();
        let (radius, height) = obj(reg, &aps, "CylinderComponent")
            .map(|c| {
                let p = reg.merged_props(c);
                (float(&p, "CollisionRadius", 25.0), float(&p, "CollisionHeight", 44.0))
            })
            .unwrap_or((25.0, 44.0));
        let loc = convert::pos(vec3(&aps, "Location", [0.0; 3]));
        let rot = match registry::find(&aps, "Rotation") {
            Some((_, Value::Rotator(r))) => *r,
            _ => [0; 3],
        };
        // pawns face Unreal +X; our characters face -Z at yaw 0
        let yaw = rot[1] as f32 * std::f32::consts::TAU / 65536.0;
        let bevy_yaw = -yaw - std::f32::consts::FRAC_PI_2;
        let mut default_anim = first_anim_name(reg, &aps, "mDefaultAnimationInfo");
        if default_anim.is_empty() {
            default_anim = first_anim_name(reg, &aps, "mIdleAnimationInfo");
        }
        let panic_anims = first_anim_name(reg, &aps, "mPanicAnimationInfo");
        let panic_speed = anim_speed(&aps, "mPanicAnimationInfo");
        let stand_up_delay = float(&aps, "mStandUpDelay", 7.5);
        let react = Reactions {
            applaud: first_anim_name(reg, &aps, "mApplaudAnimationInfo"),
            angry: first_anim_name(reg, &aps, "mAngryAnimationInfo"),
            attack: first_anim_name(reg, &aps, "mAttackAnimationInfo"),
            run: first_anim_name(reg, &aps, "mRunAnimationInfo"),
            run_speed: anim_speed(&aps, "mRunAnimationInfo"),
            sight_radius: float(&aps, "SightRadius", 1800.0),
            applaud_and_notice: !matches!(registry::find(&aps, "mApplaudAndNoticeGoat"), Some((_, Value::Bool(false)))),
            attack_range: float(&aps, "mAttackRange", 200.0),
            attack_momentum: float(&aps, "mAttackMomentum", 600.0),
        };
        let protect = protect_item(reg, &aps);
        let route = load_route(reg, &aps);
        let half = height * S;
        let e = commands
            .spawn((
                Transform::from_translation(loc).with_rotation(Quat::from_rotation_y(bevy_yaw)),
                Visibility::default(),
                Name::new(map.obj_name(i as i32 + 1)),
            ))
            .id();
        let body = phys.world.insert_body(rp::RigidBodyBuilder::kinematic_position_based().translation(convert::to_rapier(loc)));
        let r = (radius * S).min(half * 0.9);
        let collider = phys.world.insert_collider(rp::ColliderBuilder::capsule_y((half - r).max(0.01), r), Some(body));
        phys.owners.insert(collider, e);
        let translation = vec3(&cps, "Translation", [0.0, 0.0, 0.0]);
        let mesh_xf = Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)) * Transform::from_translation(convert::pos(translation));
        let start = default_anim.first().cloned().unwrap_or_else(|| "Idle".into());
        if data.clip_or_variant(&start).is_none() {
            warn!("npc {}: animation {start:?} not in its anim set ({} clips)", map.obj_name(i as i32 + 1), data.clips.len());
        }
        commands.entity(e).insert(crate::rider::RiderLook { data: data.clone(), overrides: overrides.clone(), translation: convert::pos(translation) });
        rig::spawn(commands, e, data, mesh_xf, &overrides, &start);
        commands.entity(e).insert((
            CharacterBody { body, collider, half_height: half },
            Npc {
                state: if route.is_some() { NpcState::FollowRoute } else { NpcState::Idle },
                timer: 0.0,
                default_anim: start,
                panic_anims,
                panic_speed,
                stand_up_delay,
                yaw: bevy_yaw,
                times_knocked: 0,
                score,
                actor_name,
                home: loc,
                home_yaw: bevy_yaw,
                react,
                protect,
                seen_goat: false,
                // Default__GGAIController interval infos
                visibility: Interval::new(1.5, 3.0),
                notice: Interval::new(25.0, 35.0),
                applaud: Interval::new(10.0, 20.0),
                interaction: Interval::new(7.0, 15.0),
                attack: Interval::new(2.5, 5.0),
                attack_hit_at: 0.0,
                reason: "",
                route,
                script: None,
                rng: 0x9E37_79B9 ^ (i as u32).wrapping_mul(2_654_435_761),
            },
        ));
        n += 1;
    }
    n
}

/// Tricks people applaud (Default__GGAIController.mApplaudGoatTricks).
fn applauds(t: crate::score::TrickId) -> bool {
    matches!(t, crate::score::TrickId::PerfectLanding | crate::score::TrickId::Rotation | crate::score::TrickId::Bounce | crate::score::TrickId::Baseball)
}

/// Kinematic ground-following step for walking/running people.
fn step_ground(phys: &mut Physics, cb: &CharacterBody, t: &mut Transform, dir: Vec3, speed: f32, dt: f32) {
    let mut p = t.translation + dir * speed * dt;
    let ray = rp::Ray::new(convert::to_rapier(p + Vec3::Y * cb.half_height), rp::Vector::NEG_Y);
    let filter = rp::QueryFilter::exclude_dynamic().exclude_rigid_body(cb.body);
    if let Some((_, toi)) = phys.world.cast_ray(&ray, cb.half_height * 4.0, true, filter) {
        p.y = p.y + cb.half_height - toi + cb.half_height;
    }
    t.translation = p;
    if let Some(rb) = phys.world.bodies.get_mut(cb.body) {
        rb.set_next_kinematic_translation(convert::to_rapier(p));
    }
}

fn yaw_to(from: Vec3, to: Vec3) -> f32 {
    f32::atan2(-(to.x - from.x), -(to.z - from.z))
}

/// Play a clip by name; returns its duration in seconds (0 if missing).
fn play(rig: &Rig, anim: &mut Animator, name: &str) -> f32 {
    match rig.data.clip_or_variant(name) {
        Some(c) => {
            anim.play(c, 1.0);
            let clip = &rig.data.clips[c];
            clip.length / clip.rate.max(0.01)
        }
        None => 0.0,
    }
}

/// No world geometry between two points (!GeometryBetween).
pub fn clear_line(phys: &Physics, from: Vec3, to: Vec3) -> bool {
    let d = to - from;
    let len = d.length();
    if len < 0.01 {
        return true;
    }
    let static_only = |_: rp::ColliderHandle, c: &rp::Collider| c.parent().is_none();
    let ray = rp::Ray::new(convert::to_rapier(from), convert::to_rapier(d / len));
    phys.world.cast_ray(&ray, len, true, rp::QueryFilter::default().predicate(&static_only)).is_none()
}

/// ApplaudGoat: half the time an applaud animation, otherwise turn and point/look at the goat.
fn start_applaud(n: &mut Npc, rig: &Rig, anim: &mut Animator, now: f32) {
    let (r1, r2) = (n.rand(), n.rand());
    n.applaud.stamp(now, r1);
    n.interaction.stamp(now, r2);
    if n.rand() < 0.5 {
        let list = n.react.applaud.clone();
        let name = n.pick(&list, "Clap");
        let len = play(rig, anim, &name);
        n.state = NpcState::Applaud;
        n.timer = (len - 0.1).max(1.0);
    } else {
        n.state = NpcState::Look;
        n.timer = 5.0;
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn update(
    mut commands: Commands,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut tricks: ResMut<crate::score::TrickEvents>,
    time: Res<Time>,
    mut phys: ResMut<Physics>,
    mut goats: Query<(Entity, &mut Goat, &Transform), Without<Npc>>,
    mut npcs: Query<(Entity, &mut Npc, &mut Transform, &CharacterBody, &Rig, &mut Animator, Option<&Knocked>, Has<ActiveRagdoll>)>,
) {
    let _prof_scope = crate::prof::scope("npc::update");
    let dt = time.delta_secs().min(0.05) * phys.time_scale;
    let now = time.elapsed_secs();
    let made: Vec<crate::score::TrickId> = std::mem::take(&mut tricks.0);
    let applaud_worthy = made.iter().any(|&m| applauds(m));
    let mut goat = goats.single_mut().ok();
    let goat_pos = goat.as_ref().map(|g| g.2.translation);
    let goat_wall_running = goat.as_ref().is_some_and(|g| g.1.wall.active);
    let goat_ragdoll = goat.as_ref().is_some_and(|g| g.1.ragdoll);
    for (e, mut n, mut t, cb, rig, mut anim, knocked, ragdolled) in &mut npcs {
        let n = &mut *n;
        if let Some(k) = knocked {
            commands.entity(e).remove::<Knocked>();
            if n.state != NpcState::Ragdoll && rig.data.ragdoll.is_some() {
                commands.entity(e).insert(StartRagdoll { velocity: k.velocity });
                sounds.play("npc_knocked", Some(t.translation));
                n.state = NpcState::Ragdoll;
                n.timer = n.stand_up_delay;
                n.times_knocked += 1;
                continue;
            }
        }
        // a script controller decides; walk, turn and animate as it says
        if let Some(sd) = n.script.as_mut() {
            if n.state == NpcState::Ragdoll {
                // the controller's StandUp timer decides (GGAIController.StandUp -> GGNpc.StandUp);
                // n.timer only keeps drowning people down
                n.timer -= dt;
                if sd.stand_up && n.timer <= 0.0 && ragdolled {
                    sd.stand_up = false;
                    commands.entity(e).insert(StopRagdoll);
                    n.state = NpcState::Idle;
                }
                continue;
            }
            if sd.played != sd.anim_serial {
                sd.played = sd.anim_serial;
                if let Some((name, rate, _)) = &sd.anim {
                    if let Some(c) = rig.data.clip_or_variant(name) {
                        anim.play(c, rate.max(0.05));
                    }
                }
            }
            let turn = |from: f32, to: f32| {
                let d = (to - from + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                from + d * (dt * 8.0).min(1.0)
            };
            if let Some(g) = sd.goal {
                let flat = Vec3::new(g.x - t.translation.x, 0.0, g.z - t.translation.z);
                if flat.length() > 0.05 {
                    n.yaw = turn(n.yaw, f32::atan2(-flat.x, -flat.z));
                    let speed = sd.speed.min(flat.length() / dt.max(1e-4));
                    step_ground(&mut phys, cb, &mut t, flat.normalize(), speed, dt);
                }
            } else if let Some(y) = sd.yaw {
                n.yaw = turn(n.yaw, y);
                if ((y - n.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI).abs() < 0.02 {
                    sd.yaw = None;
                }
            }
            t.rotation = Quat::from_rotation_y(n.yaw);
            continue;
        }
        // can this person see the goat? (PawnInRange && !GeometryBetween)
        let head = t.translation + Vec3::Y * cb.half_height * 0.8;
        let sight = n.react.sight_radius * S;
        let sees = |phys: &Physics| goat_pos.is_some_and(|g| g.distance(t.translation) <= sight && clear_line(phys, head, g + Vec3::Y * 0.3));
        let calm = matches!(n.state, NpcState::Idle | NpcState::Look | NpcState::FollowRoute);

        // OnTrickMade: applaud tricks done in sight
        if applaud_worthy && calm && n.protect.is_none() && n.react.applaud_and_notice && n.applaud.ready(now) && n.interaction.ready(now) && sees(&phys) {
            sounds.play("npc_applaud", Some(t.translation));
            start_applaud(n, rig, &mut anim, now);
            n.reason = "applaud";
        }

        // periodic visibility check (SeePlayer)
        if matches!(n.state, NpcState::Idle | NpcState::Look | NpcState::Return | NpcState::FollowRoute) && n.visibility.ready(now) {
            let r = n.rand();
            n.visibility.stamp(now, r);
            if let (true, Some(g)) = (sees(&phys), goat_pos) {
                if let Some(pr) = n.protect {
                    // ProtectItem: an enemy near our item gets chased off
                    if g.distance(pr.item) <= pr.radius && !goat_ragdoll {
                        n.state = NpcState::Protect;
                    }
                } else if goat_wall_running && n.react.applaud_and_notice && n.applaud.ready(now) && n.interaction.ready(now) {
                    sounds.play("npc_applaud", Some(t.translation));
                    start_applaud(n, rig, &mut anim, now);
                    n.reason = "wallrun";
                } else if !n.seen_goat
                    && n.react.applaud_and_notice
                    && n.notice.ready(now)
                    && n.interaction.ready(now)
                    && matches!(n.state, NpcState::Idle | NpcState::FollowRoute)
                {
                    // notice: look at the goat for 5 s and say something
                    n.seen_goat = true;
                    let (r1, r2) = (n.rand(), n.rand());
                    n.notice.stamp(now, r1);
                    n.interaction.stamp(now, r2);
                    sounds.play("npc_notice", Some(t.translation));
                    n.reason = "notice";
                    n.state = NpcState::Look;
                    n.timer = 5.0;
                }
            }
        }

        match n.state {
            NpcState::Idle => {
                if let Some(r) = n.route.as_mut() {
                    // ResumeDefaultAction -> ResumeScriptedRoute from the closest node
                    r.index = nearest_node(r, t.translation);
                    n.state = NpcState::FollowRoute;
                    continue;
                }
                let name = n.default_anim.clone();
                play(rig, &mut anim, &name);
            }
            NpcState::FollowRoute => {
                let name = n.default_anim.clone();
                play(rig, &mut anim, &name);
                let rnd = n.rand();
                let Some(r) = n.route.as_mut() else {
                    n.state = NpcState::Idle;
                    continue;
                };
                let target = r.nodes[r.index];
                let flat = Vec3::new(target.x - t.translation.x, 0.0, target.z - t.translation.z);
                let step = r.speed * dt;
                let mut arrived = flat.length() <= step.max(0.05);
                if r.kind == RouteType::Random && r.timeout > 0.0 {
                    r.timeout -= dt;
                    arrived |= r.timeout <= 0.0;
                }
                if !arrived {
                    // SetRotation toward the node, then MoveToward it
                    let want = f32::atan2(-flat.x, -flat.z);
                    let d = (want - n.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    n.yaw += d * (dt * 8.0).min(1.0);
                    t.rotation = Quat::from_rotation_y(n.yaw);
                    step_ground(&mut phys, cb, &mut t, flat.normalize(), r.speed, dt);
                } else {
                    // ArrivedAtPathNode + WhatToDoNext
                    let at = r.index;
                    r.reached += 1;
                    advance_route(r, rnd, t.translation);
                    if let Some(it) = r.interacts.iter().find(|i| i.node == at).cloned() {
                        r.interacted += 1;
                        n.yaw = it.yaw.unwrap_or_else(|| yaw_to(t.translation, it.look_at));
                        t.rotation = Quat::from_rotation_y(n.yaw);
                        let name = n.pick(&it.anims, &n.default_anim.clone());
                        play(rig, &mut anim, &name);
                        n.state = NpcState::Interact;
                        n.timer = it.duration;
                    }
                }
            }
            NpcState::Interact => {
                n.timer -= dt;
                if n.timer <= 0.0 {
                    // ResumeScriptedRoute
                    n.state = NpcState::FollowRoute;
                }
            }
            NpcState::Look | NpcState::Applaud => {
                n.timer -= dt;
                if let Some(g) = goat_pos {
                    let want = yaw_to(t.translation, g);
                    let d = (want - n.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
                    n.yaw += d * (dt * 4.0).min(1.0);
                    t.rotation = Quat::from_rotation_y(n.yaw);
                }
                if n.state == NpcState::Look {
                    let name = n.default_anim.clone();
                    play(rig, &mut anim, &name);
                }
                if n.timer <= 0.0 {
                    // ResumeDefaultAction
                    n.state = NpcState::Idle;
                    if n.route.is_none() {
                        n.yaw = n.home_yaw;
                        t.rotation = Quat::from_rotation_y(n.yaw);
                    }
                }
            }
            NpcState::Protect => {
                let (Some(g), Some(pr)) = (goat_pos, n.protect) else {
                    n.state = NpcState::Return;
                    continue;
                };
                if g.distance(pr.item) > pr.radius || goat_ragdoll {
                    // EndAttack: the goat left the item alone
                    n.state = NpcState::Return;
                    continue;
                }
                let run = n.react.run.first().cloned().unwrap_or_else(|| "Run".into());
                play(rig, &mut anim, &run);
                n.yaw = yaw_to(t.translation, g);
                t.rotation = Quat::from_rotation_y(n.yaw);
                let reach = cb.half_height * 0.6 + crate::goat::tune::COLLISION_RADIUS * S + n.react.attack_range * S;
                let flat = Vec3::new(g.x - t.translation.x, 0.0, g.z - t.translation.z);
                if flat.length() > n.react.attack_range * S {
                    let speed = n.react.run_speed * S;
                    step_ground(&mut phys, cb, &mut t, flat.normalize_or_zero(), speed, dt);
                }
                if flat.length() <= reach && n.attack.ready(now) {
                    // StartAttack: the kick lands half way through the attack animation
                    let list = n.react.attack.clone();
                    let name = n.pick(&list, "Kick");
                    let len = play(rig, &mut anim, &name).max(0.5);
                    n.state = NpcState::Attack;
                    n.timer = len;
                    n.attack_hit_at = len * 0.5;
                }
            }
            NpcState::Attack => {
                let before = n.timer;
                n.timer -= dt;
                if before > n.attack_hit_at && n.timer <= n.attack_hit_at {
                    // AttackPawn: momentum toward the goat plus up, 25% chance of a ragdoll
                    let r = n.rand();
                    n.attack.stamp(now, r);
                    let roll = n.rand();
                    let ragdoll_roll = n.rand();
                    if let Some((ge, g, gt)) = goat.as_mut() {
                        let mut dir = (gt.translation - t.translation).normalize_or(Vec3::X);
                        dir.y += 1.0;
                        let momentum = n.react.attack_momentum * (0.75 + 0.5 * roll);
                        g.velocity += dir * momentum * S;
                        g.grounded = false;
                        if ragdoll_roll < 0.25 && !g.ragdoll {
                            crate::goat::start_ragdoll(&mut commands, *ge, g, &mut phys);
                        }
                    }
                    sounds.play("npc_angry", Some(t.translation));
                }
                if n.timer <= 0.0 {
                    let list = n.react.angry.clone();
                    let name = n.pick(&list, "Angry_01");
                    let len = play(rig, &mut anim, &name);
                    n.state = NpcState::Angry;
                    n.timer = len.clamp(0.5, 3.0);
                }
            }
            NpcState::Angry => {
                n.timer -= dt;
                if n.timer <= 0.0 {
                    // DelayedGoToProtect
                    n.state = NpcState::Protect;
                }
            }
            NpcState::Return if n.route.is_some() => n.state = NpcState::Idle,
            NpcState::Return => {
                let home = n.home;
                let flat = Vec3::new(home.x - t.translation.x, 0.0, home.z - t.translation.z);
                if flat.length() < 0.3 {
                    t.translation = home;
                    if let Some(rb) = phys.world.bodies.get_mut(cb.body) {
                        rb.set_next_kinematic_translation(convert::to_rapier(home));
                    }
                    n.state = NpcState::Idle;
                    n.yaw = n.home_yaw;
                    t.rotation = Quat::from_rotation_y(n.yaw);
                } else {
                    let walk = if rig.data.clip("Walk").is_some() { "Walk" } else { "Run" };
                    play(rig, &mut anim, walk);
                    n.yaw = yaw_to(t.translation, home);
                    t.rotation = Quat::from_rotation_y(n.yaw);
                    step_ground(&mut phys, cb, &mut t, flat.normalize(), 300.0 * S, dt);
                }
            }
            NpcState::Ragdoll => {
                n.timer -= dt;
                if n.timer <= 0.0 && ragdolled {
                    commands.entity(e).insert(StopRagdoll);
                    sounds.play("npc_panic", Some(t.translation));
                    n.state = NpcState::Panic;
                    n.timer = 6.0;
                }
            }
            NpcState::Panic => {
                n.timer -= dt;
                let name = n.panic_anims.first().cloned().unwrap_or_else(|| "Run".into());
                if let Some(c) = rig.data.clip(&name).or_else(|| rig.data.clip("Run")) {
                    anim.play(c, 1.0);
                }
                // run away from the goat, following the ground
                let away = goat_pos.map(|g| Vec3::new(t.translation.x - g.x, 0.0, t.translation.z - g.z).normalize_or(Vec3::X)).unwrap_or(Vec3::X);
                n.yaw = f32::atan2(-away.x, -away.z);
                let speed = n.panic_speed * S;
                step_ground(&mut phys, cb, &mut t, away, speed, dt);
                t.rotation = Quat::from_rotation_y(n.yaw);
                if n.timer <= 0.0 {
                    n.state = NpcState::Idle;
                    // where they ended up becomes their new spot
                    n.home = t.translation;
                    n.home_yaw = n.yaw;
                }
            }
        }
    }
}

/// GGNpc.PlayFootStepSound: GGPawnSoundGroup HumanNPCFootstepSounds by the material underfoot.
pub fn footsteps(
    phys: Res<Physics>,
    sfx: Res<crate::audio::Sfx>,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    listener: Query<&GlobalTransform, With<Camera3d>>,
    mut q: Query<(&Npc, &Transform, &CharacterBody, &mut Animator)>,
) {
    let _prof_scope = crate::prof::scope("npc::footsteps");
    let ear = listener.single().map(|t| t.translation()).ok();
    for (n, t, cb, mut a) in &mut q {
        for cue in std::mem::take(&mut a.sounds) {
            sounds.play(&cue, Some(t.translation));
        }
        let steps = std::mem::take(&mut a.steps);
        if steps == 0 || n.state == NpcState::Ragdoll {
            continue;
        }
        // only people close enough to be heard
        if ear.is_none_or(|e| e.distance(t.translation) > 30.0) {
            continue;
        }
        let kind = crate::goat::material_below(&phys, t.translation, cb.half_height, cb.body);
        let key = format!("npcfoot:{kind}");
        let key = if sfx.cues.contains_key(&key) { key } else { "npcfoot:".to_string() };
        sounds.play(&key, Some(t.translation));
    }
}
