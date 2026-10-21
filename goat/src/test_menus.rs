//! `goat --test pausemenu --shot out/pausemenu.png`: the game's own pause menu (GGHUDGfxMenuPause,
//! Scaleform via Ruffle) opens through GGPlayerControllerGame.ShowPauseMenu, is screenshotted,
//! and a click on its Resume button closes it through the scripts (OnMenuItemClick ->
//! ResumePressed -> Close). Runs on real time: the game is paused under the menu.

use crate::Args;
use crate::uvm::menus::MenuTest;
use crate::uvm::plugin::Mutators;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

#[derive(Default)]
pub struct MenuSt {
    t: f32,
    phase: u32,
    mark: f32,
    click: Option<(f32, f32)>,
}

pub fn pausemenu_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    vtime: Res<Time<Virtual>>,
    mut commands: Commands,
    mut st: Local<MenuSt>,
    m: Res<Mutators>,
    mut test: ResMut<MenuTest>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("pausemenu") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST pausemenu FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    match st.phase {
        0 => {
            if m.host.is_some() && t > 2.0 {
                println!("  t={t:.1} {} -> ShowPauseMenu", m.status);
                test.open_pause = true;
                test.want_buttons = true;
                st.phase = 1;
                st.mark = t;
            } else if t > 40.0 {
                fail("the UnrealScript host never started");
            }
        }
        1 => {
            if test.open && !test.buttons.is_empty() && t - st.mark > 1.0 {
                for (label, r) in &test.buttons {
                    println!("  button {label:?} at {:.0},{:.0} {:.0}x{:.0}", r[0], r[1], r[2], r[3]);
                }
                if !vtime.is_paused() {
                    fail("the game is not paused under the menu");
                }
                let Some((label, r)) = test.buttons.iter().find(|(l, _)| l.eq_ignore_ascii_case("resume")).cloned() else {
                    fail("no Resume button in the pause menu")
                };
                st.click = Some((r[0] + r[2] * 0.5, r[1] + r[3] * 0.5));
                println!("  t={t:.1} pause menu open with {} buttons; {label} at {:?}", test.buttons.len(), st.click);
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
                st.phase = 2;
                st.mark = t;
            } else if t - st.mark > 5.0 {
                fail(&format!("pause menu did not open (open {}, {} buttons)", test.open, test.buttons.len()));
            }
        }
        2 if t - st.mark > 0.6 => {
            let (x, y) = st.click.unwrap();
            test.mouse.push((x, y, None));
            st.phase = 3;
        }
        3 => {
            let (x, y) = st.click.unwrap();
            test.mouse.push((x, y, Some(true)));
            st.phase = 4;
        }
        4 => {
            let (x, y) = st.click.unwrap();
            test.mouse.push((x, y, Some(false)));
            st.phase = 5;
            st.mark = t;
        }
        5 => {
            if !test.open {
                if vtime.is_paused() {
                    fail("menu closed but the game stayed paused");
                }
                println!("  t={t:.1} Resume closed the pause menu (opened {} time(s))", test.opened_count);
                // again, closed with Escape this time (GGUIViewPauseMenu.SubFilterButtonInput)
                test.open_pause = true;
                st.phase = 6;
                st.mark = t;
            } else if t - st.mark > 4.0 {
                fail("clicking Resume did not close the menu");
            }
        }
        6 => {
            if test.open && t - st.mark > 0.8 {
                test.keys.push(("Escape", true));
                test.keys.push(("Escape", false));
                st.phase = 7;
                st.mark = t;
            } else if t - st.mark > 5.0 {
                fail("the pause menu did not open a second time");
            }
        }
        7 => {
            if !test.open {
                println!("  t={t:.1} Escape closed the pause menu (opened {} time(s))", test.opened_count);
                println!("TEST pausemenu PASS");
                exit.write(AppExit::Success);
                st.phase = 8;
            } else if t - st.mark > 4.0 {
                fail("Escape did not close the pause menu");
            }
        }
        _ => {}
    }
}

#[derive(Default)]
pub struct HudSt {
    t: f32,
    phase: u32,
    mark: f32,
    sent: u32,
    /// Score widget value and when it last moved.
    last: (f32, f32),
}

