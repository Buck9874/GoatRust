//! Bicycles (GGBicycleAbstract -> GGSVehicle -> UDKVehicle, UDKVehicleSimCar): the placed bikes
//! as PhysX-style ray-cast vehicles (rapier DynamicRayCastVehicleController) on their two wheels
//! plus the two support wheels, the goat getting on with Use (R; PlayerController.FindVehicleToDrive
//! within VehicleCheckRadius) and riding at the SaddleSocket with the "Biker" animation.
//! Riding: W pedals (TorqueVSpeedCurve), S brakes / reverses, the bike steers toward the camera
//! (mSteerWithCamera, MaxSteerAngleCurve, SteerSpeed), Space jumps (JumpWithBike), A/D in the
//! air spin it (JumpFlipLeft/Right) and on the ground play GGBicycleAbstract's balance game: the
//! bike keeps leaning toward mFallDir (mFallSpeed), pressing the other way rights it
//! (mBalanceSpeed) and flips the fall side, the same way leans faster (mPenaltyFallSpeed); past
//! mFallThreshold it falls and the goat is thrown off. Crashes (hit velocity >=
//! mCollisionVelocityThreshold), tipping over (roll > mRollAngleThreshold) and explosions also
//! throw the rider off.
//!
//! The Shopping Goat mutator's cart (GGShoppingCart, from Default__GGShoppingCart in GoatGame.u)
//! is built the same way when the mutator's script spawns it and calls TryToDrive: its mesh has
//! its own goat (bDriverIsVisible false), it can't be left (DriverLeave refuses mUsedByMutator)
//! and it steers with A/D.

use crate::convert::{self, S};
use crate::goat::{Goat, InputState};
use crate::physics::{PhysBody, Physics};
use crate::rig::{self, SkelAssets, SkelCache};
use bevy::prelude::*;
use rapier3d::control::{DynamicRayCastVehicleController, WheelTuning};
use rapier3d::prelude as rp;
use upk::props::Value;
use upk::registry::{self, ObjRef, Registry};

/// Default__GGBicycleAbstract (the balance game).
const FALL_SPEED: f32 = 0.17;
const PENALTY_FALL_SPEED: f32 = 0.4;
const BALANCE_SPEED: f32 = 0.25;
const BALANCING_THRESHOLD: f32 = 0.82;
const FALL_THRESHOLD: f32 = 0.86;
const SIDE_FLIP_TORQUE: f32 = 45000.0;
const COLLISION_VELOCITY_THRESHOLD: f32 = 600.0;
const BIKER_SPEED_FACTOR: f32 = 0.35;
/// Default__GGGoat.VehicleCheckRadius.
const VEHICLE_CHECK_RADIUS: f32 = 200.0;
/// UDKVehicleSimCar.EngineBrakeFactor (bike).
const ENGINE_BRAKE_FACTOR: f32 = 0.025;
const SUSPENSION_TRAVEL: f32 = 5.0;
/// The PhysX mass the bike's jump impulse and flip torque were tuned for (kg).
const PHYSX_MASS: f32 = 640.0;
/// The cart's PhysX mass for mJumpForceSize (kg): ShoppingGoat_Physics_01's eight boxes
/// (248,500 uu^3 = 1.99 m^3 at 0.02 m/uu) x PhysMat_ShoppingCart's Density 130.
const CART_PHYSX_MASS: f32 = 258.0;
/// Default__GGSVehicle: mCollisionVelocityThreshold, mMinTimeBetweenCollisions.
const VEHICLE_COLLISION_VELOCITY: f32 = 800.0;
const MIN_TIME_BETWEEN_COLLISIONS: f32 = 1.0;
/// Pedal acceleration at the torque curve's peak (m/s^2) and the brake.
const PEDAL_ACCEL: f32 = 7.5;
const BRAKE: f32 = 6.0;

/// One vehicle class's defaults (Default__GGBicycleAbstract / Default__GGLongboardAbstract and
/// their wheel and SimulationObject subobjects).
pub struct Def {
    pub name: &'static str,
    /// (BoneName, BoneOffset, SteerFactor > 0, bPoweredWheel)
    wheels: &'static [(&'static str, [f32; 3], bool, bool)],
    /// WheelRadius (uu).
    radius: f32,
    torque: &'static [(f32, f32)],
    steer: &'static [(f32, f32)],
    steer_speed: f32,
    steer_with_camera: bool,
    /// Jump: mJumpForceSize / the PhysX mass, as a velocity change (uu/s).
    jump_dv: f32,
    roll_threshold: f32,
    /// mTripVelocityThreshold: tipping only throws the rider off below this speed.
    trip_velocity: Option<f32>,
    balance: bool,
    socket: &'static str,
    /// The driver mesh's translation on the vehicle (replaces the goat mesh's (0, 0, 8)).
    driver_offset: [f32; 3],
    /// The goat's animation while riding.
    anim: &'static str,
    mass: f32,
    score_name: &'static str,
    /// mTireSoundList (SetActiveTireSound indices), mJumpSound, CollisionSound.
    tire_cues: [&'static str; 3],
    jump_cue: &'static str,
    crash_cue: &'static str,
    /// Played when the jump starts (GGShoppingCart.mJumpSound; jump_cue is its landing sound).
    jump_start_cue: Option<&'static str>,
    tire_rule: TireRule,
    /// bDriverIsVisible (the cart's mesh has its own goat).
    pub driver_visible: bool,
}

/// Which SetTireSound picks the tire loop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TireRule {
    Bike,
    Board,
    Cart,
}

pub const BICYCLE: Def = Def {
    name: "Bicycle",
    wheels: &[
        ("TireBack", [0.0, 0.0, -3.0], false, true),
        ("TireFront", [0.0, 0.0, -3.0], true, true),
        ("Support_L", [0.0, -28.0, -3.0], false, false),
        ("Support_R", [0.0, 28.0, -3.0], false, false),
    ],
    radius: 35.0,
    torque: &[(-600.0, 0.0), (-300.0, 90.0), (0.0, 170.0), (1000.0, 110.0), (1450.0, 10.0), (1850.0, 0.0)],
    steer: &[(0.0, 45.0), (800.0, 30.0), (1200.0, 15.0), (1400.0, 8.0), (1800.0, 2.0)],
    steer_speed: 80.0,
    steer_with_camera: true,
    jump_dv: 210000.0 / PHYSX_MASS,
    roll_threshold: 1.4,
    trip_velocity: None,
    balance: true,
    socket: "SaddleSocket",
    driver_offset: [40.0, 0.0, -50.0],
    anim: "Biker",
    mass: 60.0,
    score_name: "Bicycle",
    // pedals, coasting, braking
    tire_cues: ["DrivenVehicles.Sound.Bicycle_Pedal_Cue_01", "DrivenVehicles.Sound.Bicycle_NoPedal_Cue_01", "DrivenVehicles.Sound.Bicycle_Brake_Cue_01"],
    jump_cue: "DrivenVehicles.Sound.Bicycle_jump_cue_01",
    crash_cue: "DrivenVehicles.Sound.Bicycle_Crash_Cue_01",
    jump_start_cue: None,
    tire_rule: TireRule::Bike,
    driver_visible: true,
};

