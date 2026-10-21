//! `goat --test pad`: a virtual gamepad (Bevy's own GamepadConnectionEvent / RawGamepadEvent, as
//! the gilrs backend sends them) plays the goat: the left stick walks it, A jumps, Start opens the
//! pause menu and B (Escape) closes it again.

use crate::Args;
use crate::goat::Goat;
use bevy::input::gamepad::{GamepadAxis, GamepadButton, GamepadConnection, GamepadConnectionEvent, RawGamepadAxisChangedEvent, RawGamepadButtonChangedEvent, RawGamepadEvent};
use bevy::prelude::*;

#[derive(Default)]
pub struct PadSt {
    t: f32,
    phase: u32,
    mark: f32,
    pad: Option<Entity>,
    start: Vec3,
    max_y: f32,
}

#[allow(clippy::too_many_arguments)]
pub fn pad_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<PadSt>,
    m: Res<crate::uvm::plugin::Mutators>,
    goats: Query<(&Goat, &Transform)>,
    mut connect: MessageWriter<GamepadConnectionEvent>,
    mut raw: MessageWriter<RawGamepadEvent>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("pad") {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let since = t - st.mark;
    let fail = |why: &str| -> ! {
        println!("TEST pad FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let Ok((g, tf)) = goats.single() else { return };
    st.max_y = st.max_y.max(tf.translation.y);
    let mut button = |raw: &mut MessageWriter<RawGamepadEvent>, pad: Entity, b: GamepadButton, v: f32| {
        raw.write(RawGamepadEvent::Button(RawGamepadButtonChangedEvent::new(pad, b, v)));
    };
    let ui = crate::uvm::menus::ui_open();
    match st.phase {
        0 if t > 3.0 && m.host.is_some() && g.grounded => {
            let pad = commands.spawn(Name::new("virtual pad")).id();
            connect.write(GamepadConnectionEvent::new(pad, GamepadConnection::Connected { name: "test pad".into(), vendor_id: None, product_id: None }));
            st.pad = Some(pad);
            st.start = tf.translation;
            st.phase = 1;
            st.mark = t;
        }
        0 if t > 30.0 => fail("the goat or the script host never got ready"),
        // left stick forward for 1.5 s
        1 if since > 0.3 => {
            raw.write(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(st.pad.unwrap(), GamepadAxis::LeftStickY, 1.0)));
            st.phase = 2;
            st.mark = t;
        }
        2 if since > 1.5 => {
            raw.write(RawGamepadEvent::Axis(RawGamepadAxisChangedEvent::new(st.pad.unwrap(), GamepadAxis::LeftStickY, 0.0)));
            let moved = Vec2::new(tf.translation.x - st.start.x, tf.translation.z - st.start.z).length();
            println!("  t={t:.1} left stick: walked {moved:.1} m");
            if moved < 2.0 {
                fail("the left stick didn't walk the goat");
            }
            st.phase = 3;
            st.mark = t;
        }
        // A: jump
        3 if since > 0.8 && g.grounded => {
            st.start = tf.translation;
            st.max_y = tf.translation.y;
            button(&mut raw, st.pad.unwrap(), GamepadButton::South, 1.0);
            st.phase = 4;
            st.mark = t;
        }
        4 if since > 0.1 => {
            button(&mut raw, st.pad.unwrap(), GamepadButton::South, 0.0);
            st.phase = 5;
            st.mark = t;
        }
        5 if since > 1.0 => {
            let rose = st.max_y - st.start.y;
            println!("  t={t:.1} A: rose {rose:.2} m");
            if rose < 0.3 {
                fail("A didn't jump");
            }
            st.phase = 6;
            st.mark = t;
        }
        // Start: the pause menu
        6 if since > 0.8 => {
            button(&mut raw, st.pad.unwrap(), GamepadButton::Start, 1.0);
            st.phase = 7;
            st.mark = t;
        }
        7 if since > 0.1 => {
            button(&mut raw, st.pad.unwrap(), GamepadButton::Start, 0.0);
            st.phase = 8;
            st.mark = t;
        }
        8 if since > 1.0 => {
            println!("  t={t:.1} Start: menu open {ui}");
            if !ui {
                fail("Start didn't open the pause menu");
            }
            if let Some(p) = args.shot.clone() {
                commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(p));
            }
            button(&mut raw, st.pad.unwrap(), GamepadButton::East, 1.0);
            st.phase = 9;
            st.mark = t;
        }
        9 if since > 0.1 => {
            button(&mut raw, st.pad.unwrap(), GamepadButton::East, 0.0);
            st.phase = 10;
            st.mark = t;
        }
        10 if since > 1.0 => {
            println!("  t={t:.1} B: menu open {ui}");
            if ui {
                fail("B didn't close the pause menu");
            }
            println!("TEST pad PASS");
            exit.write(AppExit::Success);
            st.phase = 11;
        }
        _ => {}
    }
    if t > 45.0 {
        fail("timeout");
    }
}