/// `goat --test hud --shot out/hud.png`: the game's in-game HUD movie (GGHUDGfxInGame, started
/// like GGPlayerControllerGame.StartHUD) is drawn over the game; tricks from the Rust combo system
/// reach its legacy combo widget through AddCombo (screenshot), and when the combo is banked
/// EndCombo + UpdateScore make the score widget count up to the new total.
pub fn hud_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<HudSt>,
    mut m: ResMut<Mutators>,
    mut score: ResMut<crate::score::Score>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("hud") {
        return;
    }
    let dt = time.delta_secs().min(0.1);
    st.t += dt;
    let t = st.t;
    let fail = |why: &str| -> ! {
        println!("TEST hud FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    // (combo widget multiplier, score widget score, its pending score)
    let mut read = |m: &mut Mutators| -> Option<(f32, f32, f32)> {
        let h = m.host.as_mut()?;
        h.with_vm(|h| {
            let mv = h.hud_movie()?;
            let combo = h.vm.get(mv, "mComboLegacyWidget").obj()?;
            let sw = h.vm.get(mv, "mScoreWidget").obj()?;
            Some((h.vm.get(combo, "mPreviousMultiplier").float(), h.vm.get(sw, "mScore").float(), h.vm.get(sw, "mPendingScore").float()))
        })
    };
    let trick = |name: &str, s: i32, mult: bool| crate::score::Trick {
        id: crate::score::TrickId::Kismet,
        name: name.into(),
        clean_name: name.into(),
        score: s,
        adds_multiplier: mult,
        has_name: true,
        min_score: 1,
    };
    match st.phase {
        0 => {
            if crate::uvm::menus::hud_shown() && t > 2.0 {
                let r = read(&mut m);
                println!("  t={t:.1} HUD movie drawn; widgets {r:?}");
                if r.is_none() {
                    fail("the HUD movie has no combo/score widget");
                }
                st.phase = 1;
                st.mark = t;
            } else if t > 40.0 {
                fail(&format!("the HUD movie never showed ({})", m.status));
            }
        }
        1 => {
            // three tricks, one per frame, as the trick system reports them
            let names = [("Backflip", 50, true), ("Headbutt Fence", 120, true), ("Lick Barrel", 75, true)];
            let (n, s, mu) = names[st.sent as usize];
            score.report(trick(n, s, mu));
            st.sent += 1;
            if st.sent as usize == names.len() {
                println!("  t={t:.1} reported {} tricks: combo {} x{:.1}", st.sent, score.combo_total(), score.multiplier);
                st.phase = 2;
                st.mark = t;
            }
        }
        2 => {
            let r = read(&mut m);
            if t - st.mark > 1.0 {
                let Some((mult, _, _)) = r else { fail("HUD widgets gone") };
                if (mult - score.multiplier).abs() > 0.01 {
                    fail(&format!("combo widget multiplier {mult} != combo multiplier {}", score.multiplier));
                }
                println!("  t={t:.1} combo widget shows x{mult:.1}");
                if let Some(p) = args.shot.clone() {
                    commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
                }
                st.phase = 3;
                st.mark = t;
            }
        }
        3 => {
            // the combo banks after 5 s; the score widget waits 1.5 s and then counts up (per
            // frame, as GGHUD.DrawHUD ticks it); fail when it stops moving short of the total
            let r = read(&mut m);
            if let Some((_, s, _)) = r {
                if s != st.last.0 {
                    st.last = (s, t);
                }
            }
            if let Some((_, s, pending)) = r {
                if score.total > 0 && pending <= 0.0 && (s - score.total as f32).abs() < 1.0 {
                    println!("  t={t:.1} score widget reached {s} (total {})", score.total);
                    println!("TEST hud PASS");
                    exit.write(AppExit::Success);
                    st.phase = 4;
                    return;
                }
            }
            let banked_by = st.mark + 5.0 + 1.5 + 2.0;
            if t > banked_by.max(st.last.1 + 4.0) || t - st.mark > 40.0 {
                fail(&format!("the score widget did not count up to {} ({r:?})", score.total));
            }
        }
        _ => {}
    }
}
