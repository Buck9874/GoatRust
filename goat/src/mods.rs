//! Mod menu (M): Steam Workshop mods for Goat Simulator and mods made for this Rust version.
//!
//! * Workshop: the item list comes from the Workshop browse pages, details (title, description,
//!   preview, file) from Steam's keyless `GetPublishedFileDetails`. Downloads are the Workshop
//!   bundles (`[u32 0][u32 count]` then `[u32 name len][name]` and chunks `[u32 size][data][u32 more]`
//!   per file, more = 1 while another 2 MB chunk of that file follows),
//!   unpacked to `mods/workshop/<id>/`. They are UnrealScript mutators and maps for the original
//!   game, so this version can list and download them but not run them yet.
//! * Rust mods: `mods/rust/<id>/mod.json` (+ optional preview image) with gameplay tweaks that
//!   apply while the mod is enabled. More can be listed from an index (URL in
//!   `mods/index_url.txt`, JSON `{"mods": [{.., "download_url", "preview_url"}]}`).
//!
//! Network and disk work runs on one worker thread; previews and lists are cached in
//! `mods/cache/`.

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, RwLock};

pub const STEAM_APP_ID: u32 = 265930;
const PER_PAGE: usize = 5;

// --- gameplay tweaks ---------------------------------------------------------------------

/// Multipliers from the enabled Rust mods (1.0 = the original game).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tweaks {
    pub walk_speed: f32,
    pub sprint_speed: f32,
    pub jump_z: f32,
    pub gravity: f32,
    pub air_control: f32,
    pub headbutt: f32,
}

impl Default for Tweaks {
    fn default() -> Self {
        Self { walk_speed: 1.0, sprint_speed: 1.0, jump_z: 1.0, gravity: 1.0, air_control: 1.0, headbutt: 1.0 }
    }
}

static MENU_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the mod menu is showing (the camera and goat ignore the mouse/keys then).
pub fn menu_open() -> bool {
    MENU_OPEN.load(std::sync::atomic::Ordering::Relaxed)
}

static TWEAKS: RwLock<Tweaks> = RwLock::new(Tweaks { walk_speed: 1.0, sprint_speed: 1.0, jump_z: 1.0, gravity: 1.0, air_control: 1.0, headbutt: 1.0 });

/// The tweaks of all enabled mods, combined.
pub fn tweak() -> Tweaks {
    *TWEAKS.read().unwrap()
}

fn tweaks_of(v: &Value) -> Tweaks {
    let t = &v["tweaks"];
    let f = |k: &str| t[k].as_f64().map(|x| x as f32).filter(|x| x.is_finite() && *x > 0.0 && *x < 20.0).unwrap_or(1.0);
    Tweaks {
        walk_speed: f("walk_speed"),
        sprint_speed: f("sprint_speed"),
        jump_z: f("jump_z"),
        gravity: f("gravity"),
        air_control: f("air_control"),
        headbutt: f("headbutt"),
    }
}

// --- data ----------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct WorkshopItem {
    pub id: String,
    pub title: String,
    pub description: String,
    pub preview_url: String,
    pub file_url: String,
    pub file_size: u64,
    pub tags: Vec<String>,
    pub subscriptions: u64,
}

