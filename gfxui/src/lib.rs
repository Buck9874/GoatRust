//! Goat Simulator's menus and HUD are Scaleform GFx movies (AS3, SWF 21) stored in SwfMovie
//! objects. Ruffle plays them once they are plain SWFs (see `gfx_to_swf`). Frames render offscreen
//! with Ruffle's own wgpu device and come back as RGBA images.
//!
//! UE3 Scaleform conventions handled here:
//! * movies live in a folder per package (`UI_FrontEnd/FrontEnd_Assets.swf`) so relative imports
//!   such as `../UI_Common/gfxfontlib.swf` resolve (`write_movie` + `Movie::load_file`);
//! * fonts come from the `[FontLib]` movie (GfxUI.int, `UI_Common.fonts_en`) and are mapped by
//!   `[Fonts]` (`$NormalFont=Bebas Neue`): `Movie::register_fonts`.
//! * UnrealScript drives the movie through GFxObject calls: `attach` / `invoke` / `get` / `set`
//!   (a small bridge added to the local Ruffle checkout, reference/ruffle/core/src/gfx_bridge.rs).

use anyhow::{Result, anyhow};
use ruffle_core::backend::navigator::{NullExecutor, NullNavigatorBackend};
use ruffle_core::backend::ui::FontDefinition;
use ruffle_core::limits::ExecutionLimit;
use ruffle_core::tag_utils::SwfMovieData;
use ruffle_core::{Player, PlayerBuilder};
use ruffle_render_wgpu::backend::WgpuRenderBackend;
use ruffle_render_wgpu::target::TextureTarget;
use ruffle_render_wgpu::wgpu;
use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub use ruffle_core::gfx_bridge::{BridgeValue as Value, Handle, HostCall};
use ruffle_core::events::PlayerEvent;
use ruffle_core::gfx_bridge as bridge;

/// A GFx movie as a plain SWF: the signature back to "FWS" and Scaleform's private tags (codes
/// 1000+, e.g. ExporterInfo) removed. They come before FileAttributes, which Flash players only
/// honour as the first tag, so Ruffle would otherwise run the AS3 movie as AVM1.
pub fn gfx_to_swf(data: &[u8]) -> Vec<u8> {
    let mut d = data.to_vec();
    if d.get(0..3) != Some(b"GFX") {
        if d.get(0..3) == Some(b"CFX") {
            d[0..3].copy_from_slice(b"CWS"); // compressed: leave the tags alone
        }
        return d;
    }
    let nbits = (d[8] >> 3) as usize;
    let header_end = 8 + (5 + 4 * nbits).div_ceil(8) + 4;
    let mut out = Vec::with_capacity(d.len());
    out.extend_from_slice(b"FWS");
    out.extend_from_slice(&d[3..header_end]);
    let mut o = header_end;
    while o + 2 <= d.len() {
        let cl = u16::from_le_bytes([d[o], d[o + 1]]);
        let (code, mut len, mut hdr) = ((cl >> 6) as u32, (cl & 0x3f) as usize, 2usize);
        if len == 0x3f {
            if o + 6 > d.len() {
                break;
            }
            len = u32::from_le_bytes([d[o + 2], d[o + 3], d[o + 4], d[o + 5]]) as usize;
            hdr = 6;
        }
        let end = (o + hdr + len).min(d.len());
        if code < 1000 {
            out.extend_from_slice(&d[o..end]);
        }
        o = end;
        if code == 0 {
            break;
        }
    }
    let n = out.len() as u32;
    out[4..8].copy_from_slice(&n.to_le_bytes());
    out
}

/// Write a SwfMovie (`Package.Name`) as `<dir>/<Package>/<Name>.swf`.
pub fn write_movie(dir: &Path, object_path: &str, gfx: &[u8]) -> Result<PathBuf> {
    let (pkg, name) = object_path.split_once('.').ok_or_else(|| anyhow!("{object_path}: no package"))?;
    let p = dir.join(pkg).join(format!("{name}.swf"));
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(&p, gfx_to_swf(gfx))?;
    Ok(p)
}