pub const LONGBOARD: Def = Def {
    name: "Longboard",
    wheels: &[
        ("Wheel_Back_L", [0.0, 0.0, -4.0], false, true),
        ("Wheel_Back_R", [0.0, 0.0, -4.0], false, true),
        ("Wheel_Front_L", [0.0, 0.0, -4.0], true, true),
        ("Wheel_Front_R", [0.0, 0.0, -4.0], true, true),
    ],
    radius: 10.0,
    torque: &[(-600.0, 0.0), (-300.0, 130.0), (0.0, 210.0), (900.0, 130.0), (1450.0, 10.0), (1850.0, 0.0)],
    steer: &[(0.0, 35.0), (500.0, 18.0), (700.0, 14.0), (900.0, 9.0), (970.0, 7.0), (1500.0, 3.0)],
    steer_speed: 80.0,
    steer_with_camera: false,
    jump_dv: 14000.0 / 43.0,
    roll_threshold: 1.25,
    trip_velocity: Some(500.0),
    balance: false,
    socket: "BoardSocket",
    driver_offset: [5.0, 0.0, 45.0],
    anim: "Idle",
    mass: 25.0,
    score_name: "Longboard",
    // accelerating, braking, rolling
    tire_cues: ["Longboard.Sound.Effect_Longboard_Speed_Up_Cue", "Longboard.Sound.Effect_Longboard_Speed_Down_Cue", "Longboard.Sound.Effect_Longboard_Driving"],
    jump_cue: "Longboard.Sound.Effect_longboard_jump_cue_01",
    crash_cue: "Longboard.Sound.Effect_Longboard_Crash_Cue_01",
    jump_start_cue: None,
    tire_rule: TireRule::Board,
    driver_visible: true,
};

pub const SHOPPING_CART: Def = Def {
    name: "ShoppingCart",
    wheels: &[
        ("Wheel_Back_L", [0.0, 0.0, -5.0], false, true),
        ("Wheel_Back_R", [0.0, 0.0, -5.0], false, true),
        ("Wheel_Front_L", [0.0, 0.0, -5.0], true, true),
        ("Wheel_Front_R", [0.0, 0.0, -5.0], true, true),
    ],
    radius: 11.0,
    torque: &[(-600.0, 0.0), (-300.0, 80.0), (0.0, 110.0), (300.0, 140.0), (900.0, 80.0), (1300.0, 0.0)],
    steer: &[(0.0, 25.0), (200.0, 20.0), (400.0, 18.0), (600.0, 15.0), (700.0, 8.0)],
    steer_speed: 40.0,
    steer_with_camera: false,
    jump_dv: 90000.0 / CART_PHYSX_MASS,
    // the StayUpright constraint rights it; nothing throws the shopper out
    roll_threshold: f32::INFINITY,
    trip_velocity: None,
    balance: false,
    socket: "",
    driver_offset: [0.0, 0.0, 8.0],
    anim: "Idle",
    mass: 40.0,
    score_name: "Shopping Cart",
    // SetTireSound: below 400 uu/s SpeedUpDown, faster DriveLoop
    tire_cues: ["shoppingGoat.Sound.ShoppingGoat_DriveLoop_Cue", "shoppingGoat.Sound.ShoppingGoat_SpeedUpDown_Cue", "shoppingGoat.Sound.ShoppingGoat_DriveLoop_Cue"],
    jump_cue: "Goat_Sounds.Cue.Effect_ShoppingGoat_land_cue",
    crash_cue: "shoppingGoat.Sound.ShoppingGoat_Crash_Cue",
    jump_start_cue: Some("Goat_Sounds.Cue.Effect_ShoppingGoat_jump_cue"),
    tire_rule: TireRule::Cart,
    driver_visible: false,
};

fn eval(curve: &[(f32, f32)], x: f32) -> f32 {
    if x <= curve[0].0 {
        return curve[0].1;
    }
    for w in curve.windows(2) {
        if x <= w[1].0 {
            let a = (x - w[0].0) / (w[1].0 - w[0].0).max(1e-3);
            return w[0].1 + (w[1].1 - w[0].1) * a;
        }
    }
    curve[curve.len() - 1].1
}

/// AnimNodeBlend.SetBlendTarget: the lean child's weight moves linearly to the target.
#[derive(Clone, Debug)]
pub struct Blend {
    pub weight: f32,
    target: f32,
    time_to_go: f32,
}

impl Blend {
    fn set_target(&mut self, target: f32, time: f32) {
        self.target = target;
        self.time_to_go = time.max(0.0);
        if self.time_to_go <= 0.0 {
            self.weight = target;
        }
    }

    fn tick(&mut self, dt: f32) {
        if self.time_to_go > dt {
            self.weight += (self.target - self.weight) * dt / self.time_to_go;
            self.time_to_go -= dt;
        } else {
            self.weight = self.target;
            self.time_to_go = 0.0;
        }
    }
}

/// GGBicycleAbstract's balance state (EFallDirection: 0 = left, 1 = right).
#[derive(Clone, Debug)]
pub struct Balance {
    pub blend: Blend,
    pub fall_dir: u8,
    pub lost_balance: bool,
    pub balancing: bool,
}

impl Default for Balance {
    fn default() -> Self {
        Self { blend: Blend { weight: 0.5, target: 0.5, time_to_go: 0.0 }, fall_dir: 0, lost_balance: false, balancing: false }
    }
}

impl Balance {
    fn w(&self) -> f32 {
        self.blend.weight
    }

    fn lose(&mut self) {
        let target = self.fall_dir as f32;
        self.lost_balance = true;
        self.blend.set_target(target, (self.w() - target).abs() / FALL_SPEED);
    }

    pub fn did_fall(&self) -> bool {
        self.w() > FALL_THRESHOLD || self.w() < 1.0 - FALL_THRESHOLD
    }

    /// TryToBalance(dir): 0 = A (left), 1 = D (right).
    pub fn try_balance(&mut self, dir: u8) {
        if BALANCING_THRESHOLD < self.w() {
            return;
        }
        if self.fall_dir != dir {
            // balancing against the fall: back to upright, then fall the other way
            self.balancing = true;
            self.blend.set_target(0.5, (self.w() - 0.5).abs() / BALANCE_SPEED);
            self.fall_dir = (self.fall_dir + 1) % 2;
        } else if !self.balancing && self.lost_balance {
            let target = self.fall_dir as f32;
            self.blend.set_target(target, (self.w() - target).abs() / PENALTY_FALL_SPEED);
        }
    }

    pub fn finish(&mut self) {
        self.lost_balance = false;
        self.balancing = false;
    }

