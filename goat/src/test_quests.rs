//! `--test quests`: the original quest manager runs in the VM. Baa three times -> GGQuestBaa
//! completes; walk onto the nearest goat trophy -> it is picked up (PickedUp in script), the
//! collectible quest moves on and the progression is saved. Run with GOAT_SAVE_DIR pointing at
//! an empty folder to start from a fresh save. Screenshot: <shot>.

use crate::goat::{Goat, ScriptedInput};
use crate::physics::Physics;
use crate::uvm::collectibles::Collectibles;
use crate::uvm::plugin::Mutators;
use crate::Args;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

#[derive(Default)]
pub struct St {
    t: f32,
    phase: u32,
    mark: f32,
    baas: u32,
    picked_before: u32,
}

fn fail(why: &str, t: f32) -> ! {
    println!("TEST quests FAIL at {t:.1}s: {why}");
    std::process::exit(10);
}

#[allow(clippy::too_many_arguments)]
pub fn quests_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut commands: Commands,
    mut st: Local<St>,
    mut m: ResMut<Mutators>,
    mut input: ResMut<ScriptedInput>,
    col: Res<Collectibles>,
    mut phys: ResMut<Physics>,
    mut goats: Query<(&mut Goat, &mut Transform)>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("quests") {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    *input = ScriptedInput::default();
    input.cam_dist = Some(5.0);
    let Ok((mut g, mut tr)) = goats.single_mut() else { return };
    let quest = |m: &mut Mutators, class: &str| m.host.as_mut().and_then(|h| h.quests().into_iter().find(|q| q.class == class));
    match st.phase {
        0 => {
            let n = m.host.as_mut().map(|h| h.quests().len()).unwrap_or(0);
            if n > 0 {
                println!("  t={t:.1} quest manager up: {n} quests");
                for q in m.host.as_mut().unwrap().quests() {
                    println!("    {:28} active={} done={} {}/{}", q.class, q.active, q.completed, q.progress, q.goal);
                }
                st.phase = 1;
                st.mark = t;
            } else if t > 15.0 {
                fail("no quest manager (VM host not started?)", t);
            }
        }
        // baa x3, 0.4 s apart (GGQuestBaa.OnBaa counts them)
        1 if t - st.mark > 0.4 => {
            input.baa = true;
            st.baas += 1;
            st.mark = t;
            if st.baas == 3 {
                st.phase = 2;
            }
        }
        2 if t - st.mark > 0.6 => {
            let Some(q) = quest(&mut m, "GGQuestBaa") else { fail("no GGQuestBaa on this map", t) };
            println!("  t={t:.1} Baa quest: done={} {}/{}", q.completed, q.progress, q.goal);
            if !q.completed {
                fail("three baas did not complete the Baa quest", t);
            }
            // walk onto the nearest trophy
            let here = tr.translation;
            let pos = col.positions();
            let Some(p) = pos.iter().copied().min_by(|a, b| a.distance_squared(here).total_cmp(&b.distance_squared(here))) else {
                fail("no collectibles left to find (use a fresh GOAT_SAVE_DIR)", t)
            };
            println!("  {} collectibles out there; going to {p:.1?}", pos.len());
            st.picked_before = col.picked;
            crate::test_tricks::place(&mut phys, &mut g, &mut tr, p + Vec3::Y * 0.7, Vec3::X);
            st.phase = 3;
            st.mark = t;
        }
        3 => {
            if col.picked > st.picked_before {
                println!("  t={t:.1} trophy picked up");
                st.phase = 4;
                st.mark = t;
            } else if t - st.mark > 3.0 {
                fail(&format!("trophy not picked up (goat at {:.1?})", tr.translation), t);
            }
        }
        4 if t - st.mark > 1.0 => {
            if let Some(q) = quest(&mut m, "GGQuestCollectible1") {
                println!("  collectible quest: {}/{}", q.progress, q.goal);
                if q.progress < 1.0 {
                    fail("the collectible quest did not count the trophy", t);
                }
            }
            let save = crate::uvm::progress::save_dir().join("progression.json");
            let text = std::fs::read_to_string(&save).unwrap_or_default();
            println!("  {} ({} bytes): collectibles found saved = {}", save.display(), text.len(), text.contains("level_01_"));
            if !text.contains("level_01_") {
                fail("progression.json lacks the found trophy", t);
            }
            println!("  achievements this run: {:?}", m.achievements);
            if let Some(p) = args.shot.clone() {
                commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
            }
            st.phase = 5;
            st.mark = t;
        }
        5 if t - st.mark > 0.5 => {
            println!("TEST quests PASS");
            exit.write(AppExit::Success);
            st.phase = 6;
        }
        _ => {}
    }
}

/// `--test music`: Deadgoa7 (GGMutatorGoatMau5) -> R starts the party:
/// GGMutatorComponentMusicParty.PlayCustomTrack on the VM's GGMusicManager must become the
/// track the game plays (silent in tests; the cue is checked).
#[allow(clippy::too_many_arguments)]
pub fn music_scenario(
    args: Res<Args>,
    time: Res<Time>,
    mut st: Local<St>,
    mut m: ResMut<Mutators>,
    amb: Res<crate::ambient::Ambience>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("music") {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    match st.phase {
        0 => {
            if m.host.as_ref().is_some_and(|h| !h.mutators.is_empty()) && t > 2.0 {
                println!("  t={t:.1} {} ambient sounds, {} in range; music before: {:?}", amb.total, amb.audible, amb.music());
                m.test_keys = vec!["R".into()];
                st.phase = 1;
                st.mark = t;
            } else if t > 20.0 {
                println!("TEST music FAIL at {t:.1}s: mutator did not start");
                std::process::exit(10);
            }
        }
        1 if t - st.mark > 0.3 => {
            m.test_keys.clear();
            st.phase = 2;
        }
        2 => {
            if let Some(c) = amb.music() {
                println!("  t={t:.1} music: {c}");
                println!("TEST music PASS");
                exit.write(AppExit::Success);
                st.phase = 3;
            } else if t - st.mark > 4.0 {
                println!("TEST music FAIL at {t:.1}s: no music track after R");
                std::process::exit(10);
            }
        }
        _ => {}
    }
}
