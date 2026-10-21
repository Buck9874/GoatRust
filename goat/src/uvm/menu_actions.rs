//! Driving the open Scaleform menu by what it shows: find a widget by its label anywhere in the
//! top movie's display list and click it (tests, `goat --uvm-menu-click`).

use super::bridge::Host;
use super::gfx::{self, as_number, with_movie};
use super::*;
use bevy::prelude::*;
use gfxui::Value as G;

/// Screen rectangle (viewport pixels) of the first visible widget whose `label` (or text field
/// text) equals `label`, ignoring case.
pub fn find_labeled(vm: &mut Vm, label: &str) -> Option<[f32; 4]> {
    let m = gfx::top(vm)?;
    let (vw, vh) = vm.gfx.size;
    let want = label.to_lowercase();
    let r = with_movie(vm, m, |mv| {
        let root = mv.root()?;
        let mut queue = vec![(root.clone(), 0)];
        let mut seen = 0;
        while let Some((o, depth)) = queue.pop() {
            seen += 1;
            if seen > 5000 {
                break;
            }
            if !matches!(mv.get(&o, "visible"), Ok(G::Bool(true))) {
                continue;
            }
            let text = match mv.get(&o, "label") {
                Ok(G::String(s)) if !s.is_empty() => Some(s),
                _ => match mv.get(&o, "text") {
                    Ok(G::String(s)) if depth > 0 => Some(s),
                    _ => None,
                },
            };
            if text.is_some_and(|t| t.trim().to_lowercase() == want) {
                let G::Object(b) = mv.invoke(&o, "getBounds", &[G::Object(root.clone())])? else { continue };
                let f = |n: &str| mv.get(&b, n).map(|v| as_number(&v));
                let r = [f("x")?, f("y")?, f("width")?, f("height")?];
                if r[2] > 1.0 && r[3] > 1.0 {
                    return Ok(Some(r));
                }
            }
            if depth < 12 {
                let n = mv.get(&o, "numChildren").map(|v| as_number(&v) as usize).unwrap_or(0);
                for i in (0..n.min(400)).rev() {
                    if let Ok(G::Object(c)) = mv.invoke(&o, "getChildAt", &[G::Number(i as f64)]) {
                        queue.push((c, depth + 1));
                    }
                }
            }
        }
        Ok(None)
    })
    .ok()??;
    let s = (vw as f64 / 1920.0).min(vh as f64 / 1080.0);
    let (ox, oy) = ((vw as f64 - 1920.0 * s) / 2.0, (vh as f64 - 1080.0 * s) / 2.0);
    Some([(ox + r[0] * s) as f32, (oy + r[1] * s) as f32, (r[2] * s) as f32, (r[3] * s) as f32])
}

/// A menu frame as the game runs it: advance, then render (CLIK finishes widget setup on the
/// RENDER event, which Ruffle sends when a frame is drawn).
pub fn frame(h: &mut Host, dt: f32) {
    h.tick_menus(dt);
    let _ = gfx::capture(&mut h.vm);
}

impl Host {
    /// Click the widget labelled `label` in the top menu (move, press, release with menu frames
    /// in between). Returns where it clicked.
    pub fn click_label(&mut self, label: &str) -> Option<(f32, f32)> {
        let r = find_labeled(&mut self.vm, label)?;
        let (x, y) = (r[0] + r[2] * 0.5, r[1] + r[3] * 0.5);
        gfx::mouse_move(&mut self.vm, x, y);
        frame(self, 1.0 / 30.0);
        gfx::mouse_button(&mut self.vm, x, y, true);
        frame(self, 1.0 / 30.0);
        gfx::mouse_button(&mut self.vm, x, y, false);
        frame(self, 1.0 / 30.0);
        Some((x, y))
    }

    /// Class names of the pause menu's view stack (GGHUDGfxMenu.mStack), bottom first.
    pub fn menu_views(&mut self) -> Vec<String> {
        let Some(top) = gfx::top(&self.vm) else { return vec![] };
        let Value::Array(stack) = self.vm.get(top, "mStack") else { return vec![] };
        stack.iter().filter_map(|v| v.obj()).map(|o| self.vm.class_name(self.vm.obj(o).class).to_string()).collect()
    }
}