    /// Tick: blend, finish balancing at 0.5, start leaning again.
    pub fn tick(&mut self, dt: f32, driving: bool, jumping: bool) {
        self.blend.tick(dt);
        if driving && self.w() == 0.5 && self.balancing {
            self.finish();
        }
        if driving && !self.lost_balance && !jumping {
            self.lose();
        }
    }

    /// Upright again (JumpWithBike / leaving): blend back to 0.5 and switch the fall side.
    fn upright(&mut self, time: f32) {
        self.finish();
        self.blend.set_target(0.5, time);
        self.fall_dir = (self.fall_dir + 1) % 2;
    }
}

/// Wheel spin/steer on a skeleton bone (SkelControlWheel).
#[derive(Clone, Debug)]
struct WheelBone {
    /// Index in the vehicle controller.
    wheel: usize,
    joint: usize,
    /// Rotation of the parent bone in mesh-root space and the bone's own ref-pose local rotation.
    parent_cs: Quat,
    ref_local: Quat,
    steers: bool,
}

#[derive(Component)]
pub struct Bike {
    pub def: &'static Def,
    /// The actor's export in the map (its VM object for script Touch); None when a script spawned it.
    pub export: Option<usize>,
    /// DriverLeave refuses (the Shopping Goat mutator's cart).
    pub locked: bool,
    last_crash: f32,
    /// bPoweredWheel per controller wheel.
    powered: Vec<bool>,
    pub body: rp::RigidBodyHandle,
    pub vehicle: usize,
    pub balance: Balance,
    pub jumping: bool,
    pub pending_jump: bool,
    /// Current steer angle (radians, + = left).
    pub steer: f32,
    pub throttle: f32,
    pub driven: bool,
    /// The entity holding the bike's rig, and the saddle: SaddleSocket's bone, the socket
    /// relative to it, and the bone's ref-pose rotation in bike space.
    pivot: Entity,
    seat_bone: Option<usize>,
    seat_local: Transform,
    seat_ref_rot: Quat,
    /// Fall_L / Fall_R (the lean blend's children).
    fall_clips: (Option<usize>, Option<usize>),
    wheels: Vec<WheelBone>,
    /// Axle and up in mesh-root space.
    axle_ms: Vec3,
    up_ms: Vec3,
    prev_vel: Vec3,
    started: f32,
    pub name: String,
    /// The tire loop playing: (mTireSoundList index, audio entity).
    tire: Option<(usize, Entity)>,
    old_speed: f32,
}

/// Bikes spawned at load, and the goat's riding state.
#[derive(Resource, Default)]
pub struct Riding {
    pub bike: Option<Entity>,
    /// A bike within VehicleCheckRadius (R uses it instead of respawning).
    pub near: bool,
    /// The goat's animation on the vehicle and its rate (Biker: GlobalAnimRateScale).
    pub anim: &'static str,
    pub anim_rate: f32,
    prev_right: f32,
    prev_jump: bool,
    /// Times the goat got on / was thrown off (tests).
    pub entered: u32,
    pub thrown: u32,
    /// A script-spawned vehicle to get into once its entity exists.
    pending_mount: Option<Entity>,
}

/// GGSVehicle.TryToDrive from the VM (a script spawned the vehicle): its class, object name,
/// location (Bevy space) and mUsedByMutator.
pub struct ScriptDrive {
    pub class: String,
    pub name: String,
    pub loc: Vec3,
    pub locked: bool,
}

static VM_DRIVES: std::sync::Mutex<Vec<ScriptDrive>> = std::sync::Mutex::new(Vec::new());

pub fn vm_try_to_drive(d: ScriptDrive) {
    if let Ok(mut q) = VM_DRIVES.lock() {
        if q.len() < 8 {
            q.push(d);
        }
    }
}

/// Spawn every placed GGBicycle (package 0 of the registry).
pub fn spawn_all(reg: &Registry, cache: &mut SkelCache, a: &mut SkelAssets, commands: &mut Commands, phys: &mut Physics) -> usize {
    let map = &reg.packages[0];
    let mut n = 0;
    let first_bike = (0..map.exports.len()).find(|&i| (map.class_name(i as i32 + 1).contains("BicycleContent") || map.class_name(i as i32 + 1).contains("LongboardContent")) && map.obj_path(i as i32 + 1).contains(".PersistentLevel.")).unwrap_or(0);
    for i in 0..map.exports.len() {
        let class = map.class_name(i as i32 + 1);
        let def: &'static Def = if class.contains("Bicycle") {
            &BICYCLE
        } else if class.contains("Longboard") {
            &LONGBOARD
        } else {
            continue;
        };
        if class.contains("Wheel") || !map.obj_path(i as i32 + 1).contains(".PersistentLevel.") {
            continue;
        }
        let actor = ObjRef { pkg: 0, export: i };
        let aps = reg.merged_props(actor);
        let loc = convert::pos(crate::npc::vec3(&aps, "Location", [0.0; 3]));
        let rot = match registry::find(&aps, "Rotation") {
            Some((_, Value::Rotator(r))) => *r,
            _ => [0; 3],
        };
        // the bike stands upright (the placed roll/pitch are the parked lean)
        let xf = Transform::from_translation(loc).with_rotation(convert::rot([0, rot[1], 0]));
        if spawn_vehicle(reg, cache, a, commands, phys, def, actor, xf, map.obj_name(i as i32 + 1), Some(i), i == first_bike).is_some() {
            n += 1;
        }
    }
    n
}

