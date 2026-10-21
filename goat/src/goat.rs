//! The player goat: movement, jump/double jump, sprint, headbutt, lick-grab and ragdoll,
//! tuned with the values from GoatGame.u (Default__GGGoat and its abilities).

use crate::convert::{self, S};
use crate::physics::Physics;
use crate::ragdoll::{ActiveRagdoll, StartRagdoll, StopRagdoll};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::window::{CursorGrabMode, CursorOptions, PrimaryWindow};
use rapier3d::control::{CharacterAutostep, CharacterLength, KinematicCharacterController};
use rapier3d::prelude as rp;

/// Values copied from Default__GGGoat / GGAbility* defaults (Unreal units).
pub mod tune {
    pub const WALK_SPEED: f32 = 500.0;
    pub const SPRINT_SPEED: f32 = 1200.0;
    /// GGGoat.mHitWallInterval / mHitRigidBodyInterval.
    pub const HIT_WALL_INTERVAL: f32 = 1.25;
    pub const HIT_RIGID_BODY_INTERVAL: f32 = 1.25;
    /// GGGoat.mRagdollCollisionSpeed = walk + (sprint - walk) / 2.
    pub const RAGDOLL_COLLISION_SPEED: f32 = WALK_SPEED + (SPRINT_SPEED - WALK_SPEED) * 0.5;
    pub const REVERSE_SPEED: f32 = 200.0;
    /// Pawn.mPawnTurnInterpSpeed.
    pub const TURN_INTERP: f32 = 5.0;
    /// PhysicsVolume.GroundFriction (8) x GGPawnBase.mFrictionMultiplier (1).
    pub const GROUND_FRICTION: f32 = 8.0;
    /// GGGoat.GroundSpeed (the default pawn speed the air-accel limit compares against).
    pub const GROUND_SPEED: f32 = 500.0;
    pub const STRAFE_SPEED: f32 = 250.0;
    pub const WALK_ACCEL: f32 = 1175.0;
    pub const DECEL_INTERP: f32 = 10.0;
    pub const JUMP_Z: f32 = 650.0;
    pub const MULTI_JUMP_BOOST: f32 = 100.0;
    pub const DOUBLE_JUMP_THRESHOLD: f32 = 500.0;
    pub const AIR_CONTROL: f32 = 0.4;
    pub const COLLISION_RADIUS: f32 = 25.0;
    pub const COLLISION_HEIGHT: f32 = 30.0;
    pub const MASS: f32 = 45.0;
    pub const MAX_STEP: f32 = 20.0;
    pub const WALKABLE_FLOOR_Z: f32 = 0.6;
    pub const CAMERA_LOOKAT_Z: f32 = 50.0;
    pub const HEADBUTT_RANGE: f32 = 50.0;
    pub const HEADBUTT_IMPULSE: f32 = 70000.0;
    pub const KICK_RANGE: f32 = 60.0;
    /// Default__GGAbilityHorn.mDamage
    pub const HEADBUTT_DAMAGE: i32 = 200;

    pub const KICK_IMPULSE: f32 = 120000.0;
    pub const LICK_RANGE: f32 = 100.0;
    pub const GRABBER_STIFFNESS: f32 = 1300.0;
    pub const MIN_RAGDOLL_TIME: f32 = 1.0;
    pub const RAGDOLL_JUMP_Z: f32 = 650.0;
}

#[derive(Component)]
pub struct Goat {
    /// Last HitWall impact sound (mLastHitWallTime).
    pub last_hit_wall: f32,
    /// GGPawnBase.mIsBraking (set by the native CalcVelocity) and whether the Braking event
    /// started the brake slide (Brake animation + Goat_Slide_Loop_Cue).
    pub braking: bool,
    pub brake_slide: bool,
    /// Last ragdoll impact sound (mHitRigidBodyTime) and the pelvis velocity a frame ago.
    pub last_rb_hit: f32,
    pub ragdoll_prev_vel: Vec3,
    pub body: rp::RigidBodyHandle,
    pub collider: rp::ColliderHandle,
    pub velocity: Vec3,
    pub yaw: f32,
    pub grounded: bool,
    pub ground_speed_forward: f32,
    pub sprinting: bool,
    pub double_jump_ready: bool,
    pub multi_jump_remaining: i32,
    pub ragdoll: bool,
    pub ragdoll_time: f32,
    pub ability_cooldown: f32,
    pub grabbed: Option<(rp::RigidBodyHandle, rp::ImpulseJointHandle)>,
    pub spawn: Transform,
    pub stat_headbutts: u32,
    pub pending_respawn: bool,
    /// Time of a jump press while falling (perfect-landing attempt).
    pub tried_landing_at: Option<f32>,
    /// Air pitch (radians, nose up positive) from free-look flips.
    pub pitch: f32,
    /// mPawnRotInterpSpeed while flipping.
    pub rot_interp: f32,
    /// Rotation accumulated in the air for GGTrickRotation (degrees, signed).
    pub air_yaw_deg: f32,
    pub air_pitch_deg: f32,
    /// Pawn.mAirFlapTime (seconds, game time) of the last bird flap.
    pub flap_time: f32,
    /// mJumpOverActors.
    pub jump_over: Vec<crate::air::Below>,
    pub ready_to_wall_run: bool,
    pub wall: crate::air::WallRun,
    /// Gravity / current / ladder from the volumes around the goat (physics::volumes::environment).
    pub env: crate::physics::volumes::GoatEnv,
    /// The manual (balance on the hind legs).
    pub manual: crate::manual::Manual,
}

#[derive(Component)]
pub struct GoatCamera {
    /// mDesiredRotation (yaw, pitch; negative pitch looks down).
    pub yaw: f32,
    pub pitch: f32,
    /// mCurrentZoomDistance (metres); lerps toward `zoom_desired`.
    pub distance: f32,
    /// mDesiredZoomDistance (metres).
    pub zoom_desired: f32,
    /// mTargetLocation / mTargetRagdollOffset / mPreviousIsRagdoll.
    pub target: Vec3,
    pub ragdoll_offset: Vec3,
    pub was_ragdoll: bool,
    /// mCurrentBounceValue (radians).
    pub bounce: f32,
}

impl GoatCamera {
    /// GGCameraModeOrbital defaults: zoom 420, pitch -3072 (OnBecomeActive).
    pub fn new(yaw: f32, at: Vec3) -> Self {
        let pitch = -3072.0 / 65536.0 * std::f32::consts::TAU;
        Self { yaw, pitch, distance: uu(cam::ZOOM), zoom_desired: uu(cam::ZOOM), target: at, ragdoll_offset: Vec3::ZERO, was_ragdoll: false, bounce: 0.0 }
    }
}

/// GGCameraModeOrbital / GGCameraMode defaults.
pub mod cam {
    pub const ZOOM: f32 = 420.0;
    pub const MIN_ZOOM: f32 = 20.0;
    pub const MAX_ZOOM: f32 = 1500.0;
    pub const ZOOM_UNIT: f32 = 50.0;
    pub const WALK_BOUNCE: f32 = 2.0;
    pub const SPRINT_BOUNCE: f32 = 3.0;
    /// GGEngine settings mFOV (DefaultSettings.ini): horizontal, like every UE3 FOV.
    pub const FOV_DEG: f32 = 70.0;
}

/// Scripted input for automated tests (replaces keyboard when present).
#[derive(Resource, Default, Clone)]
pub struct ScriptedInput {
    pub forward: f32,
    pub right: f32,
    pub sprint: bool,
    pub jump: bool,
    pub headbutt: bool,
    pub lick: bool,
    pub ragdoll: bool,
    pub baa: bool,
    pub slomo: bool,
    /// Right mouse button: free look (spins/flips in the air).
    pub freelook: bool,
    /// R: GBA_Special -> Use (get on / off a vehicle).
    pub special: bool,
    pub cam_yaw: Option<f32>,
    pub cam_dist: Option<f32>,
    pub cam_pitch: Option<f32>,
}

#[derive(Resource, Default)]
pub struct InputState(pub ScriptedInput);

/// Cap on prop speed right after an impulse (metres/second).
const MAX_LAUNCH_SPEED: f32 = 30.0;

