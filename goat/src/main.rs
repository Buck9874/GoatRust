//! Goat Simulator rebuilt in Rust. Loads the user's own installed copy of the game.
//!
//! goat [--map Maps/Level_01.goat] [--test <name> --shot <png>]

mod air;
mod ambient;
mod audio;
mod convert;
mod chain;
mod race;
mod pitch;
mod water;
mod vehicle;
mod manual;
mod matbake;
mod destruct;
mod explosive;
mod fire;
mod goat;
mod landscape;
mod lightmap;
mod look;
mod mods;
mod particles;
mod npc;
mod physics;
mod post;
mod prof;
mod ragdoll;
mod rider;
mod rig;
mod score;
mod shapes;
mod test;
mod test_mods;
mod test_mutators;
mod test_menu_flow;
mod test_menus;
mod test_npc;
mod test_destruct;
mod test_quests;
mod test_tricks;
mod uvm;
mod world;

use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::prelude::*;
use bevy::window::WindowPosition;
use goat::{GoatCamera, InputState};

#[derive(Resource, Clone)]
pub struct Args {
    pub map: String,
    pub test: Option<String>,
    pub shot: Option<String>,
    pub start: usize,
    /// Test runs open on the second monitor unless `--main-monitor` is given.
    pub main_monitor: bool,
    /// Boot into the original main menu (a plain launch; `--no-menu`, `--map` and tests skip it,
    /// `--test mainmenu` uses it).
    pub menu: bool,
}

fn parse_args() -> Args {
    let a: Vec<String> = std::env::args().collect();
    let get = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).cloned();
    Args {
        // the wall-run test needs Goat City Bay's tall buildings
        map: get("--map").unwrap_or_else(|| {
            if matches!(get("--test").as_deref(), Some("wallrun" | "attack" | "route" | "longboard" | "water" | "race" | "racepopup" | "bikerace")) { "Maps/Level_02.goat" } else { "Maps/Level_01.goat" }.into()
        }),
        test: get("--test"),
        shot: get("--shot"),
        start: get("--start").and_then(|s| s.parse().ok()).unwrap_or(0),
        main_monitor: a.iter().any(|x| x == "--main-monitor"),
        menu: (get("--test").is_none() && get("--map").is_none() && !a.iter().any(|x| x == "--no-menu")) || get("--test").as_deref() == Some("mainmenu"),
    }
}

