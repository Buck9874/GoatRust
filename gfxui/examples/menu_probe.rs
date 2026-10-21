//! Build a menu screen from FrontEnd_Assets symbols through the bridge, click a button and
//! render it.
//! usage: menu_probe <dumped .gfx dir (out/swf)> <out.png>
use gfxui::{HostCall, Value};
fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let src = std::path::Path::new(&args[0]);
    let fs = std::path::Path::new("out/gfx_fs");
    for name in ["UI_FrontEnd.FrontEnd_Assets", "ui_common.gfxfontlib", "ui_common.fonts_en"] {
        let data = std::fs::read(src.join(format!("{name}.gfx")))?;
        let path = name.replace("ui_common", "UI_Common");
        gfxui::write_movie(fs, &path, &data)?;
    }
    gfxui::enable_host_calls(true);
    let t = std::time::Instant::now();
    let m = gfxui::Movie::load_file(fs, "UI_FrontEnd/FrontEnd_Assets.swf", 1280, 720)?;
    m.set_transparent();
    let fonts = std::fs::read(fs.join("UI_Common/fonts_en.swf"))?;
    let n = m.register_fonts(&fonts, &[("$NormalFont", "Bebas Neue")])?;
    println!("registered {n} fonts, loaded in {:?}", t.elapsed());
    for i in 0..30 {
        m.run_frame();
        if m.root().is_ok() {
            println!("root after {} frames", i + 1);
            break;
        }
    }
    println!("{}", m.describe());
    let e = |r: Result<(), String>| {
        if let Err(e) = r {
            println!("error: {e}");
        }
    };
    // Scaleform's CLIK init callback -> host function 1
    let ext = m.definition("scaleform.gfx.Extensions").map_err(anyhow::Error::msg)?;
    let cb = m.host_function(1).map_err(anyhow::Error::msg)?;
    e(m.set(&ext, "CLIK_addedToStageCallback", Value::Object(cb)));
    let root = match m.path("root.mRootObject").map_err(anyhow::Error::msg)? {
        Value::Object(h) => h,
        other => anyhow::bail!("root.mRootObject = {other:?}"),
    };
    println!("mRootObject {}", m.describe_value(&Value::Object(root.clone())));
    let view = m.attach(&root, "ViewComponent", "5", -1).map_err(anyhow::Error::msg)?;
    let _bg = m.attach(&view, "ViewBG", "bg1", -1).map_err(anyhow::Error::msg)?;
    let title = m.attach(&view, "ViewLabel", "viewLabel", -1).map_err(anyhow::Error::msg)?;
    let button = m.attach(&view, "MainListRenderer", "defaultbutton_0", -1).map_err(anyhow::Error::msg)?;
    e(m.set(&button, "label", Value::String("RESUME".into())));
    e(m.set(&button, "x", Value::Number(200.0)));
    e(m.set(&button, "y", Value::Number(300.0)));
    // AddEventListener the way GFxClikWidget does it: o = {}; o.click = <host fn>; addEventListener("click", o.click)
    for (i, ev) in ["buttonPress", "buttonClick", "dragOut", "stateChange", "rollOver", "rollOut", "focusOut", "mouseDown", "click"].iter().enumerate() {
        let f = m.host_function(10 + i as u64).map_err(anyhow::Error::msg)?;
        let _ = m.invoke(&button, "addEventListener", &[Value::String(ev.to_string()), Value::Object(f), Value::Bool(false), Value::Number(0.0), Value::Bool(false)]);
    }
    // dataProvider arrays
    let arr = m.construct("Array", &[]).map_err(anyhow::Error::msg)?;
    e(m.set(&arr, "0", Value::String("a".into())));
    e(m.set(&arr, "1", Value::String("b".into())));
    println!("array length {:?} [1] {:?}", m.get(&arr, "length"), m.get(&arr, "1"));
    for _ in 0..5 {
        m.tick(1.0 / 30.0);
    }
    for c in gfxui::take_calls() {
        match c {
            HostCall::Function { id, args } => println!("host fn {id}: {:?}", args.iter().map(|a| m.describe_value(a)).collect::<Vec<_>>()),
            HostCall::External { name, args } => println!("external {name}: {:?}", args.iter().map(|a| m.describe_value(a)).collect::<Vec<_>>()),
        }
    }
    if let Ok(Value::Object(tf)) = m.get(&title, "textField") {
        e(m.set(&tf, "text", Value::String("PAUSED".into())));
    }
    let bx = m.get(&button, "x");
    let bw = m.get(&button, "width");
    let bh = m.get(&button, "height");
    println!("button x {bx:?} w {bw:?} h {bh:?}");
    // the stage is 1920x1080 scaled into 1280x720 (show all)
    let s = 720.0 / 1080.0;
    let (cx, cy) = ((200.0 + 60.0) * s, (300.0 + 20.0) * s);
    m.mouse_move(cx, cy);
    m.tick(1.0 / 30.0);
    println!("-- down");
    m.mouse_button(cx, cy, true);
    for c in gfxui::take_calls() {
        if let HostCall::Function { id, args } = c {
            println!("on down: host fn {id}: {:?}", match &args[0] { Value::Object(h) => m.get(h, "type"), _ => Ok(Value::Undefined) });
        }
    }
    m.tick(1.0 / 30.0);
    m.mouse_button(cx, cy, false);
    m.tick(1.0 / 30.0);
    for c in gfxui::take_calls() {
        match c {
            HostCall::Function { id, args } => println!("after click: host fn {id}: {:?} {:?}", args.iter().map(|a| m.describe_value(a)).collect::<Vec<_>>(), match &args[0] { Value::Object(h) => m.get(h, "type"), _ => Ok(Value::Undefined) }),
            HostCall::External { name, .. } => println!("after click: external {name}"),
        }
    }
    let t = std::time::Instant::now();
    let img = m.capture().ok_or_else(|| anyhow::anyhow!("no frame"))?;
    println!("capture {:?}", t.elapsed());
    img.save(&args[1])?;
    println!("saved {}", args[1]);
    Ok(())
}