/// Default__Pawn.LadderSpeed (uu/s).
const LADDER_SPEED: f32 = 200.0;

fn uu(v: f32) -> f32 {
    v * S
}

pub fn spawn_goat(commands: &mut Commands, physics: &mut Physics, at: Transform) -> Entity {
    let half = uu(tune::COLLISION_HEIGHT);
    let r = uu(tune::COLLISION_RADIUS);
    let pos = at.translation + Vec3::Y * (half + 0.05);
    let body = physics.world.insert_body(rp::RigidBodyBuilder::kinematic_position_based().translation(convert::to_rapier(pos)));
    let collider = physics.world.insert_collider(
        rp::ColliderBuilder::capsule_y((half - r).max(0.01), r)
            .mass(tune::MASS)
            .collision_groups(rp::InteractionGroups::new(crate::ragdoll::GROUP_WORLD | crate::ragdoll::GROUP_GOAT, rp::Group::ALL, rp::InteractionTestMode::And)),
        Some(body),
    );
    let (yaw, _, _) = at.rotation.to_euler(EulerRot::YXZ);
    commands
        .spawn((
            Goat {
                body,
                collider,
                velocity: Vec3::ZERO,
                yaw,
                grounded: false,
                ground_speed_forward: tune::WALK_SPEED,
                sprinting: false,
                double_jump_ready: false,
                multi_jump_remaining: 1,
                ragdoll: false,
                ragdoll_time: 0.0,
                ability_cooldown: 0.0,
                grabbed: None,
                spawn: at,
                stat_headbutts: 0,
                pending_respawn: false,
                last_hit_wall: -10.0,
                braking: false,
                brake_slide: false,
                last_rb_hit: -10.0,
                ragdoll_prev_vel: Vec3::ZERO,
                tried_landing_at: None,
                pitch: 0.0,
                rot_interp: crate::air::ROT_INTERP_SPEED,
                air_yaw_deg: 0.0,
                air_pitch_deg: 0.0,
                flap_time: -10.0,
                jump_over: Vec::new(),
                ready_to_wall_run: false,
                wall: Default::default(),
                env: Default::default(),
                manual: crate::manual::Manual { pressed_down: f32::MIN, ..Default::default() },
            },
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(yaw)),
            Visibility::default(),
            Name::new("Goat"),
            crate::ragdoll::CharacterBody { body, collider, half_height: half },
        ))
        .id()
}

pub fn read_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    scripted: Option<Res<ScriptedInput>>,
    menu: Option<Res<crate::mods::ModMenu>>,
    pads: Query<&bevy::input::gamepad::Gamepad>,
    mut pad: ResMut<crate::pad::PadState>,
    mut state: ResMut<InputState>,
) {
    if let Some(s) = scripted {
        state.0 = s.clone();
        if let (true, Some(p)) = (crate::pad::in_tests(), crate::pad::active(&pads)) {
            crate::pad::apply(p, &mut pad.sprint_latch, &mut state.0);
        }
        return;
    }
    // the inventory list open: the keys navigate it (GGPlayerControllerGame.mInventoryOpen stops
    // PlayerMove and UpdateCameraView)
    if menu.is_some_and(|m| m.open) || crate::uvm::menus::ui_open() || crate::uvm::inventory::open() {
        state.0 = ScriptedInput::default();
        return;
    }
    let axis = |a: KeyCode, b: KeyCode| (keys.pressed(a) as i32 - keys.pressed(b) as i32) as f32;
    state.0 = ScriptedInput {
        forward: axis(KeyCode::KeyW, KeyCode::KeyS),
        right: axis(KeyCode::KeyD, KeyCode::KeyA),
        sprint: keys.pressed(KeyCode::ShiftLeft),
        jump: keys.just_pressed(KeyCode::Space),
        headbutt: mouse.just_pressed(MouseButton::Left),
        lick: keys.just_pressed(KeyCode::KeyE),
        ragdoll: keys.just_pressed(KeyCode::KeyQ),
        baa: keys.just_pressed(KeyCode::Digit1),
        slomo: keys.just_pressed(KeyCode::KeyF),
        freelook: mouse.pressed(MouseButton::Right),
        special: keys.just_pressed(KeyCode::KeyR),
        cam_yaw: None,
        cam_dist: None,
        cam_pitch: None,
    };
    if let Some(p) = crate::pad::active(&pads) {
        crate::pad::apply(p, &mut pad.sprint_latch, &mut state.0);
    }
}

/// GGCameraModeOrbital.UpdateCamera (+ GGPlayerControllerGame.UpdateCameraView, GGCameraMode.Zoom
/// and PreventClipping).
#[allow(clippy::type_complexity)]
pub fn camera_control(
    time: Res<Time>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    input: Res<InputState>,
    mut cam: Query<(&mut GoatCamera, &mut Transform, &mut Projection), Without<Goat>>,
    goat: Query<(&Goat, &Transform, Option<&crate::rig::Animator>, Option<&crate::rig::Rig>)>,
    phys: Res<Physics>,
    windows: Query<&Window, With<PrimaryWindow>>,
    pads: Query<&bevy::input::gamepad::Gamepad>,
) {
    let _prof_scope = crate::prof::scope("goat::camera_control");
    let dt = time.delta_secs().min(0.1);
    let Ok((mut c, mut t, mut proj)) = cam.single_mut() else { return };
    let Ok((g, gt, anim, rig)) = goat.single() else { return };
    let menu_open = crate::mods::menu_open() || crate::uvm::menus::ui_open() || crate::uvm::inventory::open();
    // aTurn / aLookUp; the camera pitch stops short of straight up/down (LimitViewRotation)
    if !menu_open {
        // Options: mouse sensitivity and invert (PlayerInput.MouseSensitivity / bInvertTurn / bInvertMouse)
        let (k, ix, iy) = crate::uvm::settings::applied().map(|a| (a.mouse_scale, a.invert_x, a.invert_y)).unwrap_or((1.0, false, false));
        let (sx, sy) = (if ix { -1.0 } else { 1.0 }, if iy { -1.0 } else { 1.0 });
        c.yaw -= motion.delta.x * 0.003 * k * sx;
        c.pitch = (c.pitch - motion.delta.y * 0.003 * k * sy).clamp(-1.5, 1.5);
        // the pad's right stick (GBA_TurnLeft_Gamepad / GBA_Look_Gamepad)
        if let Some(p) = crate::pad::active(&pads) {
            let r = crate::pad::stick(p.right_stick());
            c.yaw -= r.x * crate::pad::LOOK_SPEED * dt * k * sx;
            c.pitch = (c.pitch + r.y * crate::pad::LOOK_SPEED * 0.6 * dt * k * sy).clamp(-1.5, 1.5);
        }
    }
    // D-pad up/down: GBA_ZoomIn / GBA_ZoomOut, a notch per press
    let pad_zoom = crate::pad::active(&pads).map(|p| {
        use bevy::input::gamepad::GamepadButton as B;
        p.just_pressed(B::DPadDown) as i32 as f32 - p.just_pressed(B::DPadUp) as i32 as f32
    });
    if let (Some(z), false) = (pad_zoom.filter(|z| *z != 0.0), menu_open) {
        let (lo, hi) = crate::uvm::bridge::zoom_limits();
        c.zoom_desired = (c.zoom_desired + uu(cam::ZOOM_UNIT) * z).clamp(uu(lo), uu(hi));
    }
    // GBA_ZoomIn / GBA_ZoomOut: mZoomUnit per notch, clamped; the current zoom lerps (x3)
    if scroll.delta.y != 0.0 && !menu_open {
        let notches = scroll.delta.y.signum() * scroll.delta.y.abs().max(1.0).round();
        // limits from GGCameraModeOrbital (Workshop/game mutators may change them)
        let (lo, hi) = crate::uvm::bridge::zoom_limits();
        c.zoom_desired = (c.zoom_desired - uu(cam::ZOOM_UNIT) * notches).clamp(uu(lo), uu(hi));
    }
    c.distance += (c.zoom_desired - c.distance) * (dt * 3.0).min(1.0);
    if let Some(y) = input.0.cam_yaw {
        c.yaw = y;
    }
    if let Some(d) = input.0.cam_dist {
        c.distance = d;
        c.zoom_desired = d;
    }
    if let Some(p) = input.0.cam_pitch {
        c.pitch = p;
    }
    // mTargetLocation: the goat (the ragdoll keeps the goat entity on its pelvis); going in and
    // out of ragdoll eases the jump between the two positions away (mTargetRagdollOffset)
    let new_target = gt.translation;
    if g.ragdoll != c.was_ragdoll {
        c.ragdoll_offset = c.target - new_target;
        c.was_ragdoll = g.ragdoll;
    }
    c.target = new_target + c.ragdoll_offset;
    c.ragdoll_offset = c.ragdoll_offset.lerp(Vec3::ZERO, dt.min(1.0));
    let look_at = c.target + Vec3::Y * uu(tune::CAMERA_LOOKAT_Z);
    let offset = Quat::from_euler(EulerRot::YXZ, c.yaw, c.pitch, 0.0) * Vec3::new(0.0, 0.0, c.distance);
    let mut desired = look_at + offset;
    // PreventClipping: sweep from the target out to the camera; world geometry and people block
    // it (mBlockCamera), props (GGKActor) and the goat don't
    let dir = desired - look_at;
    let len = dir.length();
    if len > 0.01 {
        let near = 0.1;
        let goat_body = g.body;
        let ball = rp::SharedShape::ball(near);
        let pose = rp::Pose::from_translation(convert::to_rapier(look_at));
        let opts = rapier3d::parry::query::ShapeCastOptions { max_time_of_impact: len, stop_at_penetration: false, ..Default::default() };
        let people = phys.owners.keys().copied().collect::<std::collections::HashSet<_>>();
        let blocks_people = |h: rp::ColliderHandle, col: &rp::Collider| col.parent().is_none() || people.contains(&h);
        let filter_people = rp::QueryFilter::default().exclude_rigid_body(goat_body).predicate(&blocks_people);
        if let Some((_, hit)) = phys.world.cast_shape(&pose, convert::to_rapier(dir / len), &*ball, opts, filter_people) {
            desired = look_at + dir / len * hit.time_of_impact.max(0.05);
        }
    }
    // AddCameraBounce: a sine at the walk/sprint cycle rate while walking on the ground
    let hspeed = Vec2::new(g.velocity.x, g.velocity.z).length();
    let amp = if g.sprinting { cam::SPRINT_BOUNCE } else { cam::WALK_BOUNCE };
    if hspeed > 0.01 && g.grounded && !g.ragdoll {
        let cycle = match (anim, rig) {
            (Some(a), Some(r)) => r.data.clips.get(a.clip).map(|cl| cl.length / cl.rate.max(0.01) / a.speed.max(0.05)).unwrap_or(0.0),
            _ => 0.0,
        };
        if cycle > 0.0 {
            c.bounce += std::f32::consts::TAU / cycle * dt;
            desired.y += c.bounce.sin() * uu(amp);
        }
    } else {
        desired.y += c.bounce.sin() * uu(cam::WALK_BOUNCE);
    }
    // UE3 FOVs are horizontal
    if let (Projection::Perspective(p), Ok(w)) = (proj.as_mut(), windows.single()) {
        let aspect = w.width().max(1.0) / w.height().max(1.0);
        // Options -> Apply (GGSystemSettings.mFOV through the pause menu)
        let fov = crate::uvm::settings::applied().map(|a| a.fov).unwrap_or(cam::FOV_DEG);
        let half = (fov.to_radians() * 0.5).tan() / aspect;
        p.fov = 2.0 * half.atan();
    }
    *t = Transform::from_translation(desired).looking_at(look_at, Vec3::Y);
}