fn main() {
    // headless UnrealScript run of downloaded Workshop mutators: goat --uvm [item ids]
    let raw: Vec<String> = std::env::args().collect();
    if let Some(i) = raw.iter().position(|a| a == "--uvm") {
        if let Err(e) = uvm::bridge::run_cli(&raw[i + 1..]) {
            eprintln!("uvm: {e}");
            std::process::exit(1);
        }
        return;
    }
    // unlocked achievements: this game's save + the player's Steam account (read-only)
    if raw.iter().any(|a| a == "--achievements") {
        for (k, v) in uvm::progress::achievements() {
            println!("{k} ({})", uvm::progress::achievement_title(&k));
            let _ = v;
        }
        println!("({} from Steam)", uvm::progress::steam_achievements().len());
        return;
    }
    // headless pause menu, clicking widgets by label: goat --uvm-menu-click Options,key:Escape [out.png]
    if let Some(i) = raw.iter().position(|a| a == "--uvm-menu-click") {
        if let Err(e) = uvm::menu_actions::run_cli(&raw[i + 1..]) {
            eprintln!("uvm-menu-click: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    // headless Scaleform menu through the scripts: goat --uvm-menu [pause] [out.png]
    if let Some(i) = raw.iter().position(|a| a == "--uvm-menu") {
        if let Err(e) = uvm::menus::run_cli(&raw[i + 1..]) {
            eprintln!("uvm-menu: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let args = parse_args();
    let testing = args.test.is_some();
    // `--test race` runs Goat City Bay's RegularRace (the options a race popup passes along)
    let race_test = match args.test.as_deref() {
        Some("race") => Some("RegularRace"),
        Some("bikerace") => Some("BikeRace"),
        _ => None,
    };
    if let Some(id) = race_test.filter(|_| std::env::var_os("GOAT_GAME_OPTIONS").is_none()) {
        // SAFETY: set before any other thread starts
        unsafe { std::env::set_var("GOAT_GAME_OPTIONS", format!("?game=GoatGame.GGGameInfoRace?SkipIntro?raceID={id}")) };
    }
    // test runs never touch the player's save (progression, achievements, settings)
    if testing && std::env::var_os("GOAT_SAVE_DIR").is_none() {
        // SAFETY: set before any other thread starts
        unsafe { std::env::set_var("GOAT_SAVE_DIR", "out/test_save") };
    }
    let window = Window {
        title: "Goat Simulator (Rust)".into(),
        resolution: (1600, 900).into(),
        // automated runs are moved to the second monitor (see test.rs) and never take focus
        position: WindowPosition::Automatic,
        focused: !testing,
        visible: !testing,
        // GOAT_NO_VSYNC=1: uncapped frame rate (performance checks)
        present_mode: if std::env::var_os("GOAT_NO_VSYNC").is_some() { bevy::window::PresentMode::AutoNoVsync } else { bevy::window::PresentMode::AutoVsync },
        ..default()
    };
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(window), ..default() }));
    // GOAT_GPU_TIMING=1: GPU time per render pass (printed at the end of a test)
    if std::env::var_os("GOAT_GPU_TIMING").is_some() {
        app.add_plugins(bevy::render::diagnostic::RenderDiagnosticsPlugin);
    }
    app
        .insert_resource(args.clone())
        .insert_resource(ClearColor(Color::srgb(0.55, 0.72, 0.92)))
        .insert_resource(GlobalAmbientLight { color: Color::srgb(0.75, 0.82, 1.0), brightness: 900.0, affects_lightmapped_meshes: false, ..default() })
        .insert_resource(DirectionalLightShadowMap { size: 4096 })
        .init_resource::<InputState>()
        .init_resource::<audio::SoundQueue>()
        .init_resource::<audio::SoundLog>()
        .init_resource::<score::Score>()
        .init_resource::<score::TrickEvents>()
        .add_plugins(physics::PhysicsPlugin)
        .add_plugins(landscape::LandscapePlugin)
        .add_plugins(mods::ModsPlugin)
        .add_plugins(uvm::plugin::MutatorPlugin)
        .add_plugins(particles::ParticlesPlugin)
        .add_plugins(chain::ChainPlugin)
        .add_plugins(destruct::DestructPlugin)
        .add_plugins(explosive::ExplosivePlugin)
        .add_plugins(ambient::AmbientPlugin)
        .add_plugins(fire::FirePlugin)
        .add_plugins(look::LookPlugin)
        .add_plugins(post::PostPlugin)
        .add_plugins(rider::RiderPlugin)
        .add_plugins(vehicle::VehiclePlugin)
        .add_plugins(water::WaterPlugin)
        .add_plugins(pitch::PitchPlugin)
        .add_plugins(race::RacePlugin)
        .add_systems(PostUpdate, ragdoll::drive.after(physics::sync).before(bevy::transform::TransformSystems::Propagate))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                goat::read_input,
                goat::goat_abilities,
                goat::goat_move,
                rig::choose_anim,
                npc::update,
                rig::animate,
                goat::goat_footsteps,
                goat::ragdoll_impacts,
                npc::footsteps,
                ragdoll::begin,
                ragdoll::end,
                goat::camera_control,
                goat::grab_cursor,
                audio::play_queued,
                score::tick,
                hud,
            )
                .chain(),
        );
    if testing {
        test::add(&mut app);
    }
    app.run();
}

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct ScoreText;

fn setup(
    mut commands: Commands,
    args: Res<Args>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
    mut bindposes: ResMut<Assets<bevy::mesh::skinning::SkinnedMeshInverseBindposes>>,
    mut phys: ResMut<physics::Physics>,
    mut audio_assets: ResMut<Assets<bevy::audio::AudioSource>>,
    mut land_mats: ResMut<Assets<landscape::LandscapeMaterial>>,
) {
    let t0 = std::time::Instant::now();
    let path = upk::cooked_dir().join(&args.map);
    // a map from outside the game (Workshop item: <item>/Maps/x.goat) brings its own texture cache
    if std::path::Path::new(&args.map).is_absolute() {
        if let Some(item) = path.parent().and_then(|p| p.parent()) {
            upk::texture::add_tfc_dir(item);
        }
    }
    let pkg = match upk::Package::open(&path) {
        Ok(p) => p,
        Err(e) => {
            error!("cannot open {}: {e:#}. Set GOATSIM_DIR to your Goat Simulator install.", path.display());
            std::process::exit(2);
        }
    };
    // map first, then the script package and Startup (shared characters, sounds, effects)
    let mut pkgs = vec![pkg];
    // Engine.u last: the engine's default objects (particle module defaults...)
    for extra in ["GoatGame.u", "Startup.upk", "Engine.u"] {
        match upk::Package::open(upk::cooked_dir().join(extra)) {
            Ok(p) => pkgs.push(p),
            Err(e) => warn!("{extra}: {e:#}"),
        }
    }
    let reg = upk::registry::Registry::new(pkgs);
    info!("packages loaded in {:?}", t0.elapsed());
    let loaded = match world::load(&reg, &mut commands, &mut meshes, &mut materials, &mut images, &mut land_mats, &mut phys) {
        Ok(l) => l,
        Err(e) => {
            error!("level load failed: {e:#}");
            std::process::exit(3);
        }
    };
    let spawn = if std::env::var("GOAT_SPAWN").is_ok() { loaded.spawn } else { loaded.level.player_starts.get(args.start).map(convert::xform).unwrap_or(loaded.spawn) };
    // GGGameInfoRace: a GGPlayerStartRace of this race
    let spawn = match uvm::race::race_id() {
        Some(id) => loaded.level.race_starts.iter().find(|(r, _)| *r == id).map(|(_, x)| convert::xform(x)).unwrap_or(spawn),
        None => spawn,
    };

    // The sun: the map's DominantDirectionalLight (direction, Brightness, LightColor), at the
    // lightmaps' brightness scale. Its direct light is dynamic in UE3 too (lightmaps hold the rest).
    let (sun_rot, sun_illum, sun_color) = loaded
        .level
        .lights
        .iter()
        .find(|(c, _, _)| c == "DominantDirectionalLightComponent")
        .map(|(_, xf, ps)| {
            let b = match upk::props::find(ps, "Brightness") {
                Some(upk::props::Value::Float(f)) => *f,
                _ => 1.0,
            };
            // FColor is stored B, G, R, A
            let col = match upk::props::find(ps, "LightColor") {
                Some(upk::props::Value::Color(c)) => Color::srgb_u8(c[2], c[1], c[0]),
                _ => Color::WHITE,
            };
            let dir = convert::rotator_dir(xf.rotation);
            (Transform::default().looking_to(dir, if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y }).rotation, lightmap::sun_illuminance(b), col)
        })
        .unwrap_or((Quat::from_euler(EulerRot::YXZ, 0.8, -0.9, 0.0), 12000.0, Color::srgb(1.0, 0.96, 0.88)));
    info!("sun: {sun_illum:.0} lx {:?} dir {:.2}", sun_color.to_srgba(), sun_rot * Vec3::NEG_Z);
    commands.spawn((
        DirectionalLight { illuminance: sun_illum, shadow_maps_enabled: std::env::var("GOAT_SHADOWS").map(|v| v != "0").unwrap_or(true), color: sun_color, ..default() },
        Transform::from_rotation(sun_rot),
        CascadeShadowConfigBuilder {
            // far enough for the whole town: moving things away from the camera still get the buildings' shadow
            // (UE3 used the cooked DominantLightShadowMap for them, ~4 m cells)
            maximum_distance: std::env::var("GOAT_SHADOW_DIST").ok().and_then(|v| v.parse().ok()).unwrap_or(450.0),
            first_cascade_far_bound: 12.0,
            ..default()
        }
        .build(),
    ));

    let goat = goat::spawn_goat(&mut commands, &mut phys, spawn);
    let mut cache = rig::SkelCache::default();
    let mut sa = rig::SkelAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes };
    // The goat's skeletal mesh, animations and ragdoll live in the script package.
    let goat_rig = (|| {
        let mesh = reg.find_path("goat.Mesh.goat").ok_or_else(|| anyhow::anyhow!("goat.Mesh.goat not found"))?;
        let anim = reg.find_path("goat.Anim.Goat_Anim_01");
        let pa = reg.find_path("goat.Mesh.goat_Physics");
        rig::load(&reg, &mut cache, mesh, anim, pa, &mut sa)
    })();
    match goat_rig {
        Ok(data) => rig::spawn(&mut commands, goat, data, rig::goat_mesh_xf(), &[], "Idle"),
        Err(e) => {
            warn!("goat mesh unavailable ({e:#}); using a box");
            let body = (
                Mesh3d(meshes.add(Cuboid::new(0.28, 0.5, 0.75))),
                MeshMaterial3d(materials.add(StandardMaterial { base_color: Color::srgb(0.95, 0.94, 0.9), ..default() })),
            );
            commands.entity(goat).with_children(|c| {
                c.spawn(body);
            });
        }
    }
    let mut sa = rig::SkelAssets { meshes: &mut meshes, materials: &mut materials, images: &mut images, bindposes: &mut bindposes };
    let n_npc = npc::spawn_all(&reg, &mut cache, &mut sa, &mut commands, &mut phys);
    let n_bikes = vehicle::spawn_all(&reg, &mut cache, &mut sa, &mut commands, &mut phys);
    let pitching = pitch::load(&reg, &mut cache, &mut sa, &mut commands, reg.find_path("goat.Mesh.goat"));
    commands.insert_resource(pitching);
    info!("bicycles: {n_bikes}");

    // sound cues used by GGGoat / GGNpc (test runs stay silent)
    let mut sfx = audio::Sfx { muted: args.test.is_some(), ..default() };
    for (key, path) in [
        ("baa", "Goat_Sounds.Cue.GoatSound_01"),
        ("jump", "Goat_Sounds.Cue.Goat_Jump_Cue"),
        ("land", "Goat_Sounds.Cue.Goat_Land_Cue"),
        // GGGoat.mBrakeCue
        ("brake", "Goat_Sounds.Cue.Goat_Slide_Loop_Cue"),
    ] {
        audio::load_cue(&reg, &mut sfx, &mut audio_assets, key, path);
    }
    audio::load_cue_list(&reg, &mut sfx, &mut audio_assets, "npc_knocked", "Default__GGNpc", "mKnockedOverSounds");
    audio::load_cue_list(&reg, &mut sfx, &mut audio_assets, "npc_panic", "Default__GGNpc", "mPanicSounds");
    audio::load_cue_list(&reg, &mut sfx, &mut audio_assets, "npc_applaud", "Default__GGNpc", "mApplaudSounds");
    audio::load_cue_list(&reg, &mut sfx, &mut audio_assets, "npc_angry", "Default__GGNpc", "mAngrySounds");
    audio::load_cue_list(&reg, &mut sfx, &mut audio_assets, "npc_notice", "Default__GGNpc", "mNoticeGoatSounds");
    // footsteps by surface (GGPawnSoundGroup) and every prop/surface ImpactSound (keyed by path)
    audio::load_footsteps(&reg, &mut sfx, &mut audio_assets, "foot:", "FootstepSounds", "DefaultFootstepSound");
    audio::load_footsteps(&reg, &mut sfx, &mut audio_assets, "npcfoot:", "HumanNPCFootstepSounds", "DefaultHumanNPCFootstepSound");
    audio::load_cue(&reg, &mut sfx, &mut audio_assets, "foot:WaterRun", "Goat_Sounds.Cue.Goat_Run_Water_Cue");
    let impact_cues: std::collections::BTreeSet<String> = phys.surfaces.iter().filter_map(|s| s.impact.clone()).collect();
    for c in &impact_cues {
        audio::load_cue(&reg, &mut sfx, &mut audio_assets, c, c);
    }
    info!("{} surfaces, {} impact cues", phys.surfaces.len(), impact_cues.len());
    // AnimNotify_Sound cues of every loaded animation
    let anim_cues: std::collections::BTreeSet<String> = cache.0.values().flat_map(|d| d.clips.iter().flat_map(|c| c.sounds.iter().map(|s| s.1.clone()))).collect();
    for c in &anim_cues {
        audio::load_cue(&reg, &mut sfx, &mut audio_assets, c, c);
    }
    info!("{} animation sound cues", anim_cues.len());
    info!("sound cues: {:?}", sfx.cues.iter().map(|(k, v)| (k.clone(), v.len())).collect::<Vec<_>>());
    commands.insert_resource(sfx);
    info!("spawned {n_npc} NPCs ({} skeletal assets) in {:?} total", cache.0.len(), t0.elapsed());

    let (yaw, _, _) = spawn.rotation.to_euler(EulerRot::YXZ);
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 70f32.to_radians(), near: 0.05, far: 4000.0, ..default() }),
        DistanceFog {
            color: Color::srgba(0.62, 0.75, 0.9, 1.0),
            falloff: FogFalloff::Linear { start: 250.0, end: 1800.0 },
            ..default()
        },
        GoatCamera::new(yaw, spawn.translation),
        Transform::from_translation(spawn.translation + Vec3::new(0.0, 3.0, 6.0)),
    ));

    commands.spawn((
        Text::new(""),
        TextFont { font_size: bevy::text::FontSize::Px(16.0), ..default() },
        TextColor(Color::WHITE),
        Node { position_type: PositionType::Absolute, top: px(10), left: px(12), ..default() },
        HudText,
    ));
    commands.spawn((
        Text::new(""),
        TextFont { font_size: bevy::text::FontSize::Px(22.0), ..default() },
        TextColor(Color::srgb(1.0, 0.95, 0.6)),
        TextLayout::justify(Justify::Right),
        Node { position_type: PositionType::Absolute, top: px(10), right: px(16), ..default() },
        ScoreText,
    ));
    info!("spawned at {:?} ({} starts)", spawn.translation, loaded.level.player_starts.len());
    // Workshop mutator props/hats can live in the map or Startup.upk
    commands.insert_resource(uvm::actors::GameRegistry(std::sync::Arc::new(reg)));
}