/// Build one vehicle from its actor (or class default object) at `xf`.
#[allow(clippy::too_many_arguments)]
pub fn spawn_vehicle(
    reg: &Registry,
    cache: &mut SkelCache,
    a: &mut SkelAssets,
    commands: &mut Commands,
    phys: &mut Physics,
    def: &'static Def,
    actor: ObjRef,
    xf: Transform,
    name: String,
    export: Option<usize>,
    trace: bool,
) -> Option<Entity> {
    {
        let loc = xf.translation;
        let aps = reg.merged_props(actor);
        let comp = crate::npc::obj(reg, &aps, "Mesh")?;
        let cps = reg.merged_props(comp);
        let mesh = crate::npc::obj(reg, &cps, "SkeletalMesh")?;
        let anim = crate::npc::objs(reg, &cps, "AnimSets").into_iter().flatten().next();
        let pa = crate::npc::obj(reg, &cps, "PhysicsAsset");
        let data = match rig::load(reg, cache, mesh, anim, pa, a) {
            Ok(d) => d,
            Err(e) => {
                warn!("vehicle {name}: {e:#}");
                return None;
            }
        };
        let mesh_xf = Transform::from_translation(convert::pos(crate::npc::vec3(&cps, "Translation", [0.0; 3])));
        let root_xf = mesh_xf * Transform::from_rotation(convert::rot(data.rot_origin));
        // bones in mesh-root space (ref pose)
        let mut cs: Vec<Transform> = Vec::with_capacity(data.ref_pose.len());
        for (k, &p) in data.ref_pose.iter().enumerate() {
            let t = if k == 0 || data.parents[k] < 0 { p } else { cs[data.parents[k] as usize] * p };
            cs.push(t);
        }
        let bone = |name: &str| data.bone_names.iter().position(|b| b.eq_ignore_ascii_case(name));
        let entity_space = |k: usize| root_xf * cs[k];
        // chassis: the physics asset's non-wheel bodies
        let body = phys.world.insert_body(
            rp::RigidBodyBuilder::dynamic()
                .pose(rp::Pose::from_parts(convert::to_rapier(xf.translation), convert::quat_to_rapier(xf.rotation)))
                .linear_damping(0.05)
                .angular_damping(0.8)
                .ccd_enabled(true)
                .sleeping(true),
        );
        let mut n_shapes = 0;
        if let Some(def) = data.ragdoll.as_ref() {
            for (bi, b) in def.asset.bodies.iter().enumerate() {
                let lower = b.bone.to_lowercase();
                if lower.contains("tire") || lower.contains("support") || lower.contains("wheel") {
                    continue;
                }
                let Some(bk) = def.bone_of_body[bi] else { continue };
                let bt = entity_space(bk);
                let bone_pose = rp::Pose::from_parts(convert::to_rapier(bt.translation), convert::quat_to_rapier(bt.rotation));
                for (pose, shape) in crate::shapes::colliders(&b.shapes, Vec3::ONE) {
                    phys.world.insert_collider(rp::ColliderBuilder::new(shape).position(bone_pose * pose).density(1.0).friction(0.6), Some(body));
                    n_shapes += 1;
                }
            }
        }
        if std::env::var_os("GOAT_TRACE").is_some() && trace {
            if let Some(rb) = phys.world.bodies.get(body) {
                for &c in rb.colliders() {
                    let col = &phys.world.colliders[c];
                    let aabb = col.shape().compute_aabb(&col.position_wrt_parent().copied().unwrap_or_default());
                    println!("    bike chassis aabb {:.2?} .. {:.2?}", aabb.mins, aabb.maxs);
                }
            }
            for m in data.material_refs.iter().flatten() {
                let info = upk::material::resolve_in(reg, *m);
                println!("    bike material {} -> diffuse {:?}", reg.path(*m), info.diffuse.map(|d| reg.path(d)));
            }
            println!("    bike comp materials {:?}", crate::npc::objs(reg, &cps, "Materials").iter().map(|m| m.map(|m| reg.path(m))).collect::<Vec<_>>());
            println!("    bike bones: {:?}", data.bone_names.iter().enumerate().map(|(k, n)| format!("{n}{:.2?}", entity_space(k).translation)).collect::<Vec<_>>());
        }
        if n_shapes == 0 {
            phys.world.insert_collider(rp::ColliderBuilder::cuboid(0.8, 0.5, 0.15).translation(rp::Vector::new(0.0, 0.6, 0.0)).density(1.0), Some(body));
        }
        // PhysX vehicles carry their wheel shapes in the chassis actor, so the mass sits low:
        // the frame shapes only collide, the mass is placed at axle height (+ COMOffset 5 uu)
        for &c in phys.world.bodies.get(body).map(|rb| rb.colliders().to_vec()).unwrap_or_default().iter() {
            if let Some(col) = phys.world.colliders.get_mut(c) {
                col.set_density(0.0);
            }
        }
        let axle_y = bone(def.wheels[0].0).map(|k| entity_space(k).translation.y).unwrap_or(0.6);
        if let Some(rb) = phys.world.bodies.get_mut(body) {
            let com = rp::Vector::new(0.0, axle_y + 5.0 * S, 0.0);
            let m = def.mass;
            let inertia = rp::Vector::new(m * 0.3 * 0.3, m * 0.7 * 0.7, m * 0.7 * 0.7);
            rb.set_additional_mass_properties(rp::MassProperties::new(com, m, inertia), true);
        }
        // wheels: bone + BoneOffset, radius WheelRadius, travel SuspensionTravel
        let tuning = WheelTuning {
            // SuspensionTravel is only 5 uu: a stiff spring (rapier scales it by the chassis mass)
            suspension_stiffness: 200.0,
            suspension_compression: 8.5,
            suspension_damping: 14.0,
            max_suspension_travel: SUSPENSION_TRAVEL * S,
            side_friction_stiffness: 1.0,
            friction_slip: 3.0,
            max_suspension_force: 20000.0,
        };
        let mut ctrl = DynamicRayCastVehicleController::new(body);
        let mut wheels = Vec::new();
        let mut powered_wheels = Vec::new();
        let rest = SUSPENSION_TRAVEL * S;
        for &(bone_name, offset, steers, powered) in def.wheels {
            let Some(k) = bone(bone_name) else { continue };
            let centre = entity_space(k).translation + convert::pos(offset);
            if std::env::var_os("GOAT_TRACE").is_some() && trace {
                println!("    bike wheel {bone_name}: centre {centre:.2?}");
            }
            let w = ctrl.add_wheel(convert::to_rapier(centre + Vec3::Y * rest), rp::Vector::NEG_Y, rp::Vector::Z, rest, def.radius * S, &tuning);
            powered_wheels.push(powered);
            // support wheels only catch the bike when it leans: softer, low grip
            if !powered {
                w.friction_slip = 1.0;
                w.suspension_stiffness *= 0.3;
            }
            if !bone_name.starts_with("Support") {
                let parent_cs = if data.parents[k] >= 0 { cs[data.parents[k] as usize].rotation } else { Quat::IDENTITY };
                wheels.push(WheelBone { wheel: powered_wheels.len() - 1, joint: k, parent_cs, ref_local: data.ref_pose[k].rotation, steers });
            }
        }
        // SaddleSocket from the mesh's Sockets
        let (seat_bone, seat_local) = match socket(reg, mesh, def.socket).and_then(|(b, t)| bone(&b).map(|k| (k, t))) {
            Some((k, t)) => (Some(k), t),
            None => (None, Transform::default()),
        };
        let seat_ref_rot = seat_bone.map(|k| (root_xf * cs[k]).rotation).unwrap_or(Quat::IDENTITY);
        let fall_clips = (data.clip("Fall_L"), data.clip("Fall_R"));
        let axle_ms = root_xf.rotation.inverse() * Vec3::Z;
        let up_ms = root_xf.rotation.inverse() * Vec3::Y;
        phys.vehicles.push(ctrl);
        phys.names.insert(body, name.clone());
        phys.prop_info.insert(body, (10, def.score_name.into()));
        let e = commands.spawn((xf, Visibility::default(), Name::new(name.clone()), PhysBody { handle: body, scale: Vec3::ONE })).id();
        let pivot = commands.spawn((Transform::default(), Visibility::default())).id();
        commands.entity(e).add_child(pivot);
        rig::spawn(commands, pivot, data.clone(), mesh_xf, &[], "Fall_L");
        commands.entity(e).insert(Bike {
            def,
            export,
            locked: false,
            last_crash: 0.0,
            powered: powered_wheels,
            body,
            vehicle: phys.vehicles.len() - 1,
            balance: Balance::default(),
            jumping: false,
            pending_jump: false,
            steer: 0.0,
            throttle: 0.0,
            driven: false,
            pivot,
            seat_bone,
            seat_local,
            seat_ref_rot,
            fall_clips,
            wheels,
            axle_ms,
            up_ms,
            prev_vel: Vec3::ZERO,
            started: 0.0,
            name: name.clone(),
            tire: None,
            old_speed: 0.0,
        });
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    bike {name} at {loc:?}: {} chassis shapes, mass {:.0}, {} clips {:?}", n_shapes, def.mass, data.clips.len(), data.by_name.keys().collect::<Vec<_>>());
        }
        Some(e)
    }
}