/// `goat --uvm-menu-click <step>[,<step>...] [out.png]`: open the pause menu headless, then per
/// step click the widget with that label (or press a key: `key:Escape`), run a second of menu
/// frames and print the open movies and view stack. Saves the last frame.
pub fn run_cli(args: &[String]) -> anyhow::Result<()> {
    let mut steps: Vec<String> = args.first().map(|s| s.split(',').map(|x| x.trim().to_string()).collect()).unwrap_or_default();
    // a first step "main" starts from the main menu instead of the pause menu
    let main = steps.first().is_some_and(|s| s.eq_ignore_ascii_case("main"));
    if main {
        steps.remove(0);
    }
    let out = args.get(1).cloned().unwrap_or_else(|| "out/menu_click.png".into());
    // UVM_MAP=Maps/Level_01.goat loads the map like the game; UVM_START=1 also runs the start
    // (Kismet, quests) before the menu opens
    let map = std::env::var("UVM_MAP").ok().map(|m| upk::cooked_dir().join(m));
    let mut h = Host::with_map(&[], map.as_deref())?;
    if std::env::var_os("UVM_START").is_some() {
        h.start(&[]);
    }
    h.vm.gfx.log_calls = std::env::var_os("UVM_GFX_LOG").is_some();
    h.with_vm(|h| {
        gfx::set_size(&mut h.vm, 1280, 720);
        if main {
            h.open_main_menu();
        } else {
            h.open_pause_menu();
        }
        for _ in 0..30 {
            frame(h, 1.0 / 30.0);
        }
    });
    let report = |h: &mut Host, what: &str| {
        let movies: Vec<String> = h.vm.gfx.order.clone().iter().map(|&m| h.vm.class_name(h.vm.obj(m).class).to_string()).collect();
        let views = h.menu_views();
        println!("--- {what}: movies {movies:?} views {views:?}");
    };
    report(&mut h, "opened");
    for step in &steps {
        let mark = h.vm.log.len();
        let t = std::time::Instant::now();
        let r = h.with_vm(|h| {
            // wait:<seconds> just runs the menus
            if let Some(secs) = step.strip_prefix("wait:").and_then(|x| x.parse::<f32>().ok()) {
                for _ in 0..(secs * 30.0) as usize {
                    frame(h, 1.0 / 30.0);
                }
                return Some((0.0, 0.0));
            }
            let r = match step.strip_prefix("key:") {
                Some(k) => {
                    gfx::key(&mut h.vm, k, true);
                    gfx::key(&mut h.vm, k, false);
                    Some((0.0, 0.0))
                }
                None => h.click_label(step),
            };
            for _ in 0..30 {
                frame(h, 1.0 / 30.0);
            }
            r
        });
        report(&mut h, &format!("{step} -> {r:?} ({:.0} ms)", t.elapsed().as_secs_f32() * 1000.0));
        for l in h.vm.log[mark..].iter().filter(|l| l.contains("WARN") || l.contains("GFx ") || l.contains("Accessed None") || l.contains("not found")).take(25) {
            println!("    {l}");
        }
    }
    let imgs = h.with_vm(|h| gfx::capture(&mut h.vm));
    if let Some((_, img)) = imgs.last() {
        let mut bg = image::RgbaImage::from_pixel(img.width(), img.height(), image::Rgba([90, 120, 150, 255]));
        image::imageops::overlay(&mut bg, img, 0, 0);
        bg.save(&out)?;
        println!("--- saved {out}");
    }
    let _ = std::fs::write("out/uvm_menu_click_log.txt", h.vm.log.join("\n"));
    let events: Vec<String> = h.vm.events.iter().map(|e| format!("{e:?}")).filter(|e| !e.starts_with("PlaySound")).collect();
    println!("--- menu actions {:?}, host events {events:?}", ACTIONS.lock().unwrap());
    println!("--- applied settings {:?}", super::settings::applied());
    println!("--- unsupported (count):");
    for (k, n) in h.vm.native_report().iter().take(40) {
        println!("  {n:5} {k}");
    }
    Ok(())
}

// --- what menu buttons do to the Rust game ------------------------------------------------------

/// Things the menu scripts ask the game to do.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    /// GGUIViewPauseMenu.RespawnPressed (the original calls GGGameInfo.OnPlayerRespawn).
    Respawn,
    /// PlayerController.ClientTravel (Restart: the same map again).
    Travel(String),
    /// ConsoleCommand("exit" / "quit") (Exit Game).
    Quit,
    /// ConsoleCommand("start <map>?options") from Custom Game's Play!, with the chosen mutators
    /// (class paths).
    Start { url: String, mutators: Vec<String> },
    /// OnlineUIInterface.OpenWebURL (DLC store page, leaderboards, Workshop browse/guide).
    OpenUrl(String),
}

static ACTIONS: std::sync::Mutex<Vec<MenuAction>> = std::sync::Mutex::new(Vec::new());

pub fn push_action(a: MenuAction) {
    ACTIONS.lock().unwrap().push(a);
}

