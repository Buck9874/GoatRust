//! Reader for Goat Simulator's UE3 (UDK, file version 868) cooked packages.
//! Reads the user's own installed game files; nothing from the game is bundled.

pub mod anim;
pub mod apex;
pub mod level;
pub mod lzo;
pub mod font;
pub mod material;
pub mod matinee;
pub mod mesh;
pub mod package;
pub mod particles;
pub mod physasset;
pub mod props;
pub mod reader;
pub mod registry;
pub mod script;
pub mod skel;
pub mod sound;
pub mod texture;

pub use package::Package;

/// Default Steam install location.
pub const DEFAULT_GAME_DIR: &str = r"C:\Program Files (x86)\Steam\steamapps\common\GoatSimulator";

/// The user's Goat Simulator install: GOATSIM_DIR, else a `goatsim_dir.txt` next to the
/// executable (one line, the folder), else the default Steam folder, else any Steam library
/// folder (steamapps/libraryfolders.vdf) holding steamapps/common/GoatSimulator.
pub fn game_dir() -> std::path::PathBuf {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(find_game_dir).clone()
}

fn find_game_dir() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Some(d) = std::env::var_os("GOATSIM_DIR") {
        return d.into();
    }
    let has_game = |d: &std::path::Path| d.join("GoatGame").join("CookedPC").join("GoatGame.u").is_file();
    if let Some(txt) = std::env::current_exe().ok().and_then(|e| std::fs::read_to_string(e.with_file_name("goatsim_dir.txt")).ok()) {
        let d = PathBuf::from(txt.trim().trim_matches('"'));
        if has_game(&d) {
            return d;
        }
    }
    let default = PathBuf::from(DEFAULT_GAME_DIR);
    if has_game(&default) {
        return default;
    }
    for steam in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
        let vdf = std::fs::read_to_string(PathBuf::from(steam).join("steamapps").join("libraryfolders.vdf")).unwrap_or_default();
        for line in vdf.lines() {
            let parts: Vec<&str> = line.split('"').filter(|p| !p.trim().is_empty()).collect();
            if let [k, v] = parts.as_slice() {
                if k.eq_ignore_ascii_case("path") {
                    let d = PathBuf::from(v.replace("\\\\", "\\")).join("steamapps").join("common").join("GoatSimulator");
                    if has_game(&d) {
                        return d;
                    }
                }
            }
        }
    }
    default
}

pub fn cooked_dir() -> std::path::PathBuf {
    game_dir().join("GoatGame").join("CookedPC")
}