#[derive(Clone, Debug)]
pub struct RustMod {
    pub id: String,
    pub name: String,
    pub author: String,
    pub version: String,
    pub description: String,
    /// Local preview file or remote preview URL.
    pub preview: String,
    pub download_url: String,
    pub installed: bool,
    pub tweaks: Tweaks,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tab {
    #[default]
    Workshop,
    Rust,
    /// The base game's own mutators (DefaultDLC.ini), run in the UnrealScript VM.
    Game,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Busy(String),
    Error(String),
}

#[derive(Resource)]
pub struct ModMenu {
    pub open: bool,
    pub tab: Tab,
    pub page: usize,
    pub dirty: bool,
    pub workshop: Vec<WorkshopItem>,
    pub workshop_status: Status,
    pub workshop_requested: bool,
    pub rust: Vec<RustMod>,
    pub rust_status: Status,
    pub enabled: HashSet<String>,
    /// Workshop ids downloaded to mods/workshop.
    pub downloaded: HashSet<String>,
    /// id -> fraction while downloading; id -> error after a failure.
    pub downloading: HashMap<String, f32>,
    pub failed: HashMap<String, String>,
    pub previews: HashMap<String, Handle<Image>>,
    pub preview_requested: HashSet<String>,
}

impl Default for ModMenu {
    fn default() -> Self {
        Self {
            open: false,
            tab: Tab::Workshop,
            page: 0,
            dirty: true,
            workshop: vec![],
            workshop_status: Status::Idle,
            workshop_requested: false,
            rust: vec![],
            rust_status: Status::Idle,
            enabled: HashSet::new(),
            downloaded: HashSet::new(),
            downloading: HashMap::new(),
            failed: HashMap::new(),
            previews: HashMap::new(),
            preview_requested: HashSet::new(),
        }
    }
}

enum Job {
    ListWorkshop { refresh: bool },
    ListRust,
    Preview { key: String, url: String },
    DownloadWorkshop(WorkshopItem),
    DownloadRust(RustMod),
}

enum Done {
    WorkshopProgress(String),
    Workshop(Result<Vec<WorkshopItem>, String>),
    Rust(Vec<RustMod>, Option<String>),
    Preview { key: String, image: Option<(u32, u32, Vec<u8>)> },
    Progress { id: String, frac: f32 },
    Downloaded { id: String, rust: bool },
    Failed { id: String, error: String },
}

#[derive(Resource)]
pub struct Worker {
    tx: Sender<Job>,
    rx: Mutex<Receiver<Done>>,
}

pub fn mods_dir() -> PathBuf {
    std::env::var_os("GOAT_MODS_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("mods"))
}

// --- worker thread -------------------------------------------------------------------------

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .user_agent("GoatRust-mod-menu/0.1")
        .build()
        .into()
}

fn spawn_worker() -> Worker {
    let (tx, jobs) = channel::<Job>();
    let (done_tx, rx) = channel::<Done>();
    std::thread::Builder::new()
        .name("mod-worker".into())
        .spawn(move || {
            let agent = agent();
            for job in jobs {
                let done = &done_tx;
                match job {
                    Job::ListWorkshop { refresh } => {
                        let r = list_workshop(&agent, refresh, |m| {
                            let _ = done.send(Done::WorkshopProgress(m));
                        });
                        let _ = done.send(Done::Workshop(r));
                    }
                    Job::ListRust => {
                        let (mods, err) = list_rust(&agent);
                        let _ = done.send(Done::Rust(mods, err));
                    }
                    Job::Preview { key, url } => {
                        let image = load_preview(&agent, &key, &url);
                        let _ = done.send(Done::Preview { key, image });
                    }
                    Job::DownloadWorkshop(item) => {
                        let id = item.id.clone();
                        match download_workshop(&agent, &item, |f| {
                            let _ = done.send(Done::Progress { id: id.clone(), frac: f });
                        }) {
                            Ok(()) => drop(done.send(Done::Downloaded { id, rust: false })),
                            Err(e) => drop(done.send(Done::Failed { id, error: e })),
                        }
                    }
                    Job::DownloadRust(m) => {
                        let id = m.id.clone();
                        match download_rust(&agent, &m) {
                            Ok(()) => drop(done.send(Done::Downloaded { id, rust: true })),
                            Err(e) => drop(done.send(Done::Failed { id, error: e })),
                        }
                    }
                }
            }
        })
        .expect("mod worker thread");
    Worker { tx, rx: Mutex::new(rx) }
}

fn get_string(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    agent.get(url).call().map_err(|e| e.to_string())?.body_mut().read_to_string().map_err(|e| e.to_string())
}

fn get_bytes(agent: &ureq::Agent, url: &str, max: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    resp.body_mut().as_reader().take(max).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Every item on the Workshop browse pages, with details from GetPublishedFileDetails.
fn list_workshop(agent: &ureq::Agent, refresh: bool, progress: impl Fn(String)) -> Result<Vec<WorkshopItem>, String> {
    let cache = mods_dir().join("cache").join("workshop.json");
    if !refresh {
        if let Ok(s) = std::fs::read_to_string(&cache) {
            if let Ok(v) = serde_json::from_str::<Value>(&s) {
                let items: Vec<WorkshopItem> = v["items"].as_array().map(|a| a.iter().map(item_from_cache).collect()).unwrap_or_default();
                if !items.is_empty() {
                    return Ok(items);
                }
            }
        }
    }
    let mut ids: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for page in 1..=200 {
        progress(format!("Reading Workshop page {page}..."));
        let url = format!(
            "https://steamcommunity.com/workshop/browse/?appid={STEAM_APP_ID}&browsesort=trend&section=readytouseitems&actualsort=trend&days=-1&p={page}"
        );
        let html = get_string(agent, &url)?;
        let mut found = 0;
        let mut rest = html.as_str();
        while let Some(i) = rest.find("sharedfiles/filedetails/?id=") {
            rest = &rest[i + 28..];
            let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            if !id.is_empty() && seen.insert(id.clone()) {
                ids.push(id);
                found += 1;
            }
        }
        if found == 0 {
            break;
        }
    }
    let mut items = Vec::new();
    for chunk in ids.chunks(100) {
        progress(format!("Loading details {}/{}...", items.len(), ids.len()));
        let mut form: Vec<(String, String)> = vec![("itemcount".into(), chunk.len().to_string())];
        for (i, id) in chunk.iter().enumerate() {
            form.push((format!("publishedfileids[{i}]"), id.clone()));
        }
        let mut resp = agent
            .post("https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/")
            .send_form(form.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&resp.body_mut().read_to_string().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        for d in v["response"]["publishedfiledetails"].as_array().into_iter().flatten() {
            if d["result"].as_i64() != Some(1) {
                continue;
            }
            let s = |k: &str| d[k].as_str().unwrap_or_default().to_string();
            let n = |k: &str| d[k].as_u64().or_else(|| d[k].as_str().and_then(|x| x.parse().ok())).unwrap_or(0);
            items.push(WorkshopItem {
                id: s("publishedfileid"),
                title: s("title"),
                description: s("description"),
                preview_url: s("preview_url"),
                file_url: s("file_url"),
                file_size: n("file_size"),
                tags: d["tags"].as_array().map(|a| a.iter().filter_map(|t| t["tag"].as_str().map(String::from)).collect()).unwrap_or_default(),
                subscriptions: n("lifetime_subscriptions"),
            });
        }
    }
    let _ = std::fs::create_dir_all(cache.parent().unwrap());
    let arr: Vec<Value> = items
        .iter()
        .map(|i| {
            json!({"id": i.id, "title": i.title, "description": i.description, "preview_url": i.preview_url,
                   "file_url": i.file_url, "file_size": i.file_size, "tags": i.tags, "subscriptions": i.subscriptions})
        })
        .collect();
    let _ = std::fs::write(&cache, serde_json::to_string(&json!({"items": arr})).unwrap_or_default());
    Ok(items)
}

fn item_from_cache(v: &Value) -> WorkshopItem {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    WorkshopItem {
        id: s("id"),
        title: s("title"),
        description: s("description"),
        preview_url: s("preview_url"),
        file_url: s("file_url"),
        file_size: v["file_size"].as_u64().unwrap_or(0),
        tags: v["tags"].as_array().map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect()).unwrap_or_default(),
        subscriptions: v["subscriptions"].as_u64().unwrap_or(0),
    }
}

/// Unpack a legacy Workshop bundle into (path, bytes).
pub fn unpack_bundle(d: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let rd = |o: usize| -> Result<u32, String> { d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| "truncated bundle".to_string()) };
    let count = rd(4)? as usize;
    if rd(0)? != 0 || count == 0 || count > 10_000 {
        return Err("not a Workshop bundle".into());
    }
    let mut o = 8;
    let mut out = Vec::new();
    for _ in 0..count {
        let len = rd(o)? as usize;
        o += 4;
        let name = String::from_utf8_lossy(d.get(o..o + len).ok_or("truncated name")?).trim_end_matches('\0').to_string();
        o += len;
        // the data comes in chunks (2 MB each for big files): [u32 size][bytes][u32 more],
        // more = 1 while another chunk of the same file follows
        let mut data = Vec::new();
        loop {
            let size = rd(o)? as usize;
            o += 4;
            data.extend_from_slice(d.get(o..o + size).ok_or("truncated file")?);
            o += size;
            let more = rd(o)?;
            o += 4;
            if more != 1 {
                break;
            }
        }
        out.push((name, data));
    }
    Ok(out)
}

/// A bundled path made safe to join under a directory.
fn safe_rel(name: &str) -> Option<PathBuf> {
    let mut p = PathBuf::new();
    for part in name.split(['\\', '/']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." || part.contains(':') {
            return None;
        }
        p.push(part);
    }
    (!p.as_os_str().is_empty()).then_some(p)
}