/// GetInVehicle: the goat takes the vehicle (its capsule off; the rider pose follows the seat).
fn enter(riding: &mut Riding, b: &mut Bike, g: &mut Goat, phys: &mut Physics, e: Entity, now: f32) {
    b.driven = true;
    b.balance = Balance::default();
    b.started = now;
    if let Some(rb) = phys.world.bodies.get_mut(b.body) {
        rb.wake_up(true);
    }
    if let Some(c) = phys.world.colliders.get_mut(g.collider) {
        c.set_enabled(false);
    }
    riding.bike = Some(e);
    riding.entered += 1;
    g.velocity = Vec3::ZERO;
}

/// Vehicles scripts spawn and drive (GGMutatorComponentShoppingGoat: Spawn(GGShoppingCart) then
/// TryToDrive(goat)): built from the class default object next to the goat, then entered.
#[allow(clippy::too_many_arguments)]
fn script_vehicles(
    mut commands: Commands,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut cache: Local<SkelCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut phys: ResMut<Physics>,
    mut riding: ResMut<Riding>,
    goats: Query<(&Goat, &Transform)>,
    bikes: Query<(Entity, &Bike)>,
) {
    // after the goat has landed at its start (the mutator runs before it is placed)
    let Ok((g, gt)) = goats.single() else { return };
    if !g.grounded {
        return;
    }
    let drives = match VM_DRIVES.lock() {
        Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
        _ => return,
    };
    let Some(game) = game else { return };
    let reg = &game.0;
    let mut a = SkelAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes };
    for d in drives {
        if let Some((e, _)) = bikes.iter().find(|(_, b)| b.export.is_none() && b.name == d.name) {
            riding.pending_mount = Some(e);
            continue;
        }
        let (def, default_obj): (&'static Def, &str) = if d.class.contains("ShoppingCart") {
            (&SHOPPING_CART, "Default__GGShoppingCart")
        } else {
            info!("TryToDrive on {} ({}): no vehicle for that class", d.name, d.class);
            continue;
        };
        let Some(actor) = reg.find_path(default_obj) else {
            warn!("{default_obj} not found");
            continue;
        };
        // at the goat (Spawn(.., user.Location + offset); the VM's copy of the goat's Location can
        // lag at map start), facing its way, dropped onto the ground under it
        let _ = d.loc;
        let ray = rp::Ray::new(convert::to_rapier(gt.translation + Vec3::Y * 0.5), rp::Vector::NEG_Y);
        let ground = phys.world.cast_ray(&ray, 30.0, true, rp::QueryFilter::exclude_dynamic().exclude_rigid_body(g.body).exclude_collider(g.collider)).map(|(_, toi)| gt.translation.y + 0.5 - toi).unwrap_or(gt.translation.y);
        let at = gt.translation.with_y(ground + 1.6);
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    cart spawn: goat {:.2?}, ground {ground:.2}", gt.translation);
        }
        let xf = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(g.yaw + std::f32::consts::FRAC_PI_2));
        let Some(e) = spawn_vehicle(reg, &mut cache, &mut a, &mut commands, &mut phys, def, actor, xf, d.name.clone(), None, true) else { continue };
        if d.locked {
            commands.entity(e).queue(|mut w: EntityWorldMut| {
                if let Some(mut b) = w.get_mut::<Bike>() {
                    b.locked = true;
                }
            });
        }
        info!("{} spawned by script; the goat gets in", d.name);
        riding.pending_mount = Some(e);
    }
}

/// bDriverIsVisible: the goat is hidden in a vehicle whose mesh has its own.
fn driver_visibility(riding: Res<Riding>, bikes: Query<&Bike>, mut goats: Query<&mut Visibility, With<Goat>>, mut hid: Local<bool>) {
    let hide = riding.bike.and_then(|e| bikes.get(e).ok()).is_some_and(|b| !b.def.driver_visible);
    if hide == *hid {
        return;
    }
    *hid = hide;
    for mut v in &mut goats {
        *v = if hide { Visibility::Hidden } else { Visibility::Inherited };
    }
}

/// A SkeletalMeshSocket's bone and transform relative to it.
pub(crate) fn socket(reg: &Registry, mesh: ObjRef, name: &str) -> Option<(String, Transform)> {
    let mps = reg.merged_props(mesh);
    for s in crate::npc::objs(reg, &mps, "Sockets").into_iter().flatten() {
        let sp = reg.merged_props(s);
        let sname = match registry::find(&sp, "SocketName") {
            Some((_, Value::Name(n))) => n.clone(),
            _ => continue,
        };
        if !sname.eq_ignore_ascii_case(name) {
            continue;
        }
        let bone = match registry::find(&sp, "BoneName") {
            Some((_, Value::Name(n))) => n.clone(),
            _ => continue,
        };
        let loc = crate::npc::vec3(&sp, "RelativeLocation", [0.0; 3]);
        let rot = match registry::find(&sp, "RelativeRotation") {
            Some((_, Value::Rotator(r))) => *r,
            _ => [0; 3],
        };
        return Some((bone, Transform::from_translation(convert::pos(loc)).with_rotation(convert::rot(rot))));
    }
    None
}

/// The goat's pose on the saddle: pawn at the socket (leaning with the saddle bone), mesh at
/// mDriverPosOffset (instead of the mesh's own Translation (0, 0, 8)), facing along the bike.
fn seat_pose(bike: &GlobalTransform, b: &Bike, saddle: Option<&GlobalTransform>) -> (Vec3, Quat) {
    let bx = bike.compute_transform();
    let (seat, lean) = match saddle {
        Some(gt) => {
            let bone = gt.compute_transform();
            (bone * b.seat_local, bone.rotation * (bx.rotation * b.seat_ref_rot).inverse())
        }
        None => (bx * Transform::from_xyz(-0.2, 1.0, 0.0), Quat::IDENTITY),
    };
    let rot = lean * bx.rotation * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
    let o = b.def.driver_offset;
    let off = Vec3::new(-o[1], o[2] - 8.0, -o[0]) * S;
    (seat.translation + rot * off, rot)
}

