//! `goat --test fly [--shot out/fly.png]`: Feather Goat + the Flappy Wings Workshop mutator
//! (309252666) make the goat a bird (Pawn.mCanFly from FlappyWingsComponent.MakeMeFly). A jump,
//! then a flap (jump again in the air: Pawn.DoBirdFlap) every 0.6 s for 4 s must climb well past a
//! plain jump; then holding forward glides.

use crate::Args;
use crate::goat::{Goat, ScriptedInput};
use bevy::prelude::*;

pub const FLY_ITEMS: &str = "gm-GoatVanilla.GGMutatorFeatherGoat,ws-309252666";

#[derive(Default)]
pub struct FlySt {
    t: f32,
    phase: u32,
    mark: f32,
    base_y: f32,
    max_y: f32,
    next_flap: f32,
    glide_from: Vec3,
}

#[allow(clippy::too_many_arguments)]
pub fn fly_scenario(
    args: Res<Args>,
    time: Res<Time<Real>>,
    mut commands: Commands,
    mut st: Local<FlySt>,
    m: Res<crate::uvm::plugin::Mutators>,
    mut input: ResMut<ScriptedInput>,
    goats: Query<(&Goat, &Transform)>,
    mut exit: MessageWriter<AppExit>,
) {
    if args.test.as_deref() != Some("fly") {
        return;
    }
    st.t += time.delta_secs().min(0.1);
    let t = st.t;
    let since = t - st.mark;
    let fail = |why: &str| -> ! {
        println!("TEST fly FAIL at {t:.1}s: {why}");
        std::process::exit(10);
    };
    let Ok((g, tf)) = goats.single() else { return };
    st.max_y = st.max_y.max(tf.translation.y);
    match st.phase {
        0 => {
            let ready = m.host.as_ref().is_some_and(|h| !h.mutators.is_empty());
            if ready && t > 4.0 && g.grounded {
                let flight = crate::uvm::bridge::flight();
                println!("  t={t:.1} {} | flight {:?}", m.status, flight.as_ref().map(|f| (f.flap_z, f.flap_cooldown, f.air_speed, f.lift.len(), f.drag.len())));
                if flight.is_none() {
                    fail("Flappy Wings didn't make the goat a bird (mCanFly)");
                }
                st.base_y = tf.translation.y;
                st.max_y = st.base_y;
                input.jump = true;
                st.next_flap = t + 0.6;
                st.phase = 1;
                st.mark = t;
            } else if t > 40.0 {
                fail(&format!("mutators never started: {}", m.status));
            }
        }
        // flap every 0.6 s for 4 s
        1 => {
            if (since * 2.0).floor() != ((since - time.delta_secs()) * 2.0).floor() {
                println!("    t={t:.1} y {:.1} m above start, grounded {}, vy {:.1}", tf.translation.y - st.base_y, g.grounded, g.velocity.y);
            }
            input.forward = 0.0;
            if t >= st.next_flap {
                input.jump = true;
                st.next_flap = t + 0.6;
            }
            if since > 4.0 {
                let rose = st.max_y - st.base_y;
                println!("  t={t:.1} flapping: rose {rose:.1} m (a plain jump is about 4 m)");
                if let Some(p) = args.shot.clone() {
                    commands.spawn(bevy::render::view::screenshot::Screenshot::primary_window()).observe(bevy::render::view::screenshot::save_to_disk(p));
                }
                if rose < 8.0 {
                    fail("flapping didn't climb");
                }
                st.glide_from = tf.translation;
                st.phase = 2;
                st.mark = t;
            }
        }
        // glide: forward held, no flaps, until landing
        2 => {
            input.forward = 1.0;
            if g.grounded || since > 8.0 {
                let d = Vec2::new(tf.translation.x - st.glide_from.x, tf.translation.z - st.glide_from.z).length();
                let drop = st.glide_from.y - tf.translation.y;
                println!("  t={t:.1} glide: {d:.1} m forward for {drop:.1} m down in {since:.1} s");
                if d < 10.0 {
                    fail("no glide");
                }
                println!("TEST fly PASS");
                exit.write(AppExit::Success);
                st.phase = 3;
            }
        }
        _ => {}
    }
    if t > 60.0 {
        fail("timeout");
    }
}