/// An offscreen renderer on the one wgpu device all movies share (creating a device per movie
/// cost 250-700 ms each time a menu opened).
fn offscreen_renderer(width: u32, height: u32) -> Result<WgpuRenderBackend<TextureTarget>> {
    struct Shared(Arc<ruffle_render_wgpu::descriptors::Descriptors>);
    // wgpu objects are thread-safe on native backends; Descriptors only lacks the marker
    unsafe impl Send for Shared {}
    static SHARED: Mutex<Option<Shared>> = Mutex::new(None);
    let mut shared = SHARED.lock().unwrap();
    if let Some(d) = shared.as_ref() {
        let target = TextureTarget::new(&d.0.device, (width.max(1), height.max(1))).map_err(|e| anyhow!("{e}"))?;
        return WgpuRenderBackend::new(d.0.clone(), target).map_err(|e| anyhow!("{e}"));
    }
    let r = WgpuRenderBackend::for_offscreen((width, height), wgpu::Backends::PRIMARY, wgpu::PowerPreference::HighPerformance).map_err(|e| anyhow!("{e}"))?;
    *shared = Some(Shared(r.descriptors().clone()));
    Ok(r)
}

/// Create the shared wgpu device now (e.g. on a loader thread) so the first menu opens quickly.
pub fn warm_up() {
    let _ = offscreen_renderer(1, 1);
}

pub struct Movie {
    player: Arc<Mutex<Player>>,
    executor: Mutex<NullExecutor>,
    pub width: u32,
    pub height: u32,
}

impl Movie {
    /// Load a movie written by `write_movie` (`rel` = "UI_FrontEnd/FrontEnd_Assets.swf"); other
    /// movies it imports are fetched from the same folder tree.
    pub fn load_file(dir: &Path, rel: &str, width: u32, height: u32) -> Result<Self> {
        Self::load_file_with_images(dir, rel, width, height, None)
    }

    /// `load_file`, with `img://Package.Texture` URLs served from `<images>/<package.texture>`
    /// (lower case; the host writes the decoded PNGs there).
    pub fn load_file_with_images(dir: &Path, rel: &str, width: u32, height: u32, images: Option<&Path>) -> Result<Self> {
        let path = dir.join(rel).canonicalize()?;
        let data = std::fs::read(&path)?;
        let url = format!("file:///{}", path.to_string_lossy().replace('\\', "/").trim_start_matches("//?/"));
        let executor = NullExecutor::new();
        let mut navigator = NullNavigatorBackend::with_base_path(path.parent().unwrap(), &executor)?;
        navigator.image_base_path = images.map(|p| p.to_path_buf());
        Self::build(&data, &url, width, height, Some(navigator), executor)
    }

    /// Load a GFx movie from memory (imports will not resolve).
    pub fn load(data: &[u8], url: &str, width: u32, height: u32) -> Result<Self> {
        Self::build(&gfx_to_swf(data), url, width, height, None, NullExecutor::new())
    }

    fn build(swf: &[u8], url: &str, width: u32, height: u32, navigator: Option<NullNavigatorBackend>, executor: NullExecutor) -> Result<Self> {
        let movie = SwfMovieData::from_data(swf, url.to_string(), None, None).map_err(|e| anyhow!("{e}"))?;
        let renderer = offscreen_renderer(width, height)?;
        let mut builder = PlayerBuilder::new().with_renderer(renderer).with_autoplay(true).with_movie(movie).with_viewport_dimensions(width, height, 1.0);
        if let Some(n) = navigator {
            builder = builder.with_navigator(n);
        }
        Ok(Self { player: builder.build(), executor: Mutex::new(executor), width, height })
    }

    /// Register every DefineFont in a font library movie as a device font, plus Scaleform's
    /// `$Alias=Real Name` mappings.
    pub fn register_fonts(&self, font_lib_swf: &[u8], aliases: &[(&str, &str)]) -> Result<usize> {
        let buf = ruffle_core::swf::decompress_swf(font_lib_swf).map_err(|e| anyhow!("{e}"))?;
        let swf = ruffle_core::swf::parse_swf(&buf).map_err(|e| anyhow!("{e}"))?;
        let enc = ruffle_core::swf::SwfStr::encoding_for_version(swf.header.version());
        let mut p = self.player.lock().unwrap();
        let mut n = 0;
        for tag in &swf.tags {
            if let ruffle_core::swf::Tag::DefineFont2(font) = tag {
                let name = font.name.to_str_lossy(enc).to_string();
                p.register_device_font(FontDefinition::SwfTag((**font).clone(), enc));
                n += 1;
                for (alias, real) in aliases {
                    if name.eq_ignore_ascii_case(real) {
                        let mut f = (**font).clone();
                        f.name = ruffle_core::swf::SwfStr::from_bytes(alias.as_bytes());
                        p.register_device_font(FontDefinition::SwfTag(f, enc));
                    }
                }
            }
        }
        Ok(n)
    }