fn hud(
    mut q: Query<&mut Text, (With<HudText>, Without<ScoreText>)>,
    mut sq: Query<&mut Text, (With<ScoreText>, Without<HudText>)>,
    mut hn: Query<&mut Node, With<HudText>>,
    score: Res<score::Score>,
    goats: Query<(&goat::Goat, &Transform)>,
    diag: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    args: Res<Args>,
    mut debug_on: Local<Option<bool>>,
) {
    let _prof_scope = crate::prof::scope("main::hud");
    // the developer overlay: F1 toggles it; off by default when playing (GOAT_DEBUG_HUD=1 turns
    // it on), on in test runs
    let on = debug_on.get_or_insert_with(|| args.test.is_some() || std::env::var_os("GOAT_DEBUG_HUD").is_some());
    if keys.just_pressed(KeyCode::F1) {
        *on = !*on;
    }
    let on = *on;
    // the game's own Scaleform HUD shows score and combos when it runs (score top left): the
    // debug text moves to the bottom left
    let game_hud = uvm::menus::hud_shown();
    // nothing of the debug overlay over the main menu / pause menu (or at all with GOAT_CLEAN=1,
    // for screenshots and clips)
    let menu = uvm::menus::ui_open() || std::env::var_os("GOAT_CLEAN").is_some() || !on;
    if let Ok(mut node) = hn.single_mut() {
        let (top, bottom) = if game_hud { (Val::Auto, px(10)) } else { (px(10), Val::Auto) };
        if node.top != top {
            node.top = top;
            node.bottom = bottom;
        }
        let display = if menu { Display::None } else { Display::Flex };
        if node.display != display {
            node.display = display;
        }
    }
    if let Ok(mut st) = sq.single_mut() {
        if (game_hud || menu) && !st.is_empty() {
            **st = String::new();
        }
    }
    if menu {
        return;
    }
    if let (false, Ok(mut st)) = (game_hud, sq.single_mut()) {
        let mut s = format!("SCORE {}
", score.total);
        if score.in_combo() {
            for l in &score.lines {
                s += &format!("{} +{}
", l.text, l.score);
            }
            s += &format!("x{:.1}   = {}
", score.multiplier, score.combo_total());
        } else if let Some((c, _)) = score.last_combo {
            s += &format!("combo +{c}
");
        }
        **st = s;
    }
    let Ok(mut t) = q.single_mut() else { return };
    let Ok((g, tr)) = goats.single() else { return };
    let speed = Vec2::new(g.velocity.x, g.velocity.z).length() / convert::S;
    **t = format!(
        "WASD move  Shift sprint  Space jump (x2)  LMB headbutt  E lick  Q ragdoll  1 baa  F slow-mo  R use bike / respawn  Esc mouse\n\
         speed {:>5.0} uu/s  {}  headbutts {}  fps {:.0}\npos {:.1} {:.1} {:.1}",
        speed,
        if g.ragdoll { "RAGDOLL" } else if g.grounded { "grounded" } else { "airborne" },
        g.stat_headbutts,
        1.0 / diag.delta_secs().max(1e-4),
        tr.translation.x,
        tr.translation.y,
        tr.translation.z
    );
}