#[allow(clippy::too_many_arguments)]
pub fn goat_move(
    npcs: Query<&crate::npc::Npc>,
    vtime: Res<Time<Virtual>>,
    mut commands: Commands,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut score: ResMut<crate::score::Score>,
    time: Res<Time>,
    input: Res<InputState>,
    mut phys: ResMut<Physics>,
    mut goats: Query<(Entity, &mut Goat, &mut Transform, Option<&ActiveRagdoll>)>,
    cam: Query<&GoatCamera>,
    riding: Res<crate::vehicle::Riding>,
) {
    let _prof_scope = crate::prof::scope("goat::goat_move");
    let dt = time.delta_secs().min(0.05) * phys.time_scale;
    let Ok((entity, mut g, mut t, active)) = goats.single_mut() else { return };
    // GGGameInfoRace: the goat waits out the countdown (state RaceCountdown)
    let inp = if crate::race::frozen() { ScriptedInput::default() } else { input.0.clone() };
    g.ability_cooldown = (g.ability_cooldown - dt).max(0.0);

    // GGGoat.SetRagdoll while doing a manual: OnBreakCombo(mManualFailString) + StopManual(false)
    if g.manual.active && (g.ragdoll || active.is_some()) {
        score.break_combo("");
        let s = g.manual.stop();
        score.report(crate::manual::trick(s));
        score.hud_event(crate::score::HudEvent::ManualStop);
    }
    if g.pending_respawn && !g.ragdoll && active.is_none() {
        g.pending_respawn = false;
        respawn(&mut g, &mut phys, &mut t);
    }
    if inp.ragdoll {
        if g.ragdoll {
            if g.ragdoll_time >= tune::MIN_RAGDOLL_TIME {
                stand_up(&mut commands, entity, &mut g);
            }
        } else {
            start_ragdoll(&mut commands, entity, &mut g, &mut phys);
        }
    }
    // on a bike: vehicle::ride moves the goat
    if riding.bike.is_some() && !g.ragdoll {
        return;
    }
    if g.ragdoll {
        g.ragdoll_time += dt;
        // in water the goat only gets up near the shore (mDistanceToGround < mIsInAirThreshold)
        let can_stand = !g.env.water || g.env.near_ground;
        // pressing a movement key stands the goat up again (GBA_Forward also triggers StandUp)
        if (inp.forward != 0.0 || inp.right != 0.0) && g.ragdoll_time > tune::MIN_RAGDOLL_TIME * 2.0 && can_stand {
            stand_up(&mut commands, entity, &mut g);
        }
        // GGPlayerInputGame.Jump: on land it stands the goat up, in water DoRagdollJump
        if inp.jump && !g.env.water {
            if g.ragdoll_time >= tune::MIN_RAGDOLL_TIME {
                stand_up(&mut commands, entity, &mut g);
            }
        } else if inp.jump {
            // DoRagdollJump: give the whole ragdoll mRagdollJumpZ upward velocity
            if std::env::var_os("GOAT_TRACE").is_some() {
                println!("    ragdoll jump: {} bodies", active.map(|r| r.bodies.len()).unwrap_or(0));
            }
            if let Some(rd) = active {
                for &(_, h) in &rd.bodies {
                    if let Some(rb) = phys.world.bodies.get_mut(h) {
                        let mut v = rb.linvel();
                        v.y = uu(tune::RAGDOLL_JUMP_Z);
                        rb.set_linvel(v, true);
                    }
                }
            }
        }
        return;
    }
    if active.is_some() {
        return; // stand-up happens later this frame
    }

    // --- desired ground speed (CalcDesiredGroundSpeed / UpdateGroundSpeed) ------------
    g.sprinting = inp.sprint;
    let tw = crate::mods::tweak();
    let mv = crate::uvm::bridge::movement();
    let desired_forward = if g.sprinting { mv.sprint_speed * tw.sprint_speed } else { mv.walk_speed * tw.walk_speed };
    if g.ground_speed_forward > desired_forward {
        g.ground_speed_forward += (desired_forward - g.ground_speed_forward) * (dt * tune::DECEL_INTERP).min(1.0);
        if g.ground_speed_forward - desired_forward < 1.0 {
            g.ground_speed_forward = desired_forward;
        }
    } else {
        g.ground_speed_forward = desired_forward;
    }

    // Input is relative to the camera yaw; the goat turns to face where it walks.
    let cam_yaw = cam.single().map(|c| c.yaw).unwrap_or(g.yaw);
    let fwd = Quat::from_rotation_y(cam_yaw) * -Vec3::Z;
    let right = Quat::from_rotation_y(cam_yaw) * Vec3::X;
    let wish = fwd * inp.forward + right * inp.right;
    let has_input = wish.length_squared() > 0.01;
    let mut hv = Vec3::new(g.velocity.x, 0.0, g.velocity.z);
    let yaw_before = g.yaw;
    let pitch_before = g.pitch;
    let flipping = !g.grounded && inp.freelook && !g.wall.active;
    // CheckManualActivation -> AttemptManual: on the ground now, else on the next landing
    if g.manual.check_input(inp.forward, time.elapsed_secs()) && !g.manual.active {
        if g.grounded {
            g.manual.start();
            score.hud_event(crate::score::HudEvent::ManualStart);
        } else {
            g.manual.pending = true;
        }
    }
    let manual = g.manual.active && g.grounded;
    // PHYS_WallRun applies its own gravity
    let mut wall_gravity = false;
    if flipping {
        // GGGoat.UpdateRotation with the free-look rotation rate (W/S flip, A/D spin)
        crate::air::rotate_in_air(&mut g, inp.forward, inp.right, dt, vtime.relative_speed());
    } else if manual {
        // PlayerMove while doing a manual: NewAccel = vector(Pawn.Rotation) * 30 and the pawn
        // keeps turning toward the camera; CalcVelocity steers the speed it has onto its facing
        g.yaw = rinterp_yaw(g.yaw, cam_yaw, dt, tune::TURN_INTERP);
        let facing = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
        let v_uu = hv / S;
        let speed = v_uu.length();
        let steered = v_uu - (v_uu - facing * speed) * (dt * tune::GROUND_FRICTION).min(1.0);
        let v = steered + facing * crate::manual::ACCEL * dt;
        hv = v.clamp_length_max(g.ground_speed_forward) * S;
        let (out, added) = g.manual.tick(inp.forward, dt);
        if added > 0 {
            score.keep_alive();
        }
        let (value, ms) = (g.manual.value, g.manual.score);
        score.hud_event(crate::score::HudEvent::Manual { value, score: ms });
        if out == crate::manual::Outcome::Fall {
            // AddToBreakManualTime -> SetRagdoll(true)
            start_ragdoll(&mut commands, entity, &mut g, &mut phys);
        }
    } else if g.wall.active {
        wall_gravity = true;
        crate::air::update_wall_run(&mut g, dt, wish, phys.world.gravity.y);
        hv = Vec3::new(g.velocity.x, 0.0, g.velocity.z);
    } else {
        // GGPlayerControllerGame.PlayerMove: Acceleration = AccelRate * Normal(aForward * X +
        // aStrafe * Y) on the pawn's own axes (before UpdateRotation turns it), so A/D strafe
        // and S backs up
        let pfwd = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
        let pright = Quat::from_rotation_y(g.yaw) * Vec3::X;
        let accel_dir = if has_input { (pfwd * inp.forward + pright * inp.right).normalize_or_zero() } else { Vec3::ZERO };
        if has_input {
            // facing the camera again also levels the goat out
            g.pitch -= g.pitch * (dt * 10.0).min(1.0);
            // UpdateRotation: with movement input the pawn turns toward the camera yaw
            // (RInterpTo at mPawnTurnInterpSpeed)
            g.yaw = rinterp_yaw(g.yaw, cam_yaw, dt, tune::TURN_INTERP);
        }
        let facing = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
        let v_uu = hv / S;
        let new_v = if g.grounded {
            let (v, started) = walking_velocity(v_uu, accel_dir, facing, g.ground_speed_forward, &mut g.braking, dt);
            if started && v_uu.length() > tune::WALK_SPEED {
                // GGGoat.Braking: slide sound + Brake animation above walking speed
                g.brake_slide = true;
                sounds.play("brake", None);
            }
            if !g.braking {
                g.brake_slide = false;
            }
            v
        } else {
            g.braking = false;
            g.brake_slide = false;
            match crate::uvm::bridge::flight() {
                // a flying goat steers at mAirAccelRate up to AirSpeed (flaps and dives may go faster)
                Some(f) if !g.ragdoll => {
                    let v = v_uu + accel_dir * f.air_accel_rate * dt;
                    v.clamp_length_max(v_uu.length().max(f.air_speed))
                }
                _ => falling_velocity(v_uu, accel_dir, dt),
            }
        };
        hv = new_v * S;
    }
    g.velocity.x = hv.x;
    g.velocity.z = hv.z;
    if !g.grounded && !g.ragdoll {
        // GGTrickRotation.OnAirborne accumulates every rotation change in the air
        g.air_yaw_deg += (g.yaw - yaw_before).to_degrees();
        g.air_pitch_deg += (g.pitch - pitch_before).to_degrees();
    }

    // --- jumping (DoJump / DoDoubleJump) --------------------------------------------------
    if inp.jump && !g.grounded && g.velocity.y < 0.0 {
        g.tried_landing_at = Some(time.elapsed_secs());
    }
    if inp.jump && g.wall.active {
        crate::air::wall_jump(&mut g);
        sounds.play("jump", None);
        score.report(crate::score::wall_jump());
    } else if inp.jump {
        if g.grounded {
            g.ready_to_wall_run = true;
            sounds.play("jump", None);
            g.velocity.y = uu(crate::uvm::bridge::movement().jump_z * crate::mods::tweak().jump_z);
            g.grounded = false;
            g.double_jump_ready = true;
            // GGGoat double jump + MaxMultiJump added by Workshop mutators
            g.multi_jump_remaining = 1i32.saturating_add(crate::uvm::bridge::movement().extra_jumps);
        } else if let Some(f) = crate::uvm::bridge::flight().filter(|_| !g.ragdoll) {
            // Pawn.DoJump for birds: DoBirdFlap (Velocity.Z = mFlapZ and a forward push that fades
            // as the 2D speed nears 1.2 x AirSpeed), at most every mAirFlapCooldown seconds
            let now = time.elapsed_secs();
            if now - g.flap_time > f.flap_cooldown {
                g.flap_time = now;
                let fwd = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
                let speed2d = Vec2::new(g.velocity.x, g.velocity.z).length() / S;
                let push = f.flap_z * (1.0 - (speed2d / (f.air_speed * 1.2).max(1.0)).clamp(0.0, 1.0));
                g.velocity.y = uu(f.flap_z);
                g.velocity += fwd * uu(push);
                g.ready_to_wall_run = true;
                sounds.play("jump", None);
            }
        } else if g.double_jump_ready && g.multi_jump_remaining > 0 && g.velocity.y.abs() < uu(tune::DOUBLE_JUMP_THRESHOLD) {
            sounds.play("jump", None);
            let mv = crate::uvm::bridge::movement();
            g.velocity.y = uu((mv.jump_z + mv.multi_jump_boost) * crate::mods::tweak().jump_z);
            g.multi_jump_remaining -= 1;
        }
    }
    // PHYS_Ladder (Pawn.LadderSpeed 200): in a LadderVolume, forward climbs (toward the wall:
    // up, away: down), no gravity; jumping or a ragdoll lets go
    let facing = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
    // grabbed when pushing into the wall, held while off the ground
    let on_ladder = g.env.ladder.filter(|w| {
        !g.ragdoll && !inp.jump && g.velocity.length() < uu(tune::RAGDOLL_COLLISION_SPEED) && (!g.grounded || (inp.forward > 0.1 && facing.dot(*w) > 0.3))
    });
    if let Some(wall) = on_ladder {
        let toward = if facing.dot(wall) >= 0.0 { 1.0 } else { -1.0 };
        let climb = inp.forward * toward;
        if climb.abs() > 0.1 {
            g.velocity = Vec3::Y * uu(LADDER_SPEED) * climb.signum() + wall * uu(20.0) * climb.max(0.0);
            g.grounded = false;
        } else {
            g.velocity = Vec3::ZERO;
        }
    } else if !wall_gravity {
        // GravityVolume.GravityZ replaces the world's gravity inside it
        let gravity = g.env.gravity_y.unwrap_or(phys.world.gravity.y);
        g.velocity.y += gravity * dt;
        // gliding (Pawn flight; the native code isn't ported 1:1): lift from mAirLiftCoefficient
        // at the angle of attack (the level body against the oncoming air), drag from
        // mAirDragCoefficient at that lift, both growing with speed squared and scaled so a bird
        // at AirSpeed with lift 1 holds its height
        if let (Some(f), false, false) = (crate::uvm::bridge::flight(), g.grounded, g.ragdoll) {
            let v = g.velocity;
            let v2d = Vec2::new(v.x, v.z).length();
            let aoa = (-v.y).atan2(v2d.max(1e-3)).to_degrees();
            let lift_c = crate::uvm::bridge::eval_curve(&f.lift, aoa);
            let drag_c = crate::uvm::bridge::eval_curve(&f.drag, lift_c);
            let k = gravity.abs() / uu(f.air_speed.max(1.0)).powi(2);
            g.velocity.y += lift_c * (f.speed_lift * 0.5) * k * v2d * v2d * dt;
            let speed = v.length();
            if speed > 1e-3 {
                g.velocity -= v / speed * (drag_c * k * speed * speed * dt).min(speed);
            }
        }
    }
    let impact_velocity = g.velocity;

    // --- move with the character controller -----------------------------------------------
    let Some(rb) = phys.world.bodies.get(g.body) else { return };
    let pose = *rb.position();
    let shape = phys.world.colliders[g.collider].shared_shape().clone();
    let kcc = KinematicCharacterController {
        up: rp::Vector::Y,
        offset: CharacterLength::Absolute(0.02),
        slide: true,
        autostep: Some(CharacterAutostep {
            max_height: CharacterLength::Absolute(uu(tune::MAX_STEP)),
            min_width: CharacterLength::Absolute(0.1),
            include_dynamic_bodies: false,
        }),
        max_slope_climb_angle: tune::WALKABLE_FLOOR_Z.acos(),
        min_slope_slide_angle: tune::WALKABLE_FLOOR_Z.acos(),
        snap_to_ground: if g.velocity.y <= 0.0 { Some(CharacterLength::Absolute(0.15)) } else { None },
        normal_nudge_factor: 1.0e-4,
    };
    // PhysicsVolume.ZoneVelocity carries the goat along (water currents, conveyors, updrafts)
    // standing on the ground, the goat doesn't push down into it: the controller's slide loop then
    // ends at once instead of iterating its 20 times every frame (6.7 ms -> 0.1 ms); snap_to_ground
    // keeps it on slopes going down
    let mut want = (g.velocity + g.env.zone_velocity) * dt;
    if g.grounded && want.y < 0.0 && !g.wall.active && g.env.ladder.is_none() {
        want.y = 0.0;
    }
    let desired = convert::to_rapier(want);
    let mut hits = Vec::new();
    let movement = {
        let _kcc = crate::prof::scope("goat_move/kcc");
        let qp = phys.world.query_pipeline_with_filter(rp::QueryFilter::default().exclude_rigid_body(g.body));
        kcc.move_shape(dt, &qp, &*shape, &pose, desired, |c| hits.push(c))
    };
    // running into people at speed knocks them over
    let hspeed = Vec2::new(g.velocity.x, g.velocity.z).length();
    for h in &hits {
        if let Some(&npc) = phys.owners.get(&h.handle) {
            if hspeed > uu(tune::WALK_SPEED + 100.0) {
                commands.entity(npc).insert(crate::npc::Knocked { velocity: Vec3::new(g.velocity.x, hspeed * 0.3, g.velocity.z) });
            }
        }
    }
    // GGGoat.HitWall: the wall's ImpactSound when hitting it faster than its ImpactThreshold
    // (at most once per mHitWallInterval)
    let now = time.elapsed_secs();
    if now - g.last_hit_wall > tune::HIT_WALL_INTERVAL {
        let speed = impact_velocity.length() / S;
        for h in &hits {
            if (-convert::from_rapier(h.hit.normal1)).y >= tune::WALKABLE_FLOOR_Z {
                continue;
            }
            let Some(s) = phys.surface_at(h.handle, rp::FeatureId::Unknown) else { continue };
            if let Some(cue) = s.impact.clone().filter(|_| speed >= s.impact_threshold && speed > 100.0) {
                g.last_hit_wall = now;
                sounds.play(&cue, None);
                break;
            }
        }
    }
    // GGGoat.HitWall: running into a prop whose material trips the goat (CanCauseGoatToTrip)
    // faster than mRagdollCollisionSpeed (half way between walk and sprint) ragdolls it
    let mut tripped = false;
    // push props we walk into (waking dormant ones, like bumping a sleeping KActor)
    for h in &hits {
        let parent = phys.world.colliders.get(h.handle).and_then(|c| c.parent());
        if let Some(parent) = parent {
            // a wall-like contact (not the floor we stand on)
            let wall_like = (-convert::from_rapier(h.hit.normal1)).y < tune::WALKABLE_FLOOR_Z;
            let speed = impact_velocity.length();
            if wall_like && phys.trip.contains(&parent) && speed > uu(tune::RAGDOLL_COLLISION_SPEED) && !g.ragdoll {
                tripped = true;
            }
            if phys.wake(parent) {
                if let Some(other) = phys.world.bodies.get_mut(parent) {
                    {
                        let push = convert::to_rapier(Vec3::new(g.velocity.x, 0.0, g.velocity.z) * tune::MASS * 0.05);
                        other.apply_impulse(push, true);
                    }
                }
            }
        }
    }
    if tripped {
        if std::env::var_os("GOAT_TRACE").is_some() {
            println!("    goat tripped on a prop at {:.0} uu/s", impact_velocity.length() / S);
        }
        start_ragdoll(&mut commands, entity, &mut g, &mut phys);
    }
    let new_pos = convert::from_rapier(pose.translation) + convert::from_rapier(movement.translation);
    let was_grounded = g.grounded;
    let fall_speed = -g.velocity.y;
    // like PHYS_Falling, a rising goat is airborne even when it brushes a ledge
    g.grounded = movement.grounded && g.velocity.y <= 0.1;
    // the capsule scales with the goat's DrawScale (set by mutator script)
    let half = uu(tune::COLLISION_HEIGHT) * crate::uvm::bridge::goat_scale();
    let radius = uu(tune::COLLISION_RADIUS) * crate::uvm::bridge::goat_scale();
    // walls: start / keep a wall run (PHYS_WallRun)
    let hdir = Vec3::new(impact_velocity.x, 0.0, impact_velocity.z).normalize_or_zero();
    // returns (gap between the collision surface and the wall, wall normal)
    let wall_probe = |dir: Vec3| -> Option<(f32, Vec3)> {
        if dir == Vec3::ZERO {
            return None;
        }
        let ray = rp::Ray::new(convert::to_rapier(new_pos), convert::to_rapier(dir.normalize()));
        // walls are world geometry only (not people or props)
        let static_only = |_: rp::ColliderHandle, c: &rp::Collider| c.parent().is_none();
        let filter = rp::QueryFilter::default().predicate(&static_only);
        phys.world
            .cast_ray_and_get_normal(&ray, radius + 0.35, true, filter)
            .map(|(_, hit)| ((hit.time_of_impact - radius).max(0.0), convert::from_rapier(hit.normal)))
    };
    if g.wall.active {
        // physWallRun's trace toward -Floor (mMaxStepOverWallBump deep)
        let n = g.wall.normal;
        let hit = wall_probe(-Vec3::new(n.x, 0.0, n.z));
        if g.grounded || !crate::air::keep_wall(&mut g, hit) {
            g.wall.active = false;
        }
        if g.wall.active && g.wall.time >= crate::score::WALL_RUN_MIN_TIME && !g.wall.scored {
            g.wall.scored = true;
            score.report(crate::score::wall_run());
        }
    } else if !g.grounded && !hits.is_empty() {
        if let Some((_, n)) = wall_probe(hdir).or_else(|| wall_probe(Quat::from_rotation_y(g.yaw) * -Vec3::Z)) {
            crate::air::try_start_wall_run(&mut g, n);
        }
    }
    if !g.grounded {
        crate::air::collect_jump_over(&phys, &mut g, new_pos, half);
    }
    if !was_grounded && g.grounded {
        let cam_fwd = Quat::from_rotation_y(cam_yaw) * -Vec3::Z;
        let _land = crate::prof::scope("goat_move/land");
        let landing = crate::air::land(&phys, &g, new_pos, half, impact_velocity, cam_fwd, time.elapsed_secs());
        if fall_speed > uu(300.0) || landing.ragdoll {
            sounds.play("land", None);
        }
        if landing.ragdoll {
            g.jump_over.clear();
            g.velocity = impact_velocity;
            start_ragdoll(&mut commands, entity, &mut g, &mut phys);
            g.manual.pending = false;
        } else {
            // GGGoat.Landed: a manual in progress ends well; a manual asked for in the air starts
            if g.manual.active {
                let s = g.manual.stop();
                score.report(crate::manual::trick(s));
                score.hud_event(crate::score::HudEvent::ManualStop);
            }
            if std::mem::take(&mut g.manual.pending) {
                g.manual.start();
                score.hud_event(crate::score::HudEvent::ManualStart);
            }
            if landing.perfect {
                score.report(crate::score::perfect_landing());
            }
            let licked = g.grabbed.and_then(|(b, _)| phys.prop_info.get(&b).map(|i| i.1.clone()));
            if let Some(trick) = crate::score::rotation_trick(g.air_yaw_deg, g.air_pitch_deg, licked.as_deref()) {
                score.report(trick);
            }
            if std::env::var_os("GOAT_TRACE").is_some() {
                let desc: Vec<String> = g.jump_over.iter().map(|crate::air::Below::Collider(c)| {
                    if phys.owners.contains_key(c) { "npc".to_string() } else { format!("{:?} parent={:?}", c.into_raw_parts(), phys.world.colliders.get(*c).and_then(|x| x.parent()).map(|p| p.into_raw_parts())) }
                }).collect();
                println!("    landing: jump_over={desc:?} floor={:?}", landing.floor.map(|f| f.into_raw_parts()));
            }
            // FlushJumpOverInfo: everything between where we jumped from and where we landed
            let from = g.jump_over.first().copied();
            let to = landing.floor.map(crate::air::Below::Collider);
            for b in g.jump_over.iter().skip(1) {
                if Some(*b) == from || Some(*b) == to {
                    continue;
                }
                let crate::air::Below::Collider(c) = *b;
                let info = if let Some(&npc) = phys.owners.get(&c) {
                    npcs.get(npc).ok().map(|n| (n.score, n.actor_name.clone()))
                } else {
                    phys.world.colliders.get(c).and_then(|col| col.parent()).and_then(|p| phys.prop_info.get(&p).cloned())
                };
                if let Some((s, name)) = info.filter(|i| i.0 > 0) {
                    score.report(crate::score::jump_over(&name, s));
                }
            }
        }
        g.jump_over.clear();
        g.pitch = 0.0;
        g.air_yaw_deg = 0.0;
        g.air_pitch_deg = 0.0;
        g.rot_interp = crate::air::ROT_INTERP_SPEED;
        g.ready_to_wall_run = false;
        g.wall.active = false;
    }
    if g.grounded {
        g.tried_landing_at = None;
    }
    if g.grounded && g.velocity.y < 0.0 {
        g.velocity.y = 0.0;
    }
    // bonk head on ceilings
    if !g.grounded && g.velocity.y > 0.0 && movement.translation.y < desired.y * 0.5 {
        g.velocity.y = 0.0;
    }
    if !was_grounded && g.grounded {
        g.double_jump_ready = false;
    }
    if let Some(rb) = phys.world.bodies.get_mut(g.body) {
        rb.set_next_kinematic_translation(convert::to_rapier(new_pos));
    }
    t.translation = new_pos;
    if g.ragdoll {
        return;
    }
    t.rotation = crate::air::goat_rotation(&g);

    // fell out of the world -> respawn
    if new_pos.y < -200.0 {
        respawn(&mut g, &mut phys, &mut t);
    }
}