    /// Advance one frame of the movie (timeline, ActionScript, pending loads).
    pub fn run_frame(&self) {
        self.executor.lock().unwrap().run();
        let mut p = self.player.lock().unwrap();
        p.preload(&mut ExecutionLimit::none());
        p.run_frame();
    }

    /// Render and read the frame back as RGBA.
    pub fn capture(&self) -> Option<image::RgbaImage> {
        let mut p = self.player.lock().unwrap();
        p.render();
        let r = <dyn Any>::downcast_mut::<WgpuRenderBackend<TextureTarget>>(p.renderer_mut())?;
        r.capture_frame()
    }

    /// Something changed since the last `capture` (a movie frame ran, input, a resize).
    pub fn needs_render(&self) -> bool {
        self.p().needs_render()
    }

    pub fn player(&self) -> &Arc<Mutex<Player>> {
        &self.player
    }

    fn p(&self) -> std::sync::MutexGuard<'_, Player> {
        self.player.lock().unwrap()
    }

    /// Advance by `dt` seconds of real time (frames at the movie's frame rate, timers, loads).
    pub fn tick(&self, dt: f64) {
        self.executor.lock().unwrap().run();
        let mut p = self.p();
        p.preload(&mut ExecutionLimit::none());
        p.tick(ruffle_core::FloatDuration::from_secs(dt));
    }

    /// Draw with a transparent background (Scaleform movies sit on top of the game view).
    pub fn set_transparent(&self) {
        self.p().set_window_mode("transparent");
    }

    pub fn set_size(&mut self, width: u32, height: u32) {
        if (width, height) == (self.width, self.height) || width == 0 || height == 0 {
            return;
        }
        self.width = width;
        self.height = height;
        self.p().set_viewport_dimensions(ruffle_core::ViewportDimensions { width, height, scale_factor: 1.0 });
    }

    pub fn mouse_move(&self, x: f64, y: f64) {
        self.p().handle_event(PlayerEvent::MouseMove { x, y });
    }

    pub fn mouse_button(&self, x: f64, y: f64, down: bool) {
        let button = ruffle_core::events::MouseButton::Left;
        let ev = if down { PlayerEvent::MouseDown { x, y, button, index: None } } else { PlayerEvent::MouseUp { x, y, button } };
        self.p().handle_event(ev);
    }

    pub fn mouse_wheel(&self, lines: f64) {
        self.p().handle_event(PlayerEvent::MouseWheel { delta: ruffle_core::events::MouseWheelDelta::Lines(lines) });
    }

    /// A key by Flash key code (13 Enter, 27 Escape, 9 Tab, 32 Space, 37-40 arrows, 8 Backspace).
    pub fn key(&self, code: u32, down: bool) {
        let key = key_descriptor(code);
        self.p().handle_event(if down { PlayerEvent::KeyDown { key } } else { PlayerEvent::KeyUp { key } });
    }

    pub fn text(&self, c: char) {
        self.p().handle_event(PlayerEvent::TextInput { codepoint: c });
    }

    pub fn lookup_class(&self, name: &str) -> Result<String, String> {
        bridge::lookup_class(&mut self.p(), name)
    }

    pub fn describe(&self) -> String {
        bridge::describe_stage(&mut self.p())
    }

    pub fn describe_value(&self, v: &Value) -> String {
        bridge::describe(&mut self.p(), v)
    }

    /// The root MovieClip.
    pub fn root(&self) -> Result<Handle, String> {
        bridge::root(&mut self.p())
    }

    /// A definition (class) from the movie's domain.
    pub fn definition(&self, name: &str) -> Result<Handle, String> {
        bridge::definition(&mut self.p(), name)
    }

    /// A Scaleform variable path from the root: "root.mRootObject", "_root.a.b", "a.b".
    pub fn path(&self, path: &str) -> Result<Value, String> {
        let mut parts: Vec<&str> = path.split('.').filter(|p| !p.is_empty()).collect();
        if matches!(parts.first(), Some(&"root") | Some(&"_root") | Some(&"_level0")) {
            parts.remove(0);
        }
        let mut cur = Value::Object(self.root()?);
        for p in parts {
            let Value::Object(h) = &cur else { return Err(format!("{path}: {p} of a non-object")) };
            cur = self.get(h, p)?;
        }
        Ok(cur)
    }

    /// Set the last element of a variable path.
    pub fn set_path(&self, path: &str, v: Value) -> Result<(), String> {
        let (head, last) = path.rsplit_once('.').unwrap_or(("root", path));
        match self.path(head)? {
            Value::Object(h) => self.set(&h, last, v),
            _ => Err(format!("{path}: not an object")),
        }
    }

    /// GFxObject.AttachMovie: an instance of an exported symbol, as child `name` of `parent`.
    pub fn attach(&self, parent: &Handle, linkage: &str, name: &str, depth: i32) -> Result<Handle, String> {
        bridge::attach_movie(&mut self.p(), parent, linkage, name, depth)
    }

    pub fn create_empty_movie_clip(&self, parent: &Handle, name: &str, depth: i32) -> Result<Handle, String> {
        bridge::create_empty_movie_clip(&mut self.p(), parent, name, depth)
    }

    /// GFxObject.Invoke / ActionScriptVoid.
    pub fn invoke(&self, target: &Handle, method: &str, args: &[Value]) -> Result<Value, String> {
        bridge::invoke(&mut self.p(), target, method, args)
    }

    pub fn get(&self, target: &Handle, name: &str) -> Result<Value, String> {
        bridge::get(&mut self.p(), target, name)
    }

    pub fn set(&self, target: &Handle, name: &str, value: Value) -> Result<(), String> {
        bridge::set(&mut self.p(), target, name, &value)
    }

    /// CreateObject / CreateArray.
    pub fn construct(&self, class: &str, args: &[Value]) -> Result<Handle, String> {
        bridge::construct(&mut self.p(), class, args)
    }

    /// An AS3 function that reports `HostCall::Function { id, .. }` when called.
    pub fn host_function(&self, id: u64) -> Result<Handle, String> {
        bridge::host_function(&mut self.p(), id)
    }

    pub fn call(&self, function: &Handle, this: &Value, args: &[Value]) -> Result<Value, String> {
        bridge::call(&mut self.p(), function, this, args)
    }

    pub fn same(&self, a: &Handle, b: &Handle) -> bool {
        bridge::same(&mut self.p(), a, b)
    }
}