/// Use (R), driving, balance and the rider (Update, before goat_move).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn ride(
    time: Res<Time>,
    input: Res<InputState>,
    mut riding: ResMut<Riding>,
    mut phys: ResMut<Physics>,
    mut commands: Commands,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut score: ResMut<crate::score::Score>,
    mut bikes: Query<(Entity, &mut Bike, &GlobalTransform)>,
    rigs: Query<&rig::Rig>,
    joint_gt: Query<&GlobalTransform, Without<Bike>>,
    mut goats: Query<(Entity, &mut Goat, &mut Transform), Without<Bike>>,
    cam: Query<&crate::goat::GoatCamera>,
    game: Option<Res<crate::uvm::actors::GameRegistry>>,
    mut sfx: Option<ResMut<crate::audio::Sfx>>,
    mut audio: ResMut<Assets<AudioSource>>,
) {
    let _prof_scope = crate::prof::scope("vehicle::ride");
    let dt = time.delta_secs().min(0.05);
    let Ok((ge, mut g, mut gt)) = goats.single_mut() else { return };
    // cues come from the map's packages (loaded the first time they play)
    let mut cue = |path: &str, sfx: &mut Option<ResMut<crate::audio::Sfx>>| -> Option<Handle<AudioSource>> {
        let sfx = sfx.as_deref_mut()?;
        if !sfx.cues.contains_key(path) {
            let gr = game.as_ref()?;
            crate::audio::load_cue(&gr.0, sfx, &mut audio, path, path);
        }
        sfx.cues.get(path).and_then(|l| l.first()).cloned()
    };
    let inp = &input.0;
    let now = time.elapsed_secs();
    // FindVehicleToDrive: the bike in range the goat faces most
    let goat_fwd = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
    let near = if riding.bike.is_none() && !g.ragdoll {
        bikes
            .iter()
            .filter(|(_, _, t)| t.translation().distance(gt.translation) <= VEHICLE_CHECK_RADIUS * S)
            .max_by(|a, b| {
                let d = |t: &GlobalTransform| (t.translation() - gt.translation).with_y(0.0).normalize_or_zero().dot(goat_fwd);
                d(a.2).total_cmp(&d(b.2))
            })
            .map(|(e, _, _)| e)
    } else {
        None
    };
    riding.near = near.is_some();
    // a script's TryToDrive (script_vehicles), once the vehicle's entity exists
    if let Some(e) = riding.pending_mount.take() {
        if riding.bike.is_none() && !g.ragdoll {
            match bikes.get_mut(e) {
                Ok((_, mut b, _)) => enter(&mut riding, &mut b, &mut g, &mut phys, e, now),
                Err(_) => riding.pending_mount = Some(e),
            }
        }
    }
    if inp.special {
        if let Some(e) = riding.bike {
            // DriverLeave: the mutator's cart refuses
            if !bikes.get(e).is_ok_and(|(_, b, _)| b.locked) {
                leave(&mut riding, &mut g, &mut gt, &mut phys, &mut bikes, e, false, ge, &mut commands);
                return;
            }
        } else if let Some(e) = near {
            // TryToDrive -> DriverEnter -> GetInVechile
            if let Ok((_, mut b, _)) = bikes.get_mut(e) {
                enter(&mut riding, &mut b, &mut g, &mut phys, e, now);
            }
        }
    }
    let Some(e) = riding.bike else { return };
    let Ok((_, mut b, bt)) = bikes.get_mut(e) else {
        riding.bike = None;
        return;
    };
    let b = &mut *b;
    // a ragdoll from anything else (explosion, Q) throws the goat off
    if g.ragdoll {
        riding.bike = None;
        b.driven = false;
        b.balance.upright(0.8);
        if let Some(c) = phys.world.colliders.get_mut(g.collider) {
            c.set_enabled(true);
        }
        return;
    }
    let Some(rb) = phys.world.bodies.get(b.body) else { return };
    let rot = convert::quat_from_rapier(*rb.rotation());
    let vel = convert::from_rapier(rb.linvel());
    let fwd = rot * Vec3::X;
    let up = rot * Vec3::Y;
    let forward_vel = vel.dot(fwd) / S;
    let ctrl = &phys.vehicles[b.vehicle];
    let on_ground = ctrl.wheels().iter().take(2).any(|w| w.raycast_info().is_in_contact);
    // SetJumping
    if b.pending_jump && !on_ground {
        b.jumping = true;
        b.pending_jump = false;
    }
    if b.jumping && on_ground {
        b.jumping = false;
        // SetJumping: PlaySound(mJumpSound) on landing
        if cue(b.def.jump_cue, &mut sfx).is_some() {
            sounds.play(b.def.jump_cue, Some(bt.translation()));
        }
    }
    // KeyState: A/D presses (balance on the ground, spin in the air), jump
    let pressed = |v: f32, prev: f32| v != 0.0 && (prev == 0.0 || v.signum() != prev.signum());
    let mut torque = Vec3::ZERO;
    if pressed(inp.right, riding.prev_right) && b.def.balance {
        let dir = if inp.right < 0.0 { 0 } else { 1 };
        if b.jumping {
            // JumpFlipLeft/Right: AddTorque(+-Z x mSideFlipTorqueMag) (world up)
            torque = Vec3::Y * if dir == 0 { 1.0 } else { -1.0 } * SIDE_FLIP_TORQUE;
        } else {
            b.balance.try_balance(dir);
        }
    }
    riding.prev_right = inp.right;
    let mut jump = false;
    // KeyState KS_Down: one jump per press
    let jump_pressed = inp.jump && !riding.prev_jump;
    riding.prev_jump = inp.jump;
    if jump_pressed && !b.jumping && on_ground {
        // JumpWithBike: world-up impulse
        b.balance.upright(0.05);
        b.pending_jump = true;
        jump = true;
        if let Some(c) = b.def.jump_start_cue.filter(|c| cue(c, &mut sfx).is_some()) {
            sounds.play(c, Some(bt.translation()));
        }
    }
    if b.def.balance {
        b.balance.tick(dt, true, b.jumping || b.pending_jump);
    }
    // pedals / brake (Throttle; S brakes while rolling forward, then reverses)
    b.throttle = inp.forward;
    let peak = b.def.torque.iter().map(|p| p.1).fold(0.0, f32::max);
    let force = |v: f32| eval(b.def.torque, v) / peak * PEDAL_ACCEL * b.def.mass;
    let (engine, brake) = if inp.forward > 0.0 {
        (force(forward_vel) * inp.forward, 0.0)
    } else if inp.forward < 0.0 && forward_vel > 50.0 {
        (0.0, BRAKE)
    } else if inp.forward < 0.0 {
        (-force(-forward_vel), 0.0)
    } else {
        (0.0, BRAKE * ENGINE_BRAKE_FACTOR * 10.0)
    };
    // mSteerWithCamera: steer toward the camera's heading
    let cam_yaw = cam.single().map(|c| c.yaw).unwrap_or(g.yaw);
    let cam_fwd = Quat::from_rotation_y(cam_yaw) * -Vec3::Z;
    let flat = fwd.with_y(0.0).normalize_or_zero();
    let max = eval(b.def.steer, forward_vel.abs()).to_radians();
    let target = if b.def.steer_with_camera {
        let want = flat.cross(cam_fwd).y.atan2(flat.dot(cam_fwd));
        want.clamp(-max, max) * if forward_vel < -10.0 { -1.0 } else { 1.0 }
    } else {
        // Steering input (A/D)
        -inp.right * max
    };
    let step = b.def.steer_speed.to_radians() * dt;
    b.steer += (target - b.steer).clamp(-step, step);
    if std::env::var_os("GOAT_TRACE").is_some() && (now * 4.0).floor() != ((now - dt) * 4.0).floor() {
        let rb = &phys.world.bodies[b.body];
        let ws: Vec<String> = phys.vehicles[b.vehicle]
            .wheels()
            .iter()
            .map(|w| format!("{}{:.2}", if w.raycast_info().is_in_contact { "c" } else { "-" }, w.raycast_info().suspension_length))
            .collect();
        println!(
            "    ride: pos {:.2?} vel {:.1} uu/s fwd {forward_vel:.0} mass {:.0} sleeping {} engine {engine:.0} brake {brake:.1} steer {:.2} wheels {ws:?} lean {:.2}",
            convert::from_rapier(rb.translation()),
            vel.length() / S,
            rb.mass(),
            rb.is_sleeping(),
            b.steer,
            b.balance.w()
        );
    }
    let ctrl = &mut phys.vehicles[b.vehicle];
    let n_powered = b.powered.iter().filter(|p| **p).count().max(1) as f32;
    let steers: Vec<bool> = b.def.wheels.iter().map(|w| w.2).collect();
    for (k, w) in ctrl.wheels_mut().iter_mut().enumerate() {
        // bPoweredWheel wheels share the pedal force (the bike's support wheels are unpowered)
        w.engine_force = if b.powered.get(k) == Some(&true) { engine / n_powered } else { 0.0 };
        w.brake = brake;
        w.steering = if steers.get(k) == Some(&true) { b.steer } else { 0.0 };
    }
    if let Some(rb) = phys.world.bodies.get_mut(b.body) {
        if jump {
            rb.apply_impulse(rp::Vector::Y * (b.def.jump_dv * S * rb.mass()), true);
        }
        if torque != Vec3::ZERO {
            rb.apply_torque_impulse(convert::to_rapier(torque / PHYSX_MASS * S * S * rb.mass()), true);
        }
        // RB_StayUpright: keep the frame upright while ridden on the ground (the lean is the
        // animation): the roll rate is replaced by one that brings the frame back level
        if on_ground {
            let w = convert::from_rapier(rb.angvel());
            let roll_rate = 6.0 * (rot * Vec3::Z).y;
            rb.set_angvel(convert::to_rapier(w - fwd * w.dot(fwd) + fwd * roll_rate), true);
        }
    }
    // AdaptBikerSpeed
    riding.anim = b.def.anim;
    riding.anim_rate = if b.def.balance {
        let ratio = (forward_vel / 10.0).trunc();
        if ratio != 0.0 { if b.throttle != 0.0 { ratio * BIKER_SPEED_FACTOR / 10.0 } else { 0.01 } } else { 0.0 }
    } else {
        1.0
    };
    // Fell / Crash / CheckIfTippedOverByPhysics
    let hit_vel = (b.prev_vel - vel).length() / S;
    let roll = up.dot(rot * Vec3::Z).asin().abs().max(if up.y < 0.0 { std::f32::consts::PI } else { 0.0 });
    // GGLongboardAbstract.CheckIfTippedOver only below mTripVelocityThreshold
    let tipped = roll > b.def.roll_threshold && b.def.trip_velocity.is_none_or(|v| vel.length() / S < v);
    let crashed = b.def.balance && hit_vel >= COLLISION_VELOCITY_THRESHOLD;
    let thrown = (b.def.balance && b.balance.did_fall()) || crashed || tipped;
    // GGSVehicle.RigidBodyCollision: CollisionSound past mCollisionVelocityThreshold
    if b.def.tire_rule == TireRule::Cart && hit_vel >= VEHICLE_COLLISION_VELOCITY && now - b.last_crash > MIN_TIME_BETWEEN_COLLISIONS && cue(b.def.crash_cue, &mut sfx).is_some() {
        b.last_crash = now;
        sounds.play(b.def.crash_cue, Some(bt.translation()));
    }
    b.prev_vel = vel;
    // SetTireSound: pedalling / coasting / braking (bike), accelerating / braking / rolling (board)
    let speed = forward_vel.abs();
    let want = if !on_ground || thrown {
        None
    } else if b.def.tire_rule == TireRule::Cart {
        Some(if speed < 400.0 { 1 } else { 0 })
    } else if b.def.tire_rule == TireRule::Bike {
        let braking = b.throttle < 0.0 && forward_vel > 0.0;
        if braking {
            Some(2)
        } else if (b.throttle == 0.0 && speed > 60.0) || b.jumping {
            Some(1)
        } else if b.throttle != 0.0 && speed > 10.0 {
            Some(0)
        } else {
            b.tire.map(|t| t.0)
        }
    } else {
        let accelerating = b.throttle != 0.0 && speed - b.old_speed > 2.0 * dt * 60.0 && speed < 900.0;
        let braking = b.throttle == 0.0 || (b.throttle < 0.0 && forward_vel > 0.0) || (b.throttle > 0.0 && forward_vel < 0.0);
        if accelerating {
            Some(0)
        } else if braking && speed > 0.0 {
            Some(1)
        } else {
            Some(2)
        }
    };
    b.old_speed = speed;
    if want.filter(|_| speed > 10.0) != b.tire.map(|t| t.0) {
        if let Some((_, e)) = b.tire.take() {
            commands.entity(e).despawn();
        }
        let muted = sfx.as_ref().is_none_or(|s| s.muted);
        if let (Some(i), false) = (want.filter(|_| speed > 10.0), muted) {
            if let Some(h) = cue(b.def.tire_cues[i], &mut sfx) {
                let vol = crate::uvm::settings::applied().map(|a| a.sfx_volume).unwrap_or(1.0) * 0.7;
                let a = commands.spawn((AudioPlayer::new(h), PlaybackSettings::LOOP.with_volume(bevy::audio::Volume::Linear(vol)), Name::new("tire sound"))).id();
                b.tire = Some((i, a));
            }
        }
    }
    if thrown {
        if let Some((_, e)) = b.tire.take() {
            commands.entity(e).despawn();
        }
        if crashed && cue(b.def.crash_cue, &mut sfx).is_some() {
            sounds.play(b.def.crash_cue, Some(bt.translation()));
        }
        let why = if tipped { "tipped over" } else if crashed { "crashed" } else { "fell" };
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    bike {}: {why} (lean {:.2}, hit {hit_vel:.0} uu/s, roll {roll:.2})", b.name, b.balance.w());
        }
        riding.thrown += 1;
        leave(&mut riding, &mut g, &mut gt, &mut phys, &mut bikes, e, true, ge, &mut commands);
        return;
    }
    // the rider and the lean
    {
        let saddle = b.seat_bone.and_then(|k| rigs.get(b.pivot).ok().and_then(|r| r.joints.get(k).copied())).and_then(|j| joint_gt.get(j).ok());
        let (pos, r) = seat_pose(bt, b, saddle);
        gt.translation = pos;
        gt.rotation = r;
        let (yaw, _, _) = r.to_euler(EulerRot::YXZ);
        g.yaw = yaw;
    }
    g.velocity = vel;
    g.grounded = true;
    if let Some(grb) = phys.world.bodies.get_mut(g.body) {
        grb.set_translation(convert::to_rapier(gt.translation), true);
    }
    let _ = &mut score;
}