/// GGPawnBase walking CalcVelocity (GoatGame.exe 0x19501c0), in Unreal units. `accel_dir` is
/// the normalised Acceleration, `facing` the pawn's Rotation vector. Returns the new horizontal
/// velocity and whether braking just started (the Braking event).
pub fn walking_velocity(v: Vec3, accel_dir: Vec3, facing: Vec3, ground_speed_forward: f32, braking: &mut bool, dt: f32) -> (Vec3, bool) {
    let friction = tune::GROUND_FRICTION;
    let mut accel = accel_dir * tune::WALK_ACCEL;
    let mut v = v;
    let mut started = false;
    // no input, or input more than ~72 degrees away from where we are going: brake first
    let brake = accel == Vec3::ZERO || (v != Vec3::ZERO && accel_dir.dot(v.normalize()) < 0.3);
    if brake {
        accel = Vec3::ZERO;
        if !*braking && (v.x.abs() >= 50.0 || v.z.abs() >= 50.0 || v.y.abs() >= 50.0) {
            *braking = true;
            started = true;
        }
        v = gg_braking(v, facing, friction, dt);
    } else {
        // turning: velocity swings toward the acceleration direction
        let speed = v.length();
        v -= (v - accel_dir * speed) * dt * friction;
    }
    v += accel * dt;
    // max speed depends on the direction of travel relative to the facing
    let max = direction_max_speed(v.normalize_or_zero(), facing, ground_speed_forward);
    if v.length_squared() > max * max {
        v = v.normalize() * max;
    }
    if *braking && ((v.x.abs() < 50.0 && v.y.abs() < 50.0 && v.z.abs() < 50.0) || accel != Vec3::ZERO) {
        *braking = false;
    }
    (v, started)
}

