//! Gamepad input with the original's layout (GoatGame/Config/Common/DefaultInput.ini XboxTypeS_*):
//! left stick moves (L3 sprints), right stick turns the camera, A jump, B ragdoll, X lick,
//! Y use (GBA_Special), RT headbutt, LT free look, LB baa, RB inventory, D-pad up/down zoom,
//! Back slow motion, Start pause menu. The first connected pad plays; keyboard and mouse keep
//! working alongside it.

use bevy::input::gamepad::{Gamepad, GamepadButton as B};
use bevy::prelude::*;

const DEAD_ZONE: f32 = 0.2;
/// Right stick camera turn rate (radians/second at full tilt).
pub const LOOK_SPEED: f32 = 2.6;

#[derive(Resource, Default)]
pub struct PadState {
    /// L3 starts sprinting until the stick comes back to the middle (as a held Shift would).
    pub sprint_latch: bool,
}

/// Test runs ignore real input; `--test pad` lets its virtual pad through.
pub fn in_tests() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::args().collect::<Vec<_>>().windows(2).any(|w| w[0] == "--test" && w[1] == "pad"))
}

/// The pad that plays (the first connected one).
pub fn active<'a>(pads: &'a Query<&Gamepad>) -> Option<&'a Gamepad> {
    pads.iter().next()
}

/// A stick with the dead zone removed and rescaled to 0..1.
pub fn stick(v: Vec2) -> Vec2 {
    let l = v.length();
    if l < DEAD_ZONE {
        return Vec2::ZERO;
    }
    v / l * ((l - DEAD_ZONE) / (1.0 - DEAD_ZONE)).min(1.0)
}

/// The pad's buttons by their Unreal names (as DefaultInput.ini binds them), for the scripts'
/// key listeners (mutators, the inventory, class skills read GBA_* through the bindings).
pub const UE_BUTTONS: [(B, &str); 14] = [
    (B::South, "XboxTypeS_A"),
    (B::East, "XboxTypeS_B"),
    (B::West, "XboxTypeS_X"),
    (B::North, "XboxTypeS_Y"),
    (B::LeftTrigger, "XboxTypeS_LeftShoulder"),
    (B::RightTrigger, "XboxTypeS_RightShoulder"),
    (B::LeftTrigger2, "XboxTypeS_LeftTrigger"),
    (B::RightTrigger2, "XboxTypeS_RightTrigger"),
    (B::LeftThumb, "XboxTypeS_LeftThumbstick"),
    (B::RightThumb, "XboxTypeS_RightThumbstick"),
    (B::Select, "XboxTypeS_Back"),
    (B::Start, "XboxTypeS_Start"),
    (B::DPadUp, "XboxTypeS_DPad_Up"),
    (B::DPadDown, "XboxTypeS_DPad_Down"),
];

/// Menu keys from the pad (CLIK menus take arrows / Enter / Escape): (button, Unreal key).
pub const MENU_KEYS: [(B, &str); 6] = [
    (B::DPadUp, "Up"),
    (B::DPadDown, "Down"),
    (B::DPadLeft, "Left"),
    (B::DPadRight, "Right"),
    (B::South, "Enter"),
    (B::East, "Escape"),
];

/// Merge the pad into the frame's goat input.
pub fn apply(p: &Gamepad, latch: &mut bool, s: &mut crate::goat::ScriptedInput) {
    let m = stick(p.left_stick());
    s.forward = (s.forward + m.y).clamp(-1.0, 1.0);
    s.right = (s.right + m.x).clamp(-1.0, 1.0);
    if p.just_pressed(B::LeftThumb) {
        *latch = true;
    }
    if m == Vec2::ZERO {
        *latch = false;
    }
    s.sprint |= *latch;
    s.jump |= p.just_pressed(B::South);
    s.ragdoll |= p.just_pressed(B::East);
    s.lick |= p.just_pressed(B::West);
    s.special |= p.just_pressed(B::North);
    s.headbutt |= p.just_pressed(B::RightTrigger2);
    s.baa |= p.just_pressed(B::LeftTrigger);
    s.slomo |= p.just_pressed(B::Select);
    s.freelook |= p.pressed(B::LeftTrigger2);
}
