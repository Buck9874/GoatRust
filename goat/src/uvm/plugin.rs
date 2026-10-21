//! In game: builds the mutator host when Workshop mutators are enabled in the mod menu
//! ("Run mutator", key "ws-<id>"). Every frame it mirrors the goat into the script goat, sends
//! held keys and game events (landed, ragdoll, tricks, abilities, baa...), ticks the mutators,
//! applies velocity/location changes the scripts made, forwards the VM log to
//! mods/uvm_log.txt and applies the goat material swaps the scripts ask for.

use super::bridge::{Host, HostEvent, ModEntry, reset};
use bevy::prelude::*;
use std::path::PathBuf;

pub struct MutatorPlugin;

impl Plugin for MutatorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Mutators>()
            .add_systems(Startup, spawn_messages)
            .init_resource::<super::actors::ScriptActors>()
            .init_resource::<super::proxies::Proxies>()
            .init_resource::<super::fx::ScriptFx>()
            .init_resource::<super::npcs::ScriptNpcs>()
            .init_resource::<super::menus::MenuView>()
            .init_resource::<super::menus::MenuTest>()
            .init_resource::<super::menu_actions::MenuActionsDone>()
            .add_systems(Update, super::menu_actions::apply_menu_actions.after(apply_events))
            .add_systems(Update, super::menus::scaleform_menus.before(crate::goat::read_input))
            .add_systems(Update, super::npcs::npcvm_scenario)
            .add_systems(Update, super::matinee::matinee_scenario)
            .add_systems(Update, super::settings::apply_setres)
            .add_systems(Update, (sync_mutators, tick_mutators, apply_events, super::actors::script_actors, super::matinee::movers, super::matinee::map_visibility, super::fx::script_fx, super::proxies::proxies, super::npcs::script_npcs, super::touch::kismet_touch, show_messages).chain().after(crate::score::tick))
            .init_resource::<super::collectibles::Collectibles>()
            .add_systems(Update, super::collectibles::collectibles.after(show_messages))
            // after the animation pose so script bone scales win
            .add_systems(Update, (apply_bone_scales, super::actors::follow_goat_pose).chain().after(crate::rig::animate));
    }
}

type Loaded = Result<(Host, Vec<ModEntry>), String>;

#[derive(Resource, Default)]
pub struct Mutators {
    pub host: Option<Host>,
    /// Workshop ids the running host was built for.
    pub active: Vec<String>,
    loading: Option<(Vec<String>, std::sync::Mutex<std::sync::mpsc::Receiver<Loaded>>)>,
    log_sent: usize,
    pub status: String,
    /// Messages for the player (text, seconds left).
    pub messages: Vec<(String, f32)>,
    /// Extra keys held this frame (test runs press keys ScriptedInput has no field for).
    pub test_keys: Vec<String>,
    /// Kismet GiveScore tricks waiting for the combo: (name, score, special).
    pub kismet_scores: Vec<(String, i32, bool)>,
    /// Kismet tricks scored so far (tests).
    pub kismet_scored: u32,
    /// Achievements unlocked this run (tests, notifications).
    pub achievements: Vec<String>,
    /// The host has been built once for this map.
    started: bool,
    /// Bumped for every new host (systems caching host data rebuild).
    pub generation: u64,
    /// Kismet touch volumes: (event, centre in Bevy space, leads to a GiveScore).
    pub touch_centers: Vec<(super::ObjId, Vec3, bool)>,
}