/// DriverLeave: off the bike beside it (or thrown off into a ragdoll).
#[allow(clippy::too_many_arguments)]
fn leave(
    riding: &mut Riding,
    g: &mut Goat,
    gt: &mut Transform,
    phys: &mut Physics,
    bikes: &mut Query<(Entity, &mut Bike, &GlobalTransform)>,
    e: Entity,
    thrown: bool,
    ge: Entity,
    commands: &mut Commands,
) {
    riding.bike = None;
    let Ok((_, mut b, bt)) = bikes.get_mut(e) else { return };
    b.driven = false;
    b.throttle = 0.0;
    if let Some((_, e)) = b.tire.take() {
        commands.entity(e).despawn();
    }
    // GetOutOfVehicle: upright the bike again
    b.balance.upright(0.8);
    for w in phys.vehicles[b.vehicle].wheels_mut() {
        w.engine_force = 0.0;
        w.brake = BRAKE;
        w.steering = 0.0;
    }
    let vel = phys.world.bodies.get(b.body).map(|rb| convert::from_rapier(rb.linvel())).unwrap_or_default();
    let (_, rot, pos) = bt.to_scale_rotation_translation();
    // GetExitRotation: the bike's yaw; out to the side
    let side = rot * Vec3::Z;
    let out = pos + side * 1.0 + Vec3::Y * 0.9;
    gt.translation = if thrown { gt.translation + Vec3::Y * 0.3 } else { out };
    if let Some(rb) = phys.world.bodies.get_mut(g.body) {
        rb.set_translation(convert::to_rapier(gt.translation), true);
    }
    if let Some(c) = phys.world.colliders.get_mut(g.collider) {
        c.set_enabled(true);
    }
    g.velocity = if thrown { vel + Vec3::Y * 2.0 } else { Vec3::ZERO };
    g.grounded = false;
    if thrown {
        crate::goat::start_ragdoll(commands, ge, g, phys);
    }
}