/// Ruffle keeps reference-counted (non-atomic) data inside a Player. A `Movie` is only ever used
/// by one thread at a time (the UnrealScript VM moves between worker threads with its whole
/// state, Movies included), so moving it between threads is fine.
unsafe impl Send for Movie {}

/// Calls ActionScript made into the host (listeners, ExternalInterface) since the last call.
pub fn take_calls() -> Vec<HostCall> {
    bridge::take_calls()
}

/// Route ExternalInterface.call to `take_calls` on this thread.
pub fn enable_host_calls(on: bool) {
    bridge::enable_host_calls(on)
}

fn key_descriptor(code: u32) -> ruffle_core::events::KeyDescriptor {
    use ruffle_core::events::{KeyDescriptor, KeyLocation, LogicalKey, NamedKey, PhysicalKey};
    let (physical, logical) = match code {
        13 => (PhysicalKey::Enter, LogicalKey::Named(NamedKey::Enter)),
        27 => (PhysicalKey::Escape, LogicalKey::Named(NamedKey::Escape)),
        9 => (PhysicalKey::Tab, LogicalKey::Named(NamedKey::Tab)),
        32 => (PhysicalKey::Space, LogicalKey::Character(' ')),
        37 => (PhysicalKey::ArrowLeft, LogicalKey::Named(NamedKey::ArrowLeft)),
        38 => (PhysicalKey::ArrowUp, LogicalKey::Named(NamedKey::ArrowUp)),
        39 => (PhysicalKey::ArrowRight, LogicalKey::Named(NamedKey::ArrowRight)),
        40 => (PhysicalKey::ArrowDown, LogicalKey::Named(NamedKey::ArrowDown)),
        8 => (PhysicalKey::Backspace, LogicalKey::Named(NamedKey::Backspace)),
        _ => (PhysicalKey::Unknown, LogicalKey::Unknown),
    };
    KeyDescriptor { physical_key: physical, logical_key: logical, key_location: KeyLocation::Standard }
}