/// GGPawnBase braking (0x1952bd0): friction weakens when sliding forward fast and is full when
/// moving sideways/backwards or slowly; applied in 0.03 s steps and averaged over the frame.
pub fn gg_braking(v0: Vec3, facing: Vec3, friction: f32, dt: f32) -> Vec3 {
    let dir = v0.normalize_or_zero();
    let f = (1.0 - facing.dot(dir)).max(0.05);
    let speed_factor = (1.0 - v0.length_squared() * 4e-6).max(0.0);
    let fr = (1.0 - f) * friction * speed_factor + f * friction;
    let mut v = v0;
    let mut avg = Vec3::ZERO;
    let mut rem = dt;
    while rem > 0.0 {
        let step = rem.min(0.03);
        rem -= step;
        v -= v * 2.0 * step * fr;
        if v.dot(v0) > 0.0 {
            avg += v * (step / dt);
        }
    }
    if avg.dot(v0) < 0.0 || avg.length_squared() < 100.0 {
        Vec3::ZERO
    } else {
        avg
    }
}

/// GGPawnBase max speed by direction (0x1952ff0): d = facing . velocity direction;
/// d^2 blends strafe speed toward the forward (or reverse) speed.
pub fn direction_max_speed(vel_dir: Vec3, facing: Vec3, forward: f32) -> f32 {
    let d = facing.dot(vel_dir);
    let mv = crate::uvm::bridge::movement();
    let strafe = mv.strafe_speed;
    if d < 0.0 { d * d * (mv.reverse_speed - strafe) + strafe } else { d * d * (forward - strafe) + strafe }
}

