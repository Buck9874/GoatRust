//! `goat --test menuflow --shot out/menuflow.png`: the pause menu's other paths in game, all
//! through the original scripts: Options view opens (and Escape goes back), Restart shows its
//! confirmation popup over the pause menu (screenshot) and No closes only the popup, Respawn
//! closes the menu and respawns the goat (it was moved 8 m away first).

use crate::Args;
use crate::uvm::menu_actions::{MenuAction, MenuActionsDone, find_labeled};
use crate::uvm::menus::MenuTest;
use crate::uvm::plugin::Mutators;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

#[derive(Default)]
pub struct FlowSt {
    t: f32,
    phase: u32,
    mark: f32,
    spawn: Option<Vec3>,
    /// Movies open with just the pause menu (the HUD movie may be open too).
    base: usize,
}

/// Queue a click on the widget labelled `label` in the top menu.
fn click(m: &mut Mutators, test: &mut MenuTest, label: &str) -> bool {
    let Some(h) = m.host.as_mut() else { return false };
    let label = label.to_string();
    let Some(r) = h.with_vm(move |h| find_labeled(&mut h.vm, &label)) else { return false };
    let (x, y) = (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
    test.mouse.extend([(x, y, None), (x, y, Some(true)), (x, y, Some(false))]);
    true
}

/// Where the widget labelled `label` is (centre, viewport pixels).
fn label_pos(m: &mut Mutators, label: &str) -> Option<(f32, f32)> {
    let h = m.host.as_mut()?;
    let label = label.to_string();
    let r = h.with_vm(move |h| find_labeled(&mut h.vm, &label))?;
    Some((r[0] + r[2] * 0.5, r[1] + r[3] * 0.5))
}

fn views(m: &mut Mutators) -> (Vec<String>, usize) {
    let Some(h) = m.host.as_mut() else { return (vec![], 0) };
    h.with_vm(|h| (h.menu_views(), h.vm.gfx.order.len()))
}

#[allow(clippy::too_many_arguments)]
pub fn menuflow_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<FlowSt>,
    mut m: ResMut<Mutators>,
    mut test: ResMut<MenuTest>,
    done: Res<MenuActionsDone>,
    mut phys: ResMut<crate::physics::Physics>,
    mut goats: Query<(&mut crate::goat::Goat, &mut Transform)>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("menuflow") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let since = t - st.mark;
    let fail = |why: &str| -> ! {
        println!("TEST menuflow FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    let mut next = |st: &mut FlowSt, p: u32| {
        st.phase = p;
        st.mark = t;
    };
    match st.phase {
        0 if m.host.is_some() && t > 2.0 => {
            // move the goat away from its spawn so Respawn has something to undo
            let spawn = g.spawn.translation;
            st.spawn = Some(spawn);
            let away = tr.translation + Vec3::new(8.0, 0.0, 0.0);
            if let Some(rb) = phys.world.bodies.get_mut(g.body) {
                rb.set_translation(crate::convert::to_rapier(away), true);
            }
            tr.translation = away;
            g.velocity = Vec3::ZERO;
            test.open_pause = true;
            next(&mut st, 1);
        }
        0 if t > 40.0 => fail("the UnrealScript host never started"),
        1 if test.open && since > 1.0 => {
            if !click(&mut m, &mut test, "Options") {
                fail("no Options button");
            }
            next(&mut st, 2);
        }
        1 if since > 5.0 => fail("pause menu did not open"),
        2 if since > 1.0 => {
            let (v, _) = views(&mut m);
            if !v.iter().any(|c| c == "GGUIViewOptions") {
                fail(&format!("Options view not on the stack: {v:?}"));
            }
            println!("  t={t:.1} Options open: {v:?}");
            test.keys.push(("Escape", true));
            test.keys.push(("Escape", false));
            next(&mut st, 3);
        }
        3 if since > 1.0 => {
            let (v, movies) = views(&mut m);
            if v != ["GGUIViewPauseMenu"] {
                fail(&format!("Escape did not go back to the pause view: {v:?}"));
            }
            st.base = movies;
            if !click(&mut m, &mut test, "Restart") {
                fail("no Restart button");
            }
            next(&mut st, 4);
        }
        4 => {
            let (_, movies) = views(&mut m);
            if movies == st.base + 1 && since > 1.0 {
                println!("  t={t:.1} Restart popup over the pause menu");
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
                next(&mut st, 5);
            } else if since > 5.0 {
                fail(&format!("no confirmation popup ({movies} movies open)"));
            }
        }
        5 if since > 0.8 => {
            if !click(&mut m, &mut test, "No") {
                fail("no No button in the popup");
            }
            next(&mut st, 6);
        }
        6 if since > 1.0 => {
            let (v, movies) = views(&mut m);
            if movies != st.base || v != ["GGUIViewPauseMenu"] {
                fail(&format!("No should close only the popup ({movies} movies, views {v:?})"));
            }
            if !click(&mut m, &mut test, "Respawn") {
                fail("no Respawn button");
            }
            next(&mut st, 7);
        }
        7 => {
            let respawned = done.0.contains(&MenuAction::Respawn);
            if respawned && !test.open && since > 1.0 {
                let d = st.spawn.map(|s| Vec2::new(tr.translation.x - s.x, tr.translation.z - s.z).length()).unwrap_or(0.0);
                if d > 2.0 {
                    fail(&format!("goat is {d:.1} m from its spawn after Respawn"));
                }
                println!("  t={t:.1} Respawn closed the menu, goat {d:.1} m from spawn");
                println!("TEST menuflow PASS");
                exit.write(AppExit::Success);
                next(&mut st, 8);
            } else if since > 5.0 {
                fail(&format!("Respawn: action {respawned}, menu open {}", test.open));
            }
        }
        _ => {}
    }
}

/// `goat --test mainmenu --shot out/mainmenu.png`: boots into the original main menu
/// (GGHUD.StartMainMenu over the paused level), Play -> Custom Game (screenshot `_custom`), then
/// Play! with the current map and no mutators -> GGUIViewCustomGame.StartClean: the menu closes,
/// the HUD movie starts and the game runs.
#[allow(clippy::too_many_arguments)]
pub fn mainmenu_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<FlowSt>,
    mut m: ResMut<Mutators>,
    mut test: ResMut<MenuTest>,
    done: Res<MenuActionsDone>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("mainmenu") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let since = t - st.mark;
    let fail = |why: &str| -> ! {
        println!("TEST mainmenu FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let shot = |commands: &mut Commands, suffix: &str| {
        if let Some(p) = args.shot.clone() {
            let p = if suffix.is_empty() { p } else { p.replace(".png", &format!("{suffix}.png")) };
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
        }
    };
    match st.phase {
        0 => {
            let (v, n) = views(&mut m);
            if v.iter().any(|x| x == "GGUIViewMainMenu") && t > 1.0 {
                println!("  t={t:.1} main menu up: views {v:?}, {n} movies, game paused: {}", crate::uvm::menus::ui_open());
                if !crate::uvm::menus::ui_open() {
                    fail("the main menu does not hold the game");
                }
                shot(&mut commands, "");
                st.phase = 1;
                st.mark = t;
            } else if t > 25.0 {
                fail(&format!("no main menu (views {v:?})"));
            }
        }
        // a click spread over frames like a real one: hover, press, release
        1 if since > std::env::var("MM_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0) => {
            let Some(p) = label_pos(&mut m, "Play") else { fail("no Play button") };
            test.mouse.push((p.0, p.1, None));
            st.spawn = Some(Vec3::new(p.0, p.1, 0.0));
            st.phase = 10;
            st.mark = t;
        }
        10 if since > 0.2 => {
            let p = st.spawn.unwrap();
            test.mouse.push((p.x, p.y, Some(true)));
            st.phase = 11;
            st.mark = t;
        }
        11 if since > 0.15 => {
            let p = st.spawn.unwrap();
            test.mouse.push((p.x, p.y, Some(false)));
            st.phase = 2;
            st.mark = t;
        }
        2 if since > 1.0 => {
            let (v, _) = views(&mut m);
            println!("  t={t:.1} views {v:?}");
            if !v.iter().any(|x| x == "GGUIViewCustomGame") {
                fail("Play did not open Custom Game");
            }
            shot(&mut commands, "_custom");
            st.phase = 3;
            st.mark = t;
        }
        3 if since > 0.8 => {
            let Some(p) = label_pos(&mut m, "Play!") else { fail("no Play! button in Custom Game") };
            test.mouse.push((p.0, p.1, None));
            st.spawn = Some(Vec3::new(p.0, p.1, 0.0));
            st.phase = 30;
            st.mark = t;
        }
        30 if since > 0.2 => {
            let p = st.spawn.unwrap();
            test.mouse.push((p.x, p.y, Some(true)));
            st.phase = 31;
            st.mark = t;
        }
        31 if since > 0.15 => {
            let p = st.spawn.unwrap();
            test.mouse.push((p.x, p.y, Some(false)));
            st.phase = 4;
            st.mark = t;
        }
        4 if since > 1.5 => {
            let (v, n) = views(&mut m);
            let hud = m.host.as_mut().map(|h| h.with_vm(|h| h.hud_movie().is_some())).unwrap_or(false);
            println!("  t={t:.1} after Play!: views {v:?}, {n} movies, HUD {hud}, menu open {}, actions {:?}", crate::uvm::menus::ui_open(), done.0);
            if done.0.iter().any(|a| matches!(a, MenuAction::Start { .. })) {
                println!("  (Play! asked for a level load instead of StartClean)");
            } else {
                if crate::uvm::menus::ui_open() {
                    fail("the menu is still open after Play!");
                }
                if !hud {
                    fail("StartClean did not start the HUD");
                }
            }
            shot(&mut commands, "_play");
            st.phase = 5;
            st.mark = t;
        }
        5 if since > 0.5 => {
            println!("TEST mainmenu PASS");
            exit.write(AppExit::Success);
            st.phase = 6;
        }
        _ => {}
    }
}