/// The bike's pose: mAnimBlendNode between Fall_L and Fall_R by the balance weight (0.5 =
/// upright), then SkelControlWheel spinning and steering the tire bones.
pub fn bike_pose(phys: Res<Physics>, bikes: Query<&Bike>, rigs: Query<&rig::Rig>, mut joints: Query<&mut Transform>) {
    let _prof_scope = crate::prof::scope("vehicle::bike_pose");
    for b in &bikes {
        let Ok(r) = rigs.get(b.pivot) else { continue };
        if let (Some(cl), Some(cr)) = b.fall_clips {
            let d = &r.data;
            let mut left = d.ref_pose.clone();
            let mut right = d.ref_pose.clone();
            d.clips[cl].sample_into(d.clips[cl].length, false, &mut left);
            d.clips[cr].sample_into(d.clips[cr].length, false, &mut right);
            let w = b.balance.blend.weight;
            for (k, &j) in r.joints.iter().enumerate() {
                if let Ok(mut t) = joints.get_mut(j) {
                    t.translation = left[k].translation.lerp(right[k].translation, w);
                    t.rotation = left[k].rotation.slerp(right[k].rotation, w);
                }
            }
        }
        let wheels = phys.vehicles[b.vehicle].wheels();
        for wb in &b.wheels {
            let Some(w) = wheels.get(wb.wheel) else { continue };
            let Some(&j) = r.joints.get(wb.joint) else { continue };
            let Ok(mut t) = joints.get_mut(j) else { continue };
            let mut spin = Quat::from_axis_angle(b.axle_ms, -w.rotation);
            if wb.steers {
                spin = Quat::from_axis_angle(b.up_ms, w.steering) * spin;
            }
            t.rotation = wb.parent_cs.inverse() * spin * wb.parent_cs * wb.ref_local;
        }
    }
}

/// GOAT_TRACE: the parked bikes' state now and then.
fn trace_bikes(time: Res<Time>, phys: Res<Physics>, bikes: Query<&Bike>) {
    if std::env::var_os("GOAT_TRACE").is_none() {
        return;
    }
    let now = time.elapsed_secs();
    if (now * 2.0).floor() == ((now - time.delta_secs()) * 2.0).floor() {
        return;
    }
    for b in &bikes {
        let Some(rb) = phys.world.bodies.get(b.body) else { continue };
        let up = convert::quat_from_rapier(*rb.rotation()) * Vec3::Y;
        let ws: String = phys.vehicles[b.vehicle].wheels().iter().map(|w| if w.raycast_info().is_in_contact { 'c' } else { '-' }).collect();
        println!("    t={now:.1} {} up.y {:.2} pos {:.2?} sleeping {} wheels {ws}", b.name, up.y, convert::from_rapier(rb.translation()), rb.is_sleeping());
    }
}

pub struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Riding>()
            .add_systems(Update, (script_vehicles, ride, driver_visibility).chain().after(crate::goat::read_input).before(crate::goat::goat_move))
            .add_systems(PostUpdate, bike_pose.before(TransformSystems::Propagate))
            .add_systems(Update, trace_bikes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_interpolate() {
        assert_eq!(eval(BICYCLE.torque, 0.0), 170.0);
        assert_eq!(eval(BICYCLE.torque, 5000.0), 0.0);
        assert!((eval(BICYCLE.steer, 400.0) - 37.5).abs() < 1e-3);
    }

    #[test]
    fn balance_falls_unless_corrected() {
        let mut b = Balance::default();
        let mut t = 0.0;
        while !b.did_fall() && t < 10.0 {
            b.tick(1.0 / 60.0, true, false);
            t += 1.0 / 60.0;
        }
        // 0.36 of blend at 0.17/s
        assert!((t - 0.36 / FALL_SPEED).abs() < 0.1, "fell after {t}");
        let mut b = Balance::default();
        for i in 0..1200 {
            if i % 30 == 0 {
                // press against the fall side
                let dir = (b.fall_dir + 1) % 2;
                b.try_balance(dir);
            }
            b.tick(1.0 / 60.0, true, false);
            assert!(!b.did_fall(), "fell at frame {i}");
        }
    }
}