fn download_workshop(agent: &ureq::Agent, item: &WorkshopItem, progress: impl Fn(f32)) -> Result<(), String> {
    if item.file_url.is_empty() {
        return Err("this item has no direct download (subscribe to it in Steam)".into());
    }
    let mut resp = agent.get(&item.file_url).call().map_err(|e| e.to_string())?;
    let total = item.file_size.max(1);
    let mut reader = resp.body_mut().as_reader();
    let mut data = Vec::with_capacity(item.file_size as usize);
    let mut buf = vec![0u8; 256 * 1024];
    let mut last = 0.0;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        let f = data.len() as f32 / total as f32;
        if f - last > 0.02 {
            last = f;
            progress(f.min(1.0));
        }
    }
    let dir = mods_dir().join("workshop").join(&item.id);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    match unpack_bundle(&data) {
        Ok(files) => {
            for (name, bytes) in files {
                let Some(rel) = safe_rel(&name) else { continue };
                let p = dir.join(rel);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(p, bytes).map_err(|e| e.to_string())?;
            }
        }
        // newer items may be plain files
        Err(_) => std::fs::write(dir.join("item.bin"), &data).map_err(|e| e.to_string())?,
    }
    let meta = json!({"id": item.id, "title": item.title, "description": item.description, "tags": item.tags, "file_size": item.file_size});
    std::fs::write(dir.join("workshop.json"), serde_json::to_string_pretty(&meta).unwrap_or_default()).map_err(|e| e.to_string())
}

fn rust_mod_from(v: &Value, dir: Option<&Path>) -> Option<RustMod> {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    let id = s("id");
    if id.is_empty() || safe_rel(&id).is_none_or(|p| p.components().count() != 1) {
        return None;
    }
    let preview = match (dir, v["preview"].as_str()) {
        (Some(d), Some(p)) if !p.is_empty() => d.join(p).to_string_lossy().to_string(),
        _ => s("preview_url"),
    };
    Some(RustMod {
        name: if s("name").is_empty() { id.clone() } else { s("name") },
        id,
        author: s("author"),
        version: s("version"),
        description: s("description"),
        preview,
        download_url: s("download_url"),
        installed: dir.is_some(),
        tweaks: tweaks_of(v),
    })
}

/// Installed mods (mods/rust/*/mod.json) plus the ones the index offers.
fn list_rust(agent: &ureq::Agent) -> (Vec<RustMod>, Option<String>) {
    let root = mods_dir().join("rust");
    let mut mods: Vec<RustMod> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            let p = e.path().join("mod.json");
            if let Some(m) = std::fs::read_to_string(&p).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| rust_mod_from(&v, Some(&e.path()))) {
                mods.push(m);
            }
        }
    }
    let mut err = None;
    let index_url = std::fs::read_to_string(mods_dir().join("index_url.txt")).unwrap_or_default().trim().to_string();
    if !index_url.is_empty() {
        match get_string(agent, &index_url).and_then(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string())) {
            Ok(v) => {
                for m in v["mods"].as_array().into_iter().flatten().filter_map(|m| rust_mod_from(m, None)) {
                    if !mods.iter().any(|x| x.id == m.id) {
                        mods.push(m);
                    }
                }
            }
            Err(e) => err = Some(format!("mod index: {e}")),
        }
    }
    mods.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    (mods, err)
}