/// Enabled Workshop items ("ws-<id>" in the mod menu's enabled set), or the
/// GOAT_MUTATORS=id,id override (test runs).
fn wanted(menu: &crate::mods::ModMenu, args: &crate::Args) -> Vec<String> {
    let forced = std::env::var("GOAT_MUTATORS").ok().or_else(|| match args.test.as_deref() {
        Some("mutators") => Some(crate::test_mutators::DEFAULT_ITEMS.to_string()),
        Some("moonjump") => Some(crate::test_mutators::MOON_JUMP_ITEM.to_string()),
        Some("forcepush") => Some(crate::test_mutators::FORCE_PUSH_ITEM.to_string()),
        Some("jetpack") => Some("gm-GoatVanilla.GGMutatorJetPack".to_string()),
        Some("cart") => Some("gm-GoatGame.GGMutatorShoppingGoat".to_string()),
        Some("inventory") => Some("gm-GoatGame.GGMutatorInventory".to_string()),
        Some("music") => Some("gm-GoatGame.GGMutatorGoatMau5".to_string()),
        Some("kismet") => Some(String::new()),
        Some("fly") => Some(crate::test_fly::FLY_ITEMS.to_string()),
        Some("resize") => Some("246864068".to_string()),
        _ => None,
    });
    // keys: "ws-<workshop id>" / "gm-<game mutator FileName>"; bare ids mean Workshop items
    let mut v: Vec<String> = match forced {
        Some(s) => s
            .split(',')
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .map(|x| if x.starts_with("ws-") || x.starts_with("gm-") { x.to_string() } else { format!("ws-{x}") })
            .collect(),
        None => menu.enabled.iter().filter(|k| k.starts_with("ws-") || k.starts_with("gm-")).cloned().collect(),
    };
    v.sort();
    v
}

fn sync_mutators(menu: Res<crate::mods::ModMenu>, args: Res<crate::Args>, mut m: ResMut<Mutators>) {
    let _prof_scope = crate::prof::scope("plugin::sync_mutators");
    if let Some((ids, rx)) = &m.loading {
        let got = rx.lock().unwrap().try_recv();
        match got {
            Ok(Ok((host, _mods))) => {
                let ids = ids.clone();
                m.status = format!("{} mutator(s) running, {} Kismet objects", host.mutators.len(), host.kismet.ops.len());
                info!("[uvm] {}", m.status);
                m.host = Some(host);
                m.active = ids;
                m.loading = None;
                m.log_sent = 0;
            }
            Ok(Err(e)) => {
                warn!("[uvm] {e}");
                let ids = ids.clone();
                m.status = e;
                m.active = ids;
                m.loading = None;
            }
            Err(_) => {}
        }
        return;
    }
    let want = wanted(&menu, &args);
    // the map's Kismet runs in the VM too, so a host exists even without mutators
    let kismet = std::env::var_os("GOAT_NO_KISMET").is_none();
    if want == m.active && (m.started || !kismet) {
        return;
    }
    m.started = true;
    m.host = None;
    m.generation += 1;
    reset();
    if want.is_empty() && !kismet {
        m.active.clear();
        m.status.clear();
        return;
    }
    let ids = want.clone();
    let map = kismet.then(|| upk::cooked_dir().join(&args.map));
    // the front end once per run (a host rebuilt for new mutators doesn't reopen it)
    let main_menu = args.menu && !MAIN_MENU_SHOWN.swap(true, std::sync::atomic::Ordering::Relaxed);
    let (tx, rx) = std::sync::mpsc::channel::<Loaded>();
    // the Scaleform menus' render device (the first pause menu would wait ~0.4 s for it)
    std::thread::spawn(gfxui::warm_up);
    std::thread::spawn(move || {
        let mods: Vec<ModEntry> = super::bridge::mutators_for(&ids);
        let r = if mods.is_empty() && !ids.is_empty() {
            Err("the enabled mods list no mutators".to_string())
        } else {
            let mut pk: Vec<PathBuf> = mods.iter().flat_map(|e| e.packages.clone()).collect();
            pk.sort();
            pk.dedup();
            // mutator start-up and Kismet loading run here too, so the game never hitches on them
            Host::with_map(&pk, map.as_deref())
                .map(|mut h| {
                    let t = std::time::Instant::now();
                    h.start(&mods);
                    h.vm.say(format!("started in {:.0} ms (loader thread)", t.elapsed().as_secs_f32() * 1000.0));
                    // the in-game Scaleform HUD (GGPlayerControllerGame.ClientSetHUD -> StartHUD);
                    // its Ruffle player loads here too, off the game thread
                    if main_menu {
                        // GGHUD.StartMainMenu, opened on the game thread with the window's size
                        // (menus::scaleform_menus); Play (StartClean) starts the HUD itself
                        h.pending_main_menu = true;
                    } else if std::env::var_os("GOAT_NO_HUD").is_none() {
                        let t = std::time::Instant::now();
                        let ok = h.with_vm(|h| h.start_hud());
                        h.vm.say(format!("HUD movie started: {ok} in {:.0} ms", t.elapsed().as_secs_f32() * 1000.0));
                    }
                    (h, mods)
                })
                .map_err(|e| format!("UnrealScript VM: {e:#}"))
        };
        let _ = tx.send(r);
    });
    m.status = "loading UnrealScript packages...".into();
    m.loading = Some((want, std::sync::Mutex::new(rx)));
}