/// APawn::physFalling acceleration (0xb39400, bLimitFallAccel): AccelRate x AirControl, plus a
/// nudge when almost still; gravity is added by the caller. Unreal units.
pub fn falling_velocity(v: Vec3, accel_dir: Vec3, dt: f32) -> Vec3 {
    let tick_air = crate::uvm::bridge::movement().air_control * crate::mods::tweak().air_control;
    let speed2d = Vec2::new(v.x, v.z).length();
    let mut limit = tune::WALK_ACCEL * tick_air;
    if speed2d < 10.0 && tick_air > 0.0 {
        limit += (10.0 - speed2d) / dt.max(1e-4);
    } else if speed2d >= tune::GROUND_SPEED && tick_air <= 0.05 {
        limit = 1.0;
    }
    let accel = accel_dir * tune::WALK_ACCEL;
    let accel = if accel.length() > limit { accel.normalize() * limit } else { accel };
    v + accel * dt
}

/// RInterpTo on the yaw: move the shortest way by Clamp(dt * speed, 0, 1) of the difference.
pub fn rinterp_yaw(current: f32, target: f32, dt: f32, speed: f32) -> f32 {
    if speed <= 0.0 {
        return target;
    }
    let d = (target - current + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    current + d * (dt * speed).clamp(0.0, 1.0)
}

/// GGPawn.GetMaterialBelowFeet: the MaterialType under a pawn (trace 1.5 x collision height).
pub fn material_below(phys: &Physics, at: Vec3, half_height: f32, exclude: rp::RigidBodyHandle) -> String {
    let ray = rp::Ray::new(convert::to_rapier(at), rp::Vector::NEG_Y);
    let filter = rp::QueryFilter::default().exclude_rigid_body(exclude);
    phys.world
        .cast_ray_and_get_normal(&ray, half_height * 1.5 + 0.3, true, filter)
        .and_then(|(c, hit)| phys.surface_at(c, hit.feature))
        .map(|s| s.kind.clone())
        .unwrap_or_default()
}

/// PlayFootStepSound from the animations' AnimNotify_Footstep (GGPawnSoundGroup by MaterialType).
pub fn goat_footsteps(
    phys: Res<Physics>,
    sfx: Res<crate::audio::Sfx>,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut q: Query<(&Goat, &Transform, &mut crate::rig::Animator)>,
) {
    let _prof_scope = crate::prof::scope("goat::goat_footsteps");
    for (g, t, mut a) in &mut q {
        for cue in std::mem::take(&mut a.sounds) {
            sounds.play(&cue, None);
        }
        let n = std::mem::take(&mut a.steps);
        if n == 0 || g.ragdoll || !g.grounded {
            continue;
        }
        let mut kind = material_below(&phys, t.translation, uu(tune::COLLISION_HEIGHT) * crate::uvm::bridge::goat_scale(), g.body);
        // GGPawn.GetMaterialBelowFeet: a water volume under the feet wins
        if let Some(d) = g.env.water_below {
            let trace = tune::COLLISION_HEIGHT * 1.5;
            kind = if d < 0.33 * trace { "Water".into() } else { "ShallowWater".into() };
        }
        let water = kind == "Water" || kind == "ShallowWater";
        let key = if water && g.sprinting { "foot:WaterRun".to_string() } else { format!("foot:{kind}") };
        // unknown material types use DefaultFootstepSound
        let key = if sfx.cues.contains_key(&key) { key } else { "foot:".to_string() };
        sounds.play(&key, None);
    }
}

/// GGGoat.RigidBodyCollision -> PlayImpactSoundAtRagdoll: the ragdoll slamming into something
/// plays that surface's ImpactSound (speed over its ImpactThreshold, once per
/// mHitRigidBodyInterval).
pub fn ragdoll_impacts(
    time: Res<Time>,
    phys: Res<Physics>,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut q: Query<(&mut Goat, &ActiveRagdoll)>,
) {
    let _prof_scope = crate::prof::scope("goat::ragdoll_impacts");
    let now = time.elapsed_secs();
    for (mut g, rd) in &mut q {
        let Some(rb) = phys.world.bodies.get(rd.pelvis) else { continue };
        let v = convert::from_rapier(rb.linvel());
        let dv = (v - g.ragdoll_prev_vel).length() / S;
        let speed = g.ragdoll_prev_vel.length() / S;
        g.ragdoll_prev_vel = v;
        // a collision is a sudden velocity change (gravity alone adds ~12 uu/s per frame)
        if dv < 150.0 || now - g.last_rb_hit < tune::HIT_RIGID_BODY_INTERVAL {
            continue;
        }
        'bodies: for &(_, h) in &rd.bodies {
            let Some(b) = phys.world.bodies.get(h) else { continue };
            for &c in b.colliders() {
                for pair in phys.world.narrow_phase.contact_pairs_with(c) {
                    if !pair.has_any_active_contact() {
                        continue;
                    }
                    let other = if pair.collider1 == c { pair.collider2 } else { pair.collider1 };
                    let Some(s) = phys.surface_at(other, rp::FeatureId::Unknown) else { continue };
                    if let Some(cue) = s.impact.clone().filter(|_| speed >= s.impact_threshold) {
                        g.last_rb_hit = now;
                        sounds.play(&cue, None);
                        break 'bodies;
                    }
                }
            }
        }
    }
}