fn download_rust(agent: &ureq::Agent, m: &RustMod) -> Result<(), String> {
    if m.download_url.is_empty() {
        return Err("no download_url in the index".into());
    }
    let text = get_string(agent, &m.download_url)?;
    let mut v: Value = serde_json::from_str(&text).map_err(|e| format!("mod.json: {e}"))?;
    let dir = mods_dir().join("rust").join(&m.id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    if m.preview.starts_with("http") {
        if let Ok(bytes) = get_bytes(agent, &m.preview, 16 << 20) {
            if std::fs::write(dir.join("preview.img"), bytes).is_ok() {
                v["preview"] = json!("preview.img");
            }
        }
    }
    v["id"] = json!(m.id);
    std::fs::write(dir.join("mod.json"), serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Preview from the cache, a local file or the web, decoded and shrunk to at most 320x180.
fn load_preview(agent: &ureq::Agent, key: &str, url: &str) -> Option<(u32, u32, Vec<u8>)> {
    let cache = mods_dir().join("cache").join("previews").join(key.replace(['/', '\\', ':'], "_"));
    let bytes = if url.starts_with("http") {
        match std::fs::read(&cache) {
            Ok(b) => b,
            Err(_) => {
                let b = get_bytes(agent, url, 16 << 20).ok()?;
                let _ = std::fs::create_dir_all(cache.parent()?);
                let _ = std::fs::write(&cache, &b);
                b
            }
        }
    } else {
        std::fs::read(url).ok()?
    };
    let img = image::load_from_memory(&bytes).ok()?;
    let img = img.resize(320, 180, image::imageops::FilterType::Triangle).to_rgba8();
    Some((img.width(), img.height(), img.into_raw()))
}

// --- maps ------------------------------------------------------------------------------------

/// Map files of a downloaded Workshop item: GoatMod.ini `mAllMaps=(FileName=...)` entries found
/// under Maps/, else every .goat/.udk/.umap in the item.
pub fn workshop_maps(id: &str) -> Vec<std::path::PathBuf> {
    let dir = mods_dir().join("workshop").join(id);
    let mut files = vec![];
    let mut stack = vec![dir.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| ["goat", "udk", "umap"].iter().any(|k| x.eq_ignore_ascii_case(k))) {
                files.push(p);
            }
        }
    }
    let ini = ["GoatMod.ini", "goatmod.ini", "Goatmod.ini"].iter().find_map(|n| std::fs::read(dir.join(n)).ok());
    let named: Vec<String> = ini
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let lk = l.to_ascii_lowercase();
            if !lk.contains("mallmaps=") {
                return None;
            }
            let at = lk.find("filename=\"")? + 10;
            let end = l[at..].find('"')? + at;
            Some(l[at..end].to_string())
        })
        .collect();
    let mut out: Vec<std::path::PathBuf> =
        files.iter().filter(|f| named.iter().any(|n| f.file_stem().is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(n)))).cloned().collect();
    if out.is_empty() {
        out = files;
    }
    out.sort();
    out
}

/// Restart on another map with these mutators ("gm-<class path>"; GOAT_MUTATORS for the new
/// process), straight into the game.
pub fn play_map_with(path: &str, mutators: &[String]) {
    play_map_with_options(path, mutators, "");
}

/// Like play_map_with, with URL options (GOAT_GAME_OPTIONS, e.g. a race's ?game=...?raceID=...).
pub fn play_map_with_options(path: &str, mutators: &[String], options: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--map") {
        args.drain(i..(i + 2).min(args.len()));
    }
    args.retain(|a| a != "--no-menu");
    info!("starting {path} with mutators {mutators:?}");
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&args).arg("--map").arg(path).arg("--no-menu");
    if options.is_empty() {
        cmd.env_remove("GOAT_GAME_OPTIONS");
    } else {
        cmd.env("GOAT_GAME_OPTIONS", options);
    }
    if mutators.is_empty() {
        cmd.env_remove("GOAT_MUTATORS");
    } else {
        cmd.env("GOAT_MUTATORS", mutators.join(","));
    }
    match cmd.spawn() {
        Ok(_) => std::process::exit(0),
        Err(e) => warn!("could not start the game on {path}: {e}"),
    }
}

/// Restart on another map: Bevy loads one level per run, so start a new process.
pub fn play_map(path: &str) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--map") {
        args.drain(i..(i + 2).min(args.len()));
    }
    info!("restarting on map {path}");
    match std::process::Command::new(exe).args(&args).arg("--map").arg(path).spawn() {
        Ok(_) => std::process::exit(0),
        Err(e) => warn!("could not start the game on {path}: {e}"),
    }
}

// --- enabled mods ------------------------------------------------------------------------