static MAIN_MENU_SHOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Unreal key name for a Bevy key (as DefaultInput.ini spells them).
fn ue_key(k: KeyCode) -> Option<String> {
    use KeyCode::*;
    let digits = ["zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];
    let dbg = format!("{k:?}");
    if let Some(l) = dbg.strip_prefix("Key").filter(|l| l.len() == 1) {
        return Some(l.to_string());
    }
    if let Some(d) = dbg.strip_prefix("Digit").and_then(|d| d.parse::<usize>().ok()) {
        return Some(digits[d].to_string());
    }
    if dbg.len() <= 3 && dbg.starts_with('F') && dbg[1..].parse::<u32>().is_ok() {
        return Some(dbg);
    }
    Some(
        match k {
            Space => "SpaceBar",
            ShiftLeft => "LeftShift",
            ShiftRight => "RightShift",
            ControlLeft => "LeftControl",
            ControlRight => "RightControl",
            AltLeft => "LeftAlt",
            AltRight => "RightAlt",
            Enter => "Enter",
            Escape => "Escape",
            Tab => "Tab",
            Backspace => "BackSpace",
            ArrowUp => "Up",
            ArrowDown => "Down",
            ArrowLeft => "Left",
            ArrowRight => "Right",
            Insert => "Insert",
            Delete => "Delete",
            Home => "Home",
            End => "End",
            PageUp => "PageUp",
            PageDown => "PageDown",
            _ => return None,
        }
        .to_string(),
    )
}

/// The goat as of last frame (game events are its changes).
#[derive(Default)]
pub struct Prev {
    grounded: bool,
    ragdoll: bool,
    wall: bool,
    headbutts: u32,
    grabbing: bool,
    respawn: bool,
    manual: bool,
    started: bool,
}

#[allow(clippy::too_many_arguments)]
fn tick_mutators(
    time: Res<Time>,
    mut m: ResMut<Mutators>,
    mut prev: Local<Prev>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    input: Res<crate::InputState>,
    tricks: Res<crate::score::TrickEvents>,
    scripted: Option<Res<crate::goat::ScriptedInput>>,
    mut phys: ResMut<crate::physics::Physics>,
    mut goats: Query<(Entity, &mut crate::goat::Goat, &mut Transform), Without<crate::goat::GoatCamera>>,
    mut cams: Query<(&Transform, &mut crate::goat::GoatCamera), Without<crate::goat::Goat>>,
    actors: Res<super::actors::ScriptActors>,
    mut commands: Commands,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    pads: Query<&bevy::input::gamepad::Gamepad>,
) {
    let _prof_scope = crate::prof::scope("plugin::tick_mutators");
    let dt = time.delta_secs();
    let _prof = super::proxies::Prof(std::time::Instant::now(), "tick_mutators");
    let Some(h) = m.host.as_mut() else {
        *prev = Prev::default();
        return;
    };
    let Ok((goat_entity, mut g, mut tr)) = goats.single_mut() else { return };
    let s = crate::convert::S;
    let p = tr.translation;
    let v = g.velocity;
    let f = Quat::from_rotation_y(g.yaw) * -Vec3::Z;
    let state = super::sync::GoatState {
        location: [p.x / s, p.z / s, p.y / s],
        velocity: [v.x / s, v.z / s, v.y / s],
        yaw: (f.z.atan2(f.x) * 32768.0 / std::f32::consts::PI) as i32,
        physics: if g.ragdoll { 10 } else if g.wall.active { 15 } else if g.grounded { 1 } else { 2 },
        ragdoll: g.ragdoll,
        sprinting: g.sprinting,
        grabbed: g.grabbed.and_then(|(b, _)| {
            let name = phys.names.get(&b)?.clone();
            let shown = phys.prop_info.get(&b).map(|p| p.1.clone()).unwrap_or_else(|| name.clone());
            let t = crate::convert::from_rapier(phys.world.bodies.get(b)?.translation());
            let (i, gn) = b.into_raw_parts();
            Some(((i as u64) | ((gn as u64) << 32), name, shown, [t.x / s, t.z / s, t.y / s]))
        }),
    };
    // game events, from what changed on the goat since last frame
    use super::sync::GameEvent as E;
    let mut events = vec![];
    if prev.started {
        if g.grounded && !prev.grounded {
            events.push(E::Landed);
        }
        if g.ragdoll != prev.ragdoll {
            events.push(E::Ragdoll(g.ragdoll));
        }
        if g.wall.active != prev.wall {
            events.push(E::WallRunning(g.wall.active));
        }
        if g.stat_headbutts > prev.headbutts {
            events.push(E::UseAbility("GGAbilityHorn"));
        }
        if g.grabbed.is_some() && !prev.grabbing {
            events.push(E::UseAbility("GGAbilityBite"));
        }
        if prev.respawn && !g.pending_respawn {
            events.push(E::PlayerRespawn);
        }
        if g.manual.active != prev.manual {
            // StopManual(wasSuccessful): a ragdoll ended it badly
            events.push(E::Manual(g.manual.active, g.manual.active || !g.ragdoll));
        }
    }
    events.push(if g.grounded || g.ragdoll { E::Ground } else { E::Airborne(1.0) });
    for w in phys.world_events.drain(..) {
        events.push(match w {
            crate::physics::WorldEvent::Exploded { export, by_goat } => E::Exploded(export, by_goat),
            crate::physics::WorldEvent::Fractured { export, by_goat } => E::Fractured(export, by_goat),
        });
    }
    if input.0.baa {
        events.push(E::Baa);
    }
    use crate::score::TrickId as T;
    for t in &tricks.0 {
        let class = match t {
            T::Ability => "GGTrickAbility",
            T::Ragdoll => "GGTrickRagdoll",
            T::PerfectLanding => "GGTrickPerfectLanding",
            T::Rotation => "GGTrickRotation",
            T::JumpOver => "GGTrickJumpOver",
            T::WallRun => "GGTrickWallRun",
            T::WallJump => "GGTrickWallJump",
            T::Kismet => "GGTrickKismet",
            T::FracturingApex => "GGTrickFracturingApex",
            T::Explosion => "GGTrickExplosion",
            T::HurtByExplosion => "GGTrickHurtByExplosion",
            T::Collision => "GGTrickCollision",
            T::CollisionSpecial => "GGTrickCollisionSpecial",
            T::Bounce => "GGTrickBounce",
            T::Manual => "GGTrickManual",
            T::Baseball => "GGTrickBaseball",
        };
        if *t == T::WallJump {
            events.push(E::WallJump);
        }
        events.push(E::TrickMade(class));
    }
    *prev = Prev {
        grounded: g.grounded,
        ragdoll: g.ragdoll,
        wall: g.wall.active,
        headbutts: g.stat_headbutts,
        grabbing: g.grabbed.is_some(),
        respawn: g.pending_respawn,
        manual: g.manual.active,
        started: true,
    };
    let wheel = if scripted.is_some() { 0 } else { scroll.delta.y.round() as i32 };
    // keys (not while the mod menu has the keyboard)
    let mut held: Vec<String> = vec![];
    if let Some(si) = scripted {
        // test runs: the scripted input as the keys DefaultInput.ini binds to it
        for (on, k) in [
            (si.forward > 0.0, "W"),
            (si.forward < 0.0, "S"),
            (si.right > 0.0, "D"),
            (si.right < 0.0, "A"),
            (si.sprint, "LeftShift"),
            (si.jump, "SpaceBar"),
            (si.headbutt, "LeftMouseButton"),
            (si.lick, "E"),
            (si.ragdoll, "Q"),
            (si.baa, "one"),
            (si.freelook, "RightMouseButton"),
        ] {
            if on {
                held.push(k.into());
            }
        }
    } else if !crate::mods::menu_open() {
        held.extend(keys.get_pressed().filter_map(|k| ue_key(*k)));
        for (b, n) in [(MouseButton::Left, "LeftMouseButton"), (MouseButton::Right, "RightMouseButton"), (MouseButton::Middle, "MiddleMouseButton")] {
            if mouse.pressed(b) {
                held.push(n.into());
            }
        }
        // the pad's buttons by the names DefaultInput.ini binds (XboxTypeS_A -> GBA_Jump...)
        if let Some(p) = crate::pad::active(&pads) {
            held.extend(crate::pad::UE_BUTTONS.iter().filter(|(b, _)| p.pressed(*b)).map(|(_, n)| n.to_string()));
        }
    }
    // Trace/FastTrace see the physics world during this call only
    h.vm.tracer = Some(super::actors::tracer(&phys, g.body, actors.by_body.clone(), h.goat));
    h.vm.overlap = Some(super::actors::overlap(&phys, g.body));
    h.vm.bounds = Some(super::actors::bounds(&phys));
    // the game camera as PlayerCamera's view point (Unreal yaw/pitch from its forward)
    let cam = cams.single().ok().map(|(c, _)| {
        let f = c.rotation * -Vec3::Z;
        let k = 32768.0 / std::f32::consts::PI;
        let loc = c.translation;
        ([loc.x / s, loc.z / s, loc.y / s], [(f.y.clamp(-1.0, 1.0).asin() * k) as i32, (f.z.atan2(f.x) * k) as i32, 0])
    });
    held.extend(m.test_keys.iter().cloned());
    let Some(h) = m.host.as_mut() else { return };
    // nothing scripted runs this frame (no mutators, timers, delayed Kismet or state code):
    // skip the VM call; Kismet touch events still go through kismet_touch
    if h.idle() {
        h.vm.tracer = None;
        h.vm.overlap = None;
    h.vm.bounds = None;
        h.vm.bounds = None;
        forward_log(&mut m);
        return;
    }
    let changes = h.with_vm(move |h| {
        h.sync_in(state);
        if let Some((l, r)) = cam {
            h.sync_camera(l, r);
        }
        let refs: Vec<&str> = held.iter().map(|s| s.as_str()).collect();
        let t0 = std::time::Instant::now();
        h.keys(&refs);
        h.inventory(wheel);
        let t1 = std::time::Instant::now();
        for e in &events {
            h.event(e);
        }
        let t2 = std::time::Instant::now();
        if dt > 0.0 {
            h.tick_now(dt);
        }
        let t3 = std::time::Instant::now();
        if std::env::var_os("GOAT_PROFILE").is_some() && (t3 - t0).as_secs_f32() > 0.002 {
            println!("  vm frame: keys {:.2} ms, events {:.2} ms ({}), tick {:.2} ms", (t1 - t0).as_secs_f32() * 1e3, (t2 - t1).as_secs_f32() * 1e3, events.len(), (t3 - t2).as_secs_f32() * 1e3);
        }
        h.sync_out()
    });
    h.vm.tracer = None;
    h.vm.overlap = None;
    h.vm.bounds = None;
    // camera zoom set by a mutator (GGCameraModeOrbital): new limits + desired distance
    let zoom = h.camera_zoom();
    if zoom != h.last_zoom {
        if let (Some(z), Ok((_, mut gc))) = (zoom, cams.single_mut()) {
            let defaults = h.last_zoom.is_none();
            if !defaults || z[2] != crate::goat::cam::ZOOM {
                gc.zoom_desired = z[2] * s;
            }
            super::bridge::set_zoom(Some(z));
        }
        h.last_zoom = zoom;
    }
    if let Some(v) = changes.velocity {
        g.velocity = Vec3::new(v[0], v[2], v[1]) * s;
    }
    if let Some(l) = changes.location {
        let to = Vec3::new(l[0], l[2], l[1]) * s;
        tr.translation = to;
        if let Some(rb) = phys.world.bodies.get_mut(g.body) {
            rb.set_translation(crate::convert::to_rapier(to), true);
        }
    }
    // SetRagdoll / SetPhysics(PHYS_RigidBody) from script
    let want_ragdoll = changes.ragdoll.or(changes.physics.map(|p| p == 10));
    if let Some(r) = want_ragdoll {
        if r && !g.ragdoll {
            crate::goat::start_ragdoll(&mut commands, goat_entity, &mut g, &mut phys);
        } else if !r && g.ragdoll && g.ragdoll_time > 0.2 {
            g.ragdoll = false;
            commands.entity(goat_entity).insert(crate::ragdoll::StopRagdoll);
        }
    }
    forward_log(&mut m);
}


/// The VM log to mods/uvm_log.txt (and the console for top-level lines).
fn forward_log(m: &mut Mutators) {
    let sent = m.log_sent;
    let Some(h) = m.host.as_ref() else { return };
    let total = h.vm.log.len();
    let sent = sent.min(total);
    if total > sent {
        let path = crate::mods::mods_dir().join("uvm_log.txt");
        let mut text = if sent > 0 { std::fs::read_to_string(&path).unwrap_or_default() } else { String::new() };
        for l in &h.vm.log[sent..] {
            text.push_str(l);
            text.push('\n');
            if !l.starts_with(' ') && !l.starts_with("->") {
                info!("[uvm] {l}");
            }
        }
        let _ = std::fs::write(&path, text);
    }
    m.log_sent = total;
}
/// Material swaps on the goat mesh (SetMaterial from mutator script).
#[allow(clippy::too_many_arguments)]
fn apply_events(
    mut m: ResMut<Mutators>,
    goats: Query<Entity, With<crate::goat::Goat>>,
    children: Query<&Children>,
    mut mats: Query<&mut MeshMaterial3d<StandardMaterial>, With<bevy::mesh::skinning::SkinnedMesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut sfx: ResMut<crate::audio::Sfx>,
    mut audio: ResMut<Assets<bevy::audio::AudioSource>>,
    mut queue: ResMut<crate::audio::SoundQueue>,
    rigs: Query<&crate::rig::Rig, With<crate::goat::Goat>>,
    mut commands: Commands,
    roots: Query<(Entity, &Name, &ChildOf)>,
    mut cache: Local<crate::rig::SkelCache>,
    mut script_actors: ResMut<super::actors::ScriptActors>,
) {
    let _prof_scope = crate::prof::scope("plugin::apply_events");
    let mut new_messages = vec![];
    let mut new_achievements: Vec<String> = vec![];
    let mut new_scores = vec![];
    let Some(h) = m.host.as_mut() else { return };
    // the goat's real skeleton answers MatchRefBone/GetBoneName
    if h.vm.bones.is_empty() {
        if let Ok(rig) = rigs.single() {
            h.vm.bones = rig.data.bone_names.clone();
        }
    }
    if h.vm.events.is_empty() {
        return;
    }
    let events: Vec<HostEvent> = h.vm.events.drain(..).collect();
    let mut m_material = h.goat_material.clone();
    let goat_mesh = h.goat.and_then(|g| h.vm.get(g, "Mesh").obj());
    for ev in events {
        match ev {
            HostEvent::SetMaterial { component, index, material } => {
                if component != goat_mesh || index != 0 {
                    continue;
                }
                // "Material'goat.Materials.Goat_Mat_08'" -> object path
                let path = material.split('\'').nth(1).unwrap_or(&material).to_string();
                let Some(r) = h.vm.reg.find_path(&path) else {
                    warn!("[uvm] material {path} is not in the loaded packages");
                    continue;
                };
                let mut sa = crate::rig::SkelAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes };
                let handle = crate::rig::material_for(&h.vm.reg, r, &mut sa);
                m_material = Some(handle.clone());
                for g in &goats {
                    for root in children.get(g).into_iter().flatten() {
                        for part in children.get(*root).into_iter().flatten() {
                            if let Ok(mut mm) = mats.get_mut(*part) {
                                mm.0 = handle.clone();
                            }
                        }
                    }
                }
                info!("[uvm] goat material -> {path}");
            }
            HostEvent::SetSkeletalMesh { component, mesh } if component == goat_mesh => {
                // swap the goat's mesh, keeping its animations, physics asset and script material
                let path = mesh.split('\'').nth(1).unwrap_or(&mesh).to_string();
                let reg = &h.vm.reg;
                let Some(mesh_ref) = reg.find_path(&path) else {
                    warn!("[uvm] skeletal mesh {path} is not in the loaded packages");
                    continue;
                };
                let anim = reg.find_path("goat.Anim.Goat_Anim_01");
                let pa = reg.find_path("goat.Mesh.goat_Physics");
                let mut sa = crate::rig::SkelAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes };
                let data = match crate::rig::load(reg, &mut cache, mesh_ref, anim, pa, &mut sa) {
                    Ok(d) => d,
                    Err(e) => {
                        warn!("[uvm] goat mesh {path}: {e:#}");
                        continue;
                    }
                };
                for g in &goats {
                    for (e, name, parent) in &roots {
                        if parent.parent() == g && name.as_str() == "MeshRoot" {
                            commands.entity(e).despawn();
                        }
                    }
                    commands.entity(g).remove::<(crate::rig::Rig, crate::rig::Animator)>();
                    let overrides = [m_material.clone()];
                    crate::rig::spawn(&mut commands, g, data.clone(), crate::rig::goat_mesh_xf(), &overrides, "Idle");
                }
                h.vm.bones = data.bone_names.clone();
                info!("[uvm] goat mesh -> {path} ({} bones)", data.bone_names.len());
            }
            HostEvent::SetSkeletalMesh { .. } => {}
            HostEvent::AttachToSocket { component, mesh, socket } => {
                script_actors.attach_queue.push((component, mesh, socket));
            }
            HostEvent::Detach(comp) => script_actors.detach_queue.push(comp),
            HostEvent::Fx(cmd) => script_actors.fx_queue.push(cmd),
            HostEvent::Mover { name, location, rotation, initial } => script_actors.mover_queue.push((name, location, rotation, initial)),
            HostEvent::MapActorHidden { name, hidden } => script_actors.hidden_queue.push((name, hidden)),
            HostEvent::MaterialParam(mic) => script_actors.tint_queue.push(mic),
            HostEvent::Impulse { actor, impulse, velocity_change } => script_actors.impulses.push((actor, impulse, velocity_change)),
            HostEvent::PlaySound { cue } => {
                let path = cue.split('\'').nth(1).unwrap_or(&cue).to_string();
                if !sfx.cues.contains_key(&path) {
                    crate::audio::load_cue(&h.vm.reg, &mut sfx, &mut audio, &path, &path);
                }
                if sfx.cues.contains_key(&path) {
                    queue.play(&path, None);
                }
            }
            HostEvent::KismetScore { name, score, special } => new_scores.push((name, score, special)),
            HostEvent::Console(cmd) => {
                let lower = cmd.trim().to_ascii_lowercase();
                // Exit Game in the pause menu ends in ConsoleCommand("exit")
                if matches!(lower.as_str(), "exit" | "quit") {
                    super::menu_actions::push_action(super::menu_actions::MenuAction::Quit);
                } else if lower.starts_with("start ") || lower.starts_with("open ") {
                    // keep the URL's case (raceID=BikeRace is compared case-sensitively)
                    let url = cmd.trim().split_once(' ').map(|x| x.1).unwrap_or("");
                    // Custom Game -> Play!: the map URL and the mutators chosen in the viewport's
                    // mMutatorMap (GGUIViewCustomGame.SetupChosenMutators)
                    let mutators = h.chosen_mutators();
                    super::menu_actions::push_action(super::menu_actions::MenuAction::Start { url: url.trim().to_string(), mutators });
                } else {
                    if let Some(res) = lower.strip_prefix("setres ") {
                        // Options -> Apply: "setres 1920x1080f" (f = fullscreen, w = windowed)
                        super::settings::request_setres(res.trim());
                    } else {
                        info!("[uvm] console command from script not run: {cmd}");
                    }
                }
            }
            HostEvent::Message(text) => {
                if !text.is_empty() {
                    new_messages.push(text);
                }
            }
            HostEvent::Achievement(id) => {
                info!("[uvm] achievement unlocked: {id}");
                new_achievements.push(id);
            }
        }
    }
    if let Some(h) = m.host.as_mut() {
        h.goat_material = m_material;
    }
    m.kismet_scores.extend(new_scores);
    m.achievements.extend(new_achievements);
    for t in new_messages {
        info!("[uvm] message: {t}");
        m.messages.push((t, 6.0));
    }
}

