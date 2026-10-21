//! `--test mods`: open the mod menu, load the Steam Workshop list, download the smallest item and
//! check its unpacked files, then enable the Moon Goat example mod and check the tweaks apply.
//! Screenshots: <shot> (Workshop tab) and <shot>_rust.png (Rust mods tab).

use crate::Args;
use crate::mods::{ModAction, ModMenu, Status, Tab};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

#[derive(Default)]
pub struct St {
    t: f32,
    phase: u32,
    mark: f32,
    last_status: String,
    status_since: f32,
    target: Option<String>,
    enabled_before: Option<bool>,
}

fn fail(why: &str, t: f32) -> ! {
    println!("TEST mods FAIL at {t:.1}s: {why}");
    std::process::exit(10);
}

fn shot(commands: &mut Commands, args: &Args, suffix: &str) {
    if let Some(p) = args.shot.clone() {
        let p = if suffix.is_empty() { p } else { p.replace(".png", &format!("{suffix}.png")) };
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(p));
    }
}

pub fn mods_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<St>,
    menu: Res<ModMenu>,
    mut actions: MessageWriter<ModAction>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("mods") {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let status = match menu.tab {
        Tab::Workshop => format!("{:?}", menu.workshop_status),
        Tab::Rust => format!("{:?}", menu.rust_status),
        Tab::Game => String::new(),
    };
    if status != st.last_status {
        st.last_status = status.clone();
        st.status_since = t;
        println!("  t={t:.1} {status}");
    }
    match st.phase {
        0 if t > 0.5 => {
            actions.write(ModAction::Open);
            st.phase = 1;
            st.mark = t;
        }
        1 => {
            if let Status::Error(e) = &menu.workshop_status {
                fail(&format!("Workshop list: {e}"), t);
            }
            if !menu.workshop.is_empty() && menu.workshop_status == Status::Idle {
                println!("  t={t:.1} Workshop list: {} items", menu.workshop.len());
                st.phase = 2;
                st.mark = t;
            } else if t - st.status_since > 30.0 {
                fail(&format!("stuck at {status}"), t);
            }
        }
        2 => {
            // the first page's previews
            let shown: Vec<&String> = menu.workshop.iter().take(5).map(|i| &i.id).collect();
            let loaded = shown.iter().filter(|id| menu.previews.contains_key(&format!("ws-{id}"))).count();
            if loaded >= shown.len().min(3) || t - st.mark > 15.0 {
                println!("  t={t:.1} previews on page 1: {loaded}/{}", shown.len());
                shot(&mut commands, &args, "");
                // the smallest downloadable item
                let pick = menu.workshop.iter().filter(|i| !i.file_url.is_empty() && i.file_size > 0).min_by_key(|i| i.file_size).cloned();
                let Some(item) = pick else { fail("no downloadable Workshop item", t) };
                println!("  downloading {} \"{}\" ({} bytes)", item.id, item.title, item.file_size);
                actions.write(ModAction::Download(item.id.clone()));
                st.target = Some(item.id);
                st.phase = 3;
                st.mark = t;
            }
        }
        3 => {
            let id = st.target.clone().unwrap();
            if let Some(e) = menu.failed.get(&id) {
                fail(&format!("download failed: {e}"), t);
            }
            if menu.downloaded.contains(&id) {
                let dir = crate::mods::mods_dir().join("workshop").join(&id);
                let mut files = Vec::new();
                let mut stack = vec![dir.clone()];
                while let Some(d) = stack.pop() {
                    for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                        if e.path().is_dir() {
                            stack.push(e.path());
                        } else {
                            files.push(e.path());
                        }
                    }
                }
                let packages = files.iter().filter(|p| std::fs::read(p).is_ok_and(|b| b.starts_with(&[0xC1, 0x83, 0x2A, 0x9E]))).count();
                println!("  t={t:.1} unpacked {} files ({packages} Unreal packages) to {}", files.len(), dir.display());
                if files.len() < 2 {
                    fail("nothing unpacked", t);
                }
                actions.write(ModAction::Tab(Tab::Rust));
                st.phase = 4;
                st.mark = t;
            } else if t - st.mark > 60.0 {
                fail("download took over 60 s", t);
            }
        }
        4 if t - st.mark > 0.5 => {
            let Some(m) = menu.rust.iter().find(|m| m.id == "moon-goat") else { fail("Moon Goat example mod not listed", t) };
            if !m.installed {
                fail("Moon Goat not installed", t);
            }
            let was = menu.enabled.contains("moon-goat");
            st.enabled_before = Some(was);
            if !was {
                actions.write(ModAction::Toggle("moon-goat".into()));
            }
            st.phase = 5;
            st.mark = t;
        }
        5 if t - st.mark > 1.5 => {
            let tw = crate::mods::tweak();
            println!("  t={t:.1} tweaks with Moon Goat: {tw:?}");
            if (tw.gravity - 0.35).abs() > 1e-3 || (tw.jump_z - 1.3).abs() > 1e-3 {
                fail("Moon Goat tweaks not applied", t);
            }
            shot(&mut commands, &args, "_rust");
            st.phase = 6;
            st.mark = t;
        }
        6 if t - st.mark > 0.5 => {
            // leave mods/enabled.json the way it was
            if st.enabled_before == Some(false) {
                actions.write(ModAction::Toggle("moon-goat".into()));
            }
            println!("TEST mods PASS");
            st.phase = 7;
            st.mark = t;
        }
        7 if t - st.mark > 0.5 => {
            exit.write(AppExit::Success);
            st.phase = 8;
        }
        _ => {}
    }
}