fn load_enabled() -> HashSet<String> {
    std::fs::read_to_string(mods_dir().join("enabled.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

fn save_enabled(e: &HashSet<String>) {
    let mut v: Vec<&String> = e.iter().collect();
    v.sort();
    let _ = std::fs::create_dir_all(mods_dir());
    let _ = std::fs::write(mods_dir().join("enabled.json"), serde_json::to_string_pretty(&v).unwrap_or_default());
}

fn apply_tweaks(menu: &ModMenu) {
    let mut t = Tweaks::default();
    for m in menu.rust.iter().filter(|m| m.installed && menu.enabled.contains(&m.id)) {
        t.walk_speed *= m.tweaks.walk_speed;
        t.sprint_speed *= m.tweaks.sprint_speed;
        t.jump_z *= m.tweaks.jump_z;
        t.gravity *= m.tweaks.gravity;
        t.air_control *= m.tweaks.air_control;
        t.headbutt *= m.tweaks.headbutt;
    }
    *TWEAKS.write().unwrap() = t;
}

/// Downloaded Workshop items: (published id, title), for the main menu's Workshop -> Downloaded.
pub fn downloaded_workshop() -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = std::fs::read_dir(mods_dir().join("workshop"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let text = std::fs::read_to_string(e.path().join("workshop.json")).ok()?;
                    let j: serde_json::Value = serde_json::from_str(&text).ok()?;
                    let id = e.file_name().to_string_lossy().to_string();
                    let title = j.get("title").and_then(|t| t.as_str()).unwrap_or(&id).to_string();
                    Some((id, title))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    v
}

fn scan_downloaded() -> HashSet<String> {
    std::fs::read_dir(mods_dir().join("workshop"))
        .map(|rd| rd.flatten().filter(|e| e.path().join("workshop.json").exists()).map(|e| e.file_name().to_string_lossy().to_string()).collect())
        .unwrap_or_default()
}

// --- plugin ------------------------------------------------------------------------------

pub struct ModsPlugin;

impl Plugin for ModsPlugin {
    fn build(&self, app: &mut App) {
        let worker = spawn_worker();
        let menu = ModMenu { enabled: load_enabled(), downloaded: scan_downloaded(), ..default() };
        let _ = worker.tx.send(Job::ListRust);
        app.insert_resource(worker)
            .insert_resource(menu)
            .add_message::<ModAction>()
            .add_systems(Startup, spawn_menu)
            .add_systems(Update, (toggle_menu, receive, button_presses, apply_actions, request_previews, rebuild_menu).chain());
    }
}

/// Something the menu (or a test) asks for.
#[derive(Message, Clone, Debug)]
pub enum ModAction {
    Open,
    Close,
    Tab(Tab),
    Page(i32),
    Download(String),
    Toggle(String),
    Refresh,
    /// Restart the game on a map file (Workshop map or one of the game's maps).
    PlayMap(String),
}

#[derive(Component)]
struct MenuRoot;
#[derive(Component)]
struct MenuBody;
#[derive(Component, Clone)]
struct MenuButton(ModAction);

fn toggle_menu(keys: Res<ButtonInput<KeyCode>>, menu: Res<ModMenu>, mut actions: MessageWriter<ModAction>) {
    if keys.just_pressed(KeyCode::KeyM) {
        actions.write(if menu.open { ModAction::Close } else { ModAction::Open });
    }
    if menu.open && keys.just_pressed(KeyCode::Escape) {
        actions.write(ModAction::Close);
    }
    if menu.open && keys.just_pressed(KeyCode::PageDown) {
        actions.write(ModAction::Page(1));
    }
    if menu.open && keys.just_pressed(KeyCode::PageUp) {
        actions.write(ModAction::Page(-1));
    }
}

fn button_presses(q: Query<(&Interaction, &MenuButton), Changed<Interaction>>, mut actions: MessageWriter<ModAction>) {
    for (i, b) in &q {
        if *i == Interaction::Pressed {
            actions.write(b.0.clone());
        }
    }
}

fn page_count(menu: &ModMenu) -> usize {
    let n = match menu.tab {
        Tab::Workshop => menu.workshop.len(),
        Tab::Rust => menu.rust.len(),
        Tab::Game => crate::uvm::bridge::game_mutators().len(),
    };
    n.div_ceil(PER_PAGE).max(1)
}

fn apply_actions(mut actions: MessageReader<ModAction>, mut menu: ResMut<ModMenu>, worker: Res<Worker>) {
    for a in actions.read() {
        menu.dirty = true;
        match a {
            ModAction::Open => {
                menu.open = true;
                MENU_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
                if !menu.workshop_requested {
                    menu.workshop_requested = true;
                    menu.workshop_status = Status::Busy("Loading the Steam Workshop...".into());
                    let _ = worker.tx.send(Job::ListWorkshop { refresh: false });
                }
            }
            ModAction::Close => {
                menu.open = false;
                MENU_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            ModAction::Tab(t) => {
                menu.tab = *t;
                menu.page = 0;
            }
            ModAction::Page(d) => {
                let pages = page_count(&menu) as i32;
                menu.page = (menu.page as i32 + d).clamp(0, pages - 1) as usize;
            }
            ModAction::Refresh => {
                menu.workshop_status = Status::Busy("Refreshing the Steam Workshop...".into());
                let _ = worker.tx.send(Job::ListWorkshop { refresh: true });
                let _ = worker.tx.send(Job::ListRust);
            }
            ModAction::Download(id) => {
                if menu.downloading.contains_key(id) {
                    continue;
                }
                if let Some(item) = menu.workshop.iter().find(|i| &i.id == id).cloned() {
                    menu.failed.remove(id);
                    menu.downloading.insert(id.clone(), 0.0);
                    let _ = worker.tx.send(Job::DownloadWorkshop(item));
                } else if let Some(m) = menu.rust.iter().find(|m| &m.id == id).cloned() {
                    menu.failed.remove(id);
                    menu.downloading.insert(id.clone(), 0.0);
                    let _ = worker.tx.send(Job::DownloadRust(m));
                }
            }
            ModAction::PlayMap(path) => play_map(path),
            ModAction::Toggle(id) => {
                if !menu.enabled.remove(id) {
                    menu.enabled.insert(id.clone());
                }
                save_enabled(&menu.enabled);
                apply_tweaks(&menu);
            }
        }
    }
}

fn receive(worker: Res<Worker>, mut menu: ResMut<ModMenu>, mut images: ResMut<Assets<Image>>) {
    let rx = worker.rx.lock().unwrap();
    while let Ok(d) = rx.try_recv() {
        menu.dirty = true;
        match d {
            Done::WorkshopProgress(m) => menu.workshop_status = Status::Busy(m),
            Done::Workshop(Ok(items)) => {
                info!("mod menu: {} Workshop items", items.len());
                menu.workshop = items;
                menu.workshop_status = Status::Idle;
            }
            Done::Workshop(Err(e)) => {
                warn!("mod menu: Workshop list failed: {e}");
                menu.workshop_status = Status::Error(format!("Could not reach the Steam Workshop: {e}"));
                menu.workshop_requested = false;
            }
            Done::Rust(mods, err) => {
                menu.rust = mods;
                menu.rust_status = err.map(Status::Error).unwrap_or(Status::Idle);
                apply_tweaks(&menu);
            }
            Done::Preview { key, image } => {
                if let Some((w, h, rgba)) = image {
                    let img = Image::new(
                        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                        TextureDimension::D2,
                        rgba,
                        TextureFormat::Rgba8UnormSrgb,
                        RenderAssetUsages::RENDER_WORLD,
                    );
                    let h = images.add(img);
                    menu.previews.insert(key, h);
                }
            }
            Done::Progress { id, frac } => {
                menu.downloading.insert(id, frac);
            }
            Done::Downloaded { id, rust } => {
                menu.downloading.remove(&id);
                info!("mod menu: downloaded {id}");
                if rust {
                    let _ = worker.tx.send(Job::ListRust);
                } else {
                    menu.downloaded.insert(id);
                }
            }
            Done::Failed { id, error } => {
                warn!("mod menu: {id} failed: {error}");
                menu.downloading.remove(&id);
                menu.failed.insert(id, error);
            }
        }
    }
}

/// Ask for the previews of the cards on the current page.
fn request_previews(mut menu: ResMut<ModMenu>, worker: Res<Worker>) {
    if !menu.open {
        return;
    }
    let start = menu.page * PER_PAGE;
    let wanted: Vec<(String, String)> = match menu.tab {
        Tab::Workshop => menu.workshop.iter().skip(start).take(PER_PAGE).map(|i| (format!("ws-{}", i.id), i.preview_url.clone())).collect(),
        Tab::Rust => menu.rust.iter().skip(start).take(PER_PAGE).map(|m| (format!("rust-{}", m.id), m.preview.clone())).collect(),
        Tab::Game => vec![],
    };
    for (key, url) in wanted {
        if !url.is_empty() && menu.preview_requested.insert(key.clone()) {
            let _ = worker.tx.send(Job::Preview { key, url });
        }
    }
}

// --- UI --------------------------------------------------------------------------------------

const PANEL: Color = Color::srgba(0.06, 0.07, 0.09, 0.94);
const CARD: Color = Color::srgba(0.13, 0.15, 0.18, 1.0);
const ACCENT: Color = Color::srgb(0.98, 0.78, 0.25);
const MUTED: Color = Color::srgb(0.62, 0.66, 0.72);
const BUTTON: Color = Color::srgb(0.22, 0.42, 0.75);
const BUTTON_OFF: Color = Color::srgb(0.25, 0.27, 0.3);

fn spawn_menu(mut commands: Commands) {
    commands
        .spawn((
            MenuRoot,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(8.0),
                right: Val::Percent(8.0),
                top: Val::Percent(9.0),
                bottom: Val::Percent(5.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(16.0)),
                row_gap: Val::Px(10.0),
                ..default()
            },
            BackgroundColor(PANEL),
            Visibility::Hidden,
            GlobalZIndex(10),
        ))
        .with_children(|p| {
            p.spawn((MenuBody, Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(10.0), flex_grow: 1.0, ..default() }));
        });
}

/// Steam descriptions use BBCode; keep the plain text, short.
fn plain(desc: &str, max: usize) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in desc.chars() {
        match c {
            '[' => in_tag = true,
            ']' if in_tag => in_tag = false,
            _ if in_tag => {}
            '\r' => {}
            '\n' => {
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c => out.push(c),
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > max { format!("{}...", out.chars().take(max).collect::<String>()) } else { out }
}

fn text(s: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor) {
    (Text::new(s), TextFont { font_size: bevy::text::FontSize::Px(size), ..default() }, TextColor(color))
}

fn button(p: &mut ChildSpawnerCommands, label: &str, action: ModAction, on: bool) {
    p.spawn((
        Button,
        MenuButton(action),
        Node { padding: UiRect::axes(Val::Px(14.0), Val::Px(7.0)), justify_content: JustifyContent::Center, ..default() },
        BackgroundColor(if on { BUTTON } else { BUTTON_OFF }),
    ))
    .with_children(|b| {
        b.spawn(text(label, 15.0, Color::WHITE));
    });
}

fn rebuild_menu(
    mut commands: Commands,
    mut menu: ResMut<ModMenu>,
    mut root: Query<&mut Visibility, With<MenuRoot>>,
    body: Query<Entity, With<MenuBody>>,
) {
    if !menu.dirty {
        return;
    }
    menu.dirty = false;
    if let Ok(mut v) = root.single_mut() {
        *v = if menu.open { Visibility::Inherited } else { Visibility::Hidden };
    }
    if !menu.open {
        return;
    }
    let Ok(body) = body.single() else { return };
    commands.entity(body).despawn_related::<Children>();
    let pages = page_count(&menu);
    let menu = &*menu;
    commands.entity(body).with_children(|p| {
        // header: title + tabs
        p.spawn(Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(10.0), align_items: AlignItems::Center, ..default() }).with_children(|h| {
            h.spawn(text("MODS", 26.0, ACCENT));
            button(h, &format!("Steam Workshop ({})", menu.workshop.len()), ModAction::Tab(Tab::Workshop), menu.tab == Tab::Workshop);
            button(h, &format!("Goat Rust mods ({})", menu.rust.len()), ModAction::Tab(Tab::Rust), menu.tab == Tab::Rust);
            button(h, &format!("Game mutators ({})", crate::uvm::bridge::game_mutators().len()), ModAction::Tab(Tab::Game), menu.tab == Tab::Game);
            button(h, "Refresh", ModAction::Refresh, false);
            h.spawn(text("M / Esc close   PgUp/PgDn page", 13.0, MUTED));
        });
        let (status, note) = match menu.tab {
            Tab::Workshop => (
                &menu.workshop_status,
                "Workshop mods are UnrealScript mutators and maps for the original game. Downloaded mutators run in the UnrealScript VM when enabled; downloaded maps restart the game on that map.",
            ),
            Tab::Rust => (&menu.rust_status, "Mods for this version (mods/rust/<id>/mod.json). Enabled mods change the goat and the world right away."),
            Tab::Game => (&Status::Idle, "The original game's mutators (Custom Game menu), run from GoatGame.u / Startup.upk in the UnrealScript VM."),
        };
        p.spawn(text(note, 13.0, MUTED));
        match status {
            Status::Busy(m) => {
                p.spawn(text(m.clone(), 16.0, ACCENT));
            }
            Status::Error(m) => {
                p.spawn(text(m.clone(), 16.0, Color::srgb(1.0, 0.45, 0.4)));
            }
            Status::Idle => {}
        }
        // cards
        p.spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(8.0), flex_grow: 1.0, overflow: Overflow::clip(), ..default() }).with_children(|list| {
            let start = menu.page * PER_PAGE;
            match menu.tab {
                Tab::Workshop => {
                    if menu.workshop.is_empty() && menu.workshop_status == Status::Idle {
                        list.spawn(text("No Workshop items.", 16.0, MUTED));
                    }
                    for item in menu.workshop.iter().skip(start).take(PER_PAGE) {
                        let tags = item.tags.join(", ");
                        let size = if item.file_size >= 1_000_000 { format!("{:.1} MB", item.file_size as f64 / 1e6) } else { format!("{} KB", item.file_size.div_ceil(1000)) };
                        let meta = format!("{}  |  {}  |  {} subscribers", tags, size, item.subscriptions);
                        let (label, on) = if let Some(f) = menu.downloading.get(&item.id) {
                            (format!("Downloading {:.0}%", f * 100.0), false)
                        } else if menu.downloaded.contains(&item.id) && !workshop_maps(&item.id).is_empty() {
                            let maps = workshop_maps(&item.id);
                            let err = menu.failed.get(&item.id).cloned();
                            let path = maps[0].display().to_string();
                            card(list, menu.previews.get(&format!("ws-{}", item.id)), &item.title, &meta, &plain(&item.description, 260), err, "Play map", Some(ModAction::PlayMap(path)));
                            continue;
                        } else if menu.downloaded.contains(&item.id) && crate::uvm::bridge::has_mutators(&item.id) {
                            let key = format!("ws-{}", item.id);
                            let label = if menu.enabled.contains(&key) { "Running - disable" } else { "Run mutator" };
                            let err = menu.failed.get(&item.id).cloned();
                            card(list, menu.previews.get(&format!("ws-{}", item.id)), &item.title, &meta, &plain(&item.description, 260), err, label, Some(ModAction::Toggle(key)));
                            continue;
                        } else if menu.downloaded.contains(&item.id) {
                            ("Downloaded".to_string(), false)
                        } else if menu.failed.contains_key(&item.id) {
                            ("Retry download".to_string(), true)
                        } else if item.file_url.is_empty() {
                            ("Steam only".to_string(), false)
                        } else {
                            ("Download".to_string(), true)
                        };
                        let err = menu.failed.get(&item.id).cloned();
                        card(list, menu.previews.get(&format!("ws-{}", item.id)), &item.title, &meta, &plain(&item.description, 260), err, &label, on.then(|| ModAction::Download(item.id.clone())));
                    }
                }
                Tab::Game => {
                    if menu.page == 0 {
                        list.spawn(Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(10.0), ..default() }).with_children(|r| {
                            r.spawn(text("Maps:", 16.0, MUTED));
                            for (name, file) in [("Goatville", "Maps/Level_01.goat"), ("Goat City Bay", "Maps/Level_02.goat")] {
                                button(r, name, ModAction::PlayMap(file.into()), false);
                            }
                        });
                    }
                    for g in crate::uvm::bridge::game_mutators().into_iter().skip(start).take(PER_PAGE) {
                        let key = format!("gm-{}", g.class_path);
                        let (title, desc) = g.display.split_once(": ").map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((g.display.clone(), String::new()));
                        let label = if menu.enabled.contains(&key) { "Running - disable" } else { "Run mutator" };
                        card(list, None, &title, &g.class_path, &desc, None, label, Some(ModAction::Toggle(key)));
                    }
                }
                Tab::Rust => {
                    if menu.rust.is_empty() {
                        list.spawn(text("No mods yet: put one in mods/rust/<id>/mod.json, or add an index URL to mods/index_url.txt.", 16.0, MUTED));
                    }
                    for m in menu.rust.iter().skip(start).take(PER_PAGE) {
                        let t = m.tweaks;
                        let d = Tweaks::default();
                        let mut changes = Vec::new();
                        for (n, v, base) in [
                            ("walk", t.walk_speed, d.walk_speed),
                            ("sprint", t.sprint_speed, d.sprint_speed),
                            ("jump", t.jump_z, d.jump_z),
                            ("gravity", t.gravity, d.gravity),
                            ("air control", t.air_control, d.air_control),
                            ("headbutt", t.headbutt, d.headbutt),
                        ] {
                            if v != base {
                                changes.push(format!("{n} x{v:.2}"));
                            }
                        }
                        let meta = format!(
                            "{}{}  |  {}",
                            if m.author.is_empty() { "unknown author".to_string() } else { format!("by {}", m.author) },
                            if m.version.is_empty() { String::new() } else { format!("  v{}", m.version) },
                            if changes.is_empty() { "no tweaks".to_string() } else { changes.join(", ") }
                        );
                        let (label, action) = if let Some(f) = menu.downloading.get(&m.id) {
                            (format!("Downloading {:.0}%", f * 100.0), None)
                        } else if !m.installed {
                            (if menu.failed.contains_key(&m.id) { "Retry download" } else { "Download" }.to_string(), Some(ModAction::Download(m.id.clone())))
                        } else if menu.enabled.contains(&m.id) {
                            ("Enabled - disable".to_string(), Some(ModAction::Toggle(m.id.clone())))
                        } else {
                            ("Enable".to_string(), Some(ModAction::Toggle(m.id.clone())))
                        };
                        let err = menu.failed.get(&m.id).cloned();
                        card(list, menu.previews.get(&format!("rust-{}", m.id)), &m.name, &meta, &plain(&m.description, 260), err, &label, action);
                    }
                }
            }
        });
        // footer
        p.spawn(Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(10.0), align_items: AlignItems::Center, ..default() }).with_children(|f| {
            button(f, "< Prev", ModAction::Page(-1), menu.page > 0);
            f.spawn(text(format!("Page {} / {}", menu.page + 1, pages), 15.0, Color::WHITE));
            button(f, "Next >", ModAction::Page(1), menu.page + 1 < pages);
        });
    });
}