/// SetRagdoll(true): hand the goat to the physics-asset ragdoll (built in ragdoll::begin).
pub fn start_ragdoll(commands: &mut Commands, entity: Entity, g: &mut Goat, phys: &mut Physics) {
    g.ragdoll = true;
    g.ragdoll_time = 0.0;
    drop_grabbed(g, phys);
    commands.entity(entity).insert(StartRagdoll { velocity: g.velocity });
}

/// StandUp(): ragdoll::end removes the bodies and puts the capsule at the pelvis.
fn stand_up(commands: &mut Commands, entity: Entity, g: &mut Goat) {
    g.ragdoll = false;
    commands.entity(entity).insert(StopRagdoll);
}

pub fn respawn(g: &mut Goat, phys: &mut Physics, t: &mut Transform) {
    if g.ragdoll {
        // stand up first; goat_move respawns once the ragdoll is gone
        g.pending_respawn = true;
        return;
    }
    let p = g.spawn.translation + Vec3::Y * (uu(tune::COLLISION_HEIGHT) * crate::uvm::bridge::goat_scale() + 0.1);
    if let Some(rb) = phys.world.bodies.get_mut(g.body) {
        rb.set_translation(convert::to_rapier(p), true);
    }
    t.translation = p;
    g.velocity = Vec3::ZERO;
}

pub(crate) fn drop_grabbed(g: &mut Goat, phys: &mut Physics) {
    if let Some((_, j)) = g.grabbed.take() {
        phys.world.remove_impulse_joint(j);
    }
}