/// Natives/overrides that turn menu actions into `MenuAction`s.
pub fn register(vm: &mut Vm) {
    // Steam overlay pages the menus open (OnlineUIInterface.OpenWebURL / ShowAchievementsUI)
    vm.overrides.insert("gguiviewmainmenu.dlcpressed".into(), |vm, _| {
        vm.say("OpenWebURL DLC".into());
        push_action(MenuAction::OpenUrl("http://store.steampowered.com/dlc/265930/".into()));
        Value::Obj(None)
    });
    vm.overrides.insert("gguiviewmainmenu.leaderboardspressed".into(), |vm, c| {
        let url = c.this.map(|t| vm.get(t, "mLeaderboardURL").str().to_string()).filter(|u| !u.is_empty()).unwrap_or_else(|| "https://steamcommunity.com/stats/265930/leaderboards/".into());
        push_action(MenuAction::OpenUrl(url));
        Value::Obj(None)
    });
    vm.overrides.insert("gguiviewmainmenu.achievementspressed".into(), |_, _| {
        push_action(MenuAction::OpenUrl("https://steamcommunity.com/stats/265930/achievements/".into()));
        Value::Obj(None)
    });
    vm.overrides.insert("gguiviewworkshop.onbrowseworkshoppressed".into(), |_, _| {
        push_action(MenuAction::OpenUrl("http://steamcommunity.com/workshop/browse/?appid=265930".into()));
        Value::Obj(None)
    });
    vm.overrides.insert("gguiviewworkshop.opensteamworkshopguide".into(), |_, _| {
        push_action(MenuAction::OpenUrl("http://steamcommunity.com/sharedfiles/filedetails/?id=232365177".into()));
        Value::Obj(None)
    });
    // the original respawns through GGGameInfo.OnPlayerRespawn -> RestartPlayer; here the Rust goat
    // respawns (its respawn then reaches the scripts as the usual OnPlayerRespawn game event)
    vm.overrides.insert("gguiviewpausemenu.respawnpressed".into(), |vm, c| {
        push_action(MenuAction::Respawn);
        if let Some(m) = c.this.and_then(|o| vm.obj(o).outer) {
            vm.call_named(m, "Close", vec![Value::Bool(true)]);
        }
        Value::Int(0)
    });
    vm.natives.insert("playercontroller.clienttravel".into(), |vm, c| {
        let url = c.str(0);
        vm.say(format!("ClientTravel({url})"));
        push_action(MenuAction::Travel(url));
        Value::Int(0)
    });
}

/// Run the queued menu actions: respawn the goat, restart the level (a new process on the same
/// map, like the mod menu's map switch), quit. Test runs only log restart/quit.
pub fn apply_menu_actions(
    args: Res<crate::Args>,
    mut phys: ResMut<crate::physics::Physics>,
    mut goats: Query<(&mut crate::goat::Goat, &mut Transform)>,
    mut exit: MessageWriter<AppExit>,
    mut done: ResMut<MenuActionsDone>,
) {
    let actions: Vec<MenuAction> = std::mem::take(&mut *ACTIONS.lock().unwrap());
    for a in actions {
        info!("[menu] {a:?}");
        done.0.push(a.clone());
        match a {
            MenuAction::Respawn => {
                if let Ok((mut g, mut t)) = goats.single_mut() {
                    crate::goat::respawn(&mut g, &mut phys, &mut t);
                }
            }
            MenuAction::Travel(_) if args.test.is_none() => {
                let map = args.map.clone();
                crate::mods::play_map(&map);
            }
            MenuAction::Start { url, mutators } if args.test.is_none() => {
                let name = url.split('?').next().unwrap_or("").trim().to_string();
                let path = map_path(&name).unwrap_or_else(|| args.map.clone());
                let keys: Vec<String> = mutators.iter().map(|m| format!("gm-{m}")).collect();
                // ?game=...?raceID=... go along to the new process
                let options = url.find('?').map(|i| url[i..].to_string()).unwrap_or_default();
                crate::mods::play_map_with_options(&path, &keys, &options);
            }
            MenuAction::OpenUrl(url) if args.test.is_none() => {
                // the Steam overlay's web page, in the default browser instead
                if url.starts_with("http://") || url.starts_with("https://") {
                    info!("opening {url}");
                    let _ = std::process::Command::new("cmd").args(["/C", "start", "", url.as_str()]).spawn();
                }
            }
            MenuAction::Quit if args.test.is_none() => {
                exit.write(AppExit::Success);
            }
            _ => {}
        }
    }
}

/// Menu actions run so far (tests).
#[derive(Resource, Default)]
pub struct MenuActionsDone(pub Vec<MenuAction>);

/// A map name from a travel URL ("Level_02") -> its file relative to CookedPC.
pub fn map_path(name: &str) -> Option<String> {
    let dir = upk::cooked_dir();
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let ext = p.extension().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if stem.eq_ignore_ascii_case(name) && matches!(ext.as_str(), "goat" | "udk" | "umap") {
                return p.strip_prefix(&dir).ok().map(|r| r.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    None
}

impl super::bridge::Host {
    /// GGGameViewportClient.mMutatorMap MutatorName entries (the Custom Game selection).
    pub fn chosen_mutators(&mut self) -> Vec<String> {
        let Some(&vp) = self.vm.globals.get("viewport") else { return vec![] };
        let list = match self.vm.get(vp, "mMutatorMap") {
            Value::Array(a) => a,
            _ => return vec![],
        };
        let k = self.vm.sym("MutatorName");
        list.iter()
            .filter_map(|v| match v {
                Value::Struct(s) => s.field(k).map(|f| f.str().to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty())
            .collect()
    }
}