#[allow(clippy::too_many_arguments)]
fn card(list: &mut ChildSpawnerCommands, preview: Option<&Handle<Image>>, title: &str, meta: &str, desc: &str, error: Option<String>, label: &str, action: Option<ModAction>) {
    list.spawn((Node { flex_direction: FlexDirection::Row, column_gap: Val::Px(12.0), padding: UiRect::all(Val::Px(8.0)), ..default() }, BackgroundColor(CARD))).with_children(|c| {
        let size = Node { width: Val::Px(176.0), height: Val::Px(99.0), flex_shrink: 0.0, ..default() };
        match preview {
            Some(h) => {
                c.spawn((ImageNode::new(h.clone()), size));
            }
            None => {
                c.spawn((size, BackgroundColor(Color::srgb(0.2, 0.22, 0.25)))).with_children(|n| {
                    n.spawn(text("no preview", 12.0, MUTED));
                });
            }
        }
        c.spawn(Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0), flex_grow: 1.0, flex_shrink: 1.0, ..default() }).with_children(|t| {
            t.spawn(text(title, 18.0, Color::WHITE));
            t.spawn(text(meta, 12.0, ACCENT));
            t.spawn(text(desc, 13.0, MUTED));
            if let Some(e) = error {
                t.spawn(text(e, 12.0, Color::srgb(1.0, 0.45, 0.4)));
            }
        });
        c.spawn(Node { flex_shrink: 0.0, align_self: AlignSelf::Center, ..default() }).with_children(|b| match action {
            Some(a) => button(b, label, a, true),
            None => {
                b.spawn((Node { padding: UiRect::axes(Val::Px(14.0), Val::Px(7.0)), ..default() }, BackgroundColor(BUTTON_OFF))).with_children(|x| {
                    x.spawn(text(label, 15.0, Color::WHITE));
                });
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_round_trip() {
        let mut d = vec![0, 0, 0, 0, 2, 0, 0, 0];
        for (name, data) in [("\\Scripts\\a.u\0", &b"\xC1\x83\x2A\x9E"[..]), ("\\GoatMod.ini\0", &b"[x]"[..])] {
            d.extend((name.len() as u32).to_le_bytes());
            d.extend(name.as_bytes());
            d.extend((data.len() as u32).to_le_bytes());
            d.extend(data);
            d.extend(0u32.to_le_bytes());
        }
        let files = unpack_bundle(&d).unwrap();
        assert_eq!(files[0].0, "\\Scripts\\a.u");
        assert_eq!(files[1].1, b"[x]");
        assert_eq!(safe_rel(&files[0].0).unwrap(), PathBuf::from("Scripts").join("a.u"));
        assert!(safe_rel("..\\evil").is_none());
    }

    #[test]
    fn bbcode_is_stripped() {
        assert_eq!(plain("[h1]Goat[/h1]\r\n[b]Fast[/b] goat", 100), "Goat Fast goat");
        assert_eq!(plain("abcdef", 3), "abc...");
    }
}

#[cfg(test)]
mod bundle_tests {
    use super::unpack_bundle;

    #[test]
    fn chunked_files_join() {
        // [0][2 files]; a.txt in two chunks (more = 1, then 0); b in one
        let mut d = vec![];
        let w = |d: &mut Vec<u8>, v: u32| d.extend_from_slice(&v.to_le_bytes());
        w(&mut d, 0);
        w(&mut d, 2);
        w(&mut d, 6);
        d.extend_from_slice(b"a.txt\0");
        w(&mut d, 3);
        d.extend_from_slice(b"abc");
        w(&mut d, 1);
        w(&mut d, 2);
        d.extend_from_slice(b"de");
        w(&mut d, 0);
        w(&mut d, 2);
        d.extend_from_slice(b"b\0");
        w(&mut d, 1);
        d.extend_from_slice(b"z");
        w(&mut d, 0);
        let files = unpack_bundle(&d).unwrap();
        assert_eq!(files, vec![("a.txt".to_string(), b"abcde".to_vec()), ("b".to_string(), b"z".to_vec())]);
    }
}