#[derive(Component)]
struct MessageText;

fn spawn_messages(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont { font_size: bevy::text::FontSize::Px(22.0), ..default() },
        TextColor(Color::srgb(1.0, 0.95, 0.6)),
        Node { position_type: PositionType::Absolute, left: Val::Px(16.0), bottom: Val::Px(90.0), ..default() },
        MessageText,
    ));
}

/// Broadcast / ClientMessage text from mutator scripts, 6 s each, newest last.
fn show_messages(time: Res<Time<Real>>, mut m: ResMut<Mutators>, mut q: Query<&mut Text, With<MessageText>>, mut score: ResMut<crate::score::Score>) {
    let dt = time.delta_secs();
    // GGTrickKismet: secrets and stunts the map's Kismet rewards
    for (name, s, special) in std::mem::take(&mut m.kismet_scores) {
        info!("[uvm] kismet trick {name} +{s}");
        score.report(crate::score::Trick {
            id: crate::score::TrickId::Kismet,
            name: name.clone(),
            clean_name: name.clone(),
            score: s,
            adds_multiplier: true,
            has_name: true,
            min_score: s,
        });
        m.kismet_scored += 1;
        // special scores get the HUD's juice from the script itself (GGComboManager.AddHUDJuice)
        let _ = special;
    }
    m.messages.retain_mut(|(_, t)| {
        *t -= dt;
        *t > 0.0
    });
    let n = m.messages.len();
    let shown: Vec<&str> = m.messages[n.saturating_sub(5)..].iter().map(|(s, _)| s.as_str()).collect();
    let text = shown.join("\n");
    for mut t in &mut q {
        if t.0 != text {
            t.0 = text.clone();
        }
    }
}

/// SkelControlSingleBone BoneScale set by scripts (AddSkelControl) on the goat's bones.
fn apply_bone_scales(m: Res<Mutators>, rigs: Query<&crate::rig::Rig, With<crate::goat::Goat>>, mut joints: Query<&mut Transform>, mut scaled: Local<Vec<usize>>) {
    let Some(h) = m.host.as_ref() else { return };
    let Ok(rig) = rigs.single() else { return };
    let mut now = vec![];
    for (bone, scale) in h.bone_scales() {
        let Some(i) = rig.data.bone_names.iter().position(|b| b.eq_ignore_ascii_case(&bone)) else { continue };
        if let Some(mut t) = rig.joints.get(i).and_then(|&e| joints.get_mut(e).ok()) {
            t.scale = Vec3::splat(scale);
        }
        now.push(i);
    }
    for &i in scaled.iter().filter(|i| !now.contains(i)) {
        if let Some(mut t) = rig.joints.get(i).and_then(|&e| joints.get_mut(e).ok()) {
            t.scale = Vec3::ONE;
        }
    }
    *scaled = now;
}