/// Headbutt (GGAbilityHorn) and lick (GGAbilityBite).
pub fn goat_abilities(
    mut commands: Commands,
    mut score: ResMut<crate::score::Score>,
    npcs: Query<&crate::npc::Npc>,
    mut sounds: ResMut<crate::audio::SoundQueue>,
    mut vtime: ResMut<Time<Virtual>>,
    input: Res<InputState>,
    mut phys: ResMut<Physics>,
    mut goats: Query<(Entity, &mut Goat, &Transform, Option<&mut crate::rig::Animator>, Option<&crate::rig::Rig>)>,
    keys: Res<ButtonInput<KeyCode>>,
    riding: Res<crate::vehicle::Riding>,
) {
    let _prof_scope = crate::prof::scope("goat::goat_abilities");
    let Ok((entity, mut g, t, mut animator, rig)) = goats.single_mut() else { return };
    // R is Use first (vehicle::ride gets on / off a bike)
    if keys.just_pressed(KeyCode::KeyR) && riding.bike.is_none() && !riding.near && !crate::pitch::WEARING.load(std::sync::atomic::Ordering::Relaxed) {
        // GBA_Respawn
        if g.ragdoll {
            g.ragdoll = false;
            commands.entity(entity).insert(StopRagdoll);
        }
        g.pending_respawn = true;
    }
    if input.0.slomo {
        // GGGameInfo.SetSlomo: game speed 0.25
        let s = if vtime.relative_speed() < 0.99 { 1.0 } else { 0.25 };
        vtime.set_relative_speed(s);
    }
    if input.0.baa && !g.ragdoll {
        sounds.play("baa", None);
        if let (Some(a), Some(r)) = (animator.as_deref_mut(), rig) {
            crate::rig::play_oneshot(a, r, "Baa");
        }
    }
    if g.ragdoll {
        return;
    }
    let forward = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
    let head = t.translation + forward * uu(40.0) + Vec3::Y * uu(15.0);
    if input.0.headbutt && g.ability_cooldown <= 0.0 {
        g.ability_cooldown = 0.45;
        if let (Some(a), Some(r)) = (animator.as_deref_mut(), rig) {
            crate::rig::play_oneshot(a, r, "Ram");
        }
        // DealDirectionalDamage: everything touching a sphere around the head socket
        let range = uu(tune::HEADBUTT_RANGE) + 0.4;
        let shape = rp::SharedShape::ball(range);
        let pose = rp::Pose::from_translation(convert::to_rapier(head));
        let mut victims = Vec::new();
        {
            let qp = phys.world.query_pipeline_with_filter(rp::QueryFilter::default().exclude_rigid_body(g.body));
            for (h, _) in qp.intersect_shape(pose, &*shape) {
                victims.push(h);
            }
        }
        let impulse = uu(tune::HEADBUTT_IMPULSE) * crate::mods::tweak().headbutt;
        // GGAbilityHorn.DealDirectionalDamage: TakeDamage(mDamage 200, GGDamageTypeAbilityHorn)
        // on everything in reach (destructibles, explosives and KActors read these)
        let goat_speed = g.velocity.length() / convert::S;
        for &h in &victims {
            phys.hits.push(crate::physics::Hit {
                collider: h,
                damage: tune::HEADBUTT_DAMAGE,
                momentum: forward * tune::HEADBUTT_IMPULSE,
                kind: crate::physics::HitKind::Ability,
                goat: Some(goat_speed),
                point: head,
            });
        }
        for &h in &victims {
            if let Some(&npc) = phys.owners.get(&h) {
                // GGNpc TakeDamage from a headbutt: knocked over with the impulse direction
                commands.entity(npc).insert(crate::npc::Knocked { velocity: (forward + Vec3::Y * 0.4).normalize() * 9.0 });
                g.stat_headbutts += 1;
                if let Ok(n) = npcs.get(npc) {
                    score.report(crate::score::ability_trick("headbutt", !g.grounded, n.score, &n.actor_name));
                }
            }
        }
        let mut bodies: Vec<rp::RigidBodyHandle> = victims.iter().filter_map(|&h| phys.world.colliders.get(h).and_then(|c| c.parent())).collect();
        bodies.sort_by_key(|h| h.into_raw_parts());
        bodies.dedup();
        for parent in bodies {
            if !phys.wake(parent) {
                continue;
            }
            if let Some(rb) = phys.world.bodies.get_mut(parent) {
                {
                    let dir = (forward + Vec3::Y * 0.35).normalize();
                    // keep featherweight props from leaving the map (impulses apply at the next step,
                    // so cap the impulse rather than the resulting velocity)
                    let j = impulse.min(rb.mass() * MAX_LAUNCH_SPEED);
                    rb.apply_impulse_at_point(convert::to_rapier(dir * j), convert::to_rapier(head), true);
                    if let Some((s, name)) = phys.prop_info.get(&parent) {
                        score.report(crate::score::ability_trick("headbutt", !g.grounded, *s, name));
                    }
                    g.stat_headbutts += 1;
                }
            }
        }
    }
    if input.0.lick {
        if let (Some(a), Some(r)) = (animator.as_deref_mut(), rig) {
            crate::rig::play_oneshot(a, r, "Bite");
        }
        if g.grabbed.is_some() {
            drop_grabbed(&mut g, &mut phys);
        } else {
            let range = uu(tune::LICK_RANGE);
            let shape = rp::SharedShape::ball(range);
            let pose = rp::Pose::from_translation(convert::to_rapier(head));
            let mut found = None;
            {
                let qp = phys.world.query_pipeline_with_filter(rp::QueryFilter::default().exclude_rigid_body(g.body));
                for (h, c) in qp.intersect_shape(pose, &*shape) {
                    if c.parent().is_some() {
                        found = Some(h);
                        break;
                    }
                }
            }
            let parent = found.and_then(|h| phys.world.colliders.get(h)).and_then(|c| c.parent());
            if let Some(parent) = parent.filter(|&p| phys.wake(p)) {
                // GGRB_Handle: a stiff spring from the mouth to the grabbed point
                let other_pose = *phys.world.bodies[parent].position();
                let local = other_pose.inverse_transform_point(convert::to_rapier(head));
                let joint = rp::SphericalJointBuilder::new().local_anchor1(convert::to_rapier(forward * uu(40.0) + Vec3::Y * uu(15.0))).local_anchor2(local);
                let j = phys.world.insert_impulse_joint(g.body, parent, joint);
                g.grabbed = Some((parent, j));
                if let Some((s, name)) = phys.prop_info.get(&parent) {
                    score.report(crate::score::ability_trick("lick", !g.grounded, *s, name));
                }
            }
        }
    }
}

pub fn grab_cursor(
    mut cursor: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    scripted: Option<Res<ScriptedInput>>,
    menu: Option<Res<crate::mods::ModMenu>>,
) {
    if scripted.is_some() {
        return; // tests never capture the mouse
    }
    let Ok(mut c) = cursor.single_mut() else { return };
    // the mod menu and the game's Scaleform menus need the mouse
    if menu.is_some_and(|m| m.open) || crate::uvm::menus::ui_open() {
        c.grab_mode = CursorGrabMode::None;
        c.visible = true;
        return;
    }
    if mouse.just_pressed(MouseButton::Left) {
        c.grab_mode = CursorGrabMode::Locked;
        c.visible = false;
    }
    if keys.just_pressed(KeyCode::Escape) {
        c.grab_mode = CursorGrabMode::None;
        c.visible = true;
    }
}

#[cfg(test)]
mod movement_tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;
    const FWD: Vec3 = Vec3::NEG_Z;

    fn run(mut v: Vec3, dir: Vec3, secs: f32, braking: &mut bool) -> Vec3 {
        for _ in 0..(secs / DT) as usize {
            v = walking_velocity(v, dir, FWD, tune::WALK_SPEED, braking, DT).0;
        }
        v
    }

    #[test]
    fn speed_caps_by_direction() {
        let mut b = false;
        // forward: AccelRate 1175 reaches mWalkSpeed 500 in under half a second and stays there
        let v = run(Vec3::ZERO, FWD, 0.5, &mut b);
        assert!((v.length() - 500.0).abs() < 1.0, "{v}");
        // strafing caps at mStrafeSpeed, backing up at mReverseSpeed
        let v = run(Vec3::ZERO, Vec3::X, 1.0, &mut b);
        assert!((v.length() - 250.0).abs() < 1.0, "{v}");
        let v = run(Vec3::ZERO, Vec3::Z, 1.0, &mut b);
        assert!((v.length() - 200.0).abs() < 1.0, "{v}");
    }

    #[test]
    fn reversing_input_brakes_first() {
        let mut b = false;
        let (v, started) = walking_velocity(FWD * 500.0, -FWD, FWD, tune::WALK_SPEED, &mut b, DT);
        assert!(started && b, "input against the motion starts braking");
        assert!(v.dot(FWD) > 0.0 && v.length() < 500.0, "still sliding forward: {v}");
    }

    #[test]
    fn braking_slides_further_from_speed() {
        // friction is weakest sliding forward fast (speed factor 1 - v^2 * 4e-6 hits 0 at 500)
        let stop = |v0: f32| {
            let mut b = false;
            let mut v = FWD * v0;
            let mut d = 0.0;
            let mut t = 0.0;
            while v.length() > 0.0 && t < 20.0 {
                v = walking_velocity(v, Vec3::ZERO, FWD, tune::WALK_SPEED, &mut b, DT).0;
                d += v.length() * DT;
                t += DT;
            }
            (d, t)
        };
        let (d_walk, t_walk) = stop(500.0);
        let (d_slow, _) = stop(150.0);
        assert!(t_walk < 3.0, "stops from walking speed ({t_walk} s)");
        assert!(d_walk > 4.0 * d_slow, "slides much further from 500 ({d_walk}) than from 150 ({d_slow})");
        // sideways motion brakes at full friction
        let mut b = false;
        let side = walking_velocity(Vec3::X * 400.0, Vec3::ZERO, FWD, tune::WALK_SPEED, &mut b, DT).0;
        let ahead = walking_velocity(FWD * 400.0, Vec3::ZERO, FWD, tune::WALK_SPEED, &mut b, DT).0;
        assert!(side.length() < ahead.length());
    }

    #[test]
    fn air_accel_is_limited() {
        // AccelRate x AirControl = 470 uu/s^2 once moving
        let v = falling_velocity(FWD * 300.0, FWD, 1.0);
        assert!((v.length() - (300.0 + 1175.0 * 0.4)).abs() < 0.5, "{v}");
    }
}
