//! The base game's Inventory mutator (GGMutatorInventory / GGMutatorComponentInventory /
//! GGInventory): its scripts keep the slots and the HUD list; this feeds them the input the
//! original's key bindings do. Baa (1) while licking an item -> GGGoat.AddGrabbedItemToInventory
//! (PlayBaa does it first thing), I -> GGPlayerInputGame.ToggleInventory, and while the list is
//! open each key press goes to the HUD movie's FilterButtonInput (NavigateInventory: W/S/A/D
//! move, Baa takes the item out) and the mouse wheel to CameraZoom (ScrollInventory).
//! GGPlayerControllerGame.mInventoryOpen freezes the goat's movement (INVENTORY_OPEN).

use super::*;
use super::bridge::Host;
use std::sync::atomic::{AtomicBool, Ordering};

pub static INVENTORY_OPEN: AtomicBool = AtomicBool::new(false);

pub fn open() -> bool {
    INVENTORY_OPEN.load(Ordering::Relaxed)
}

impl Host {
    /// After `keys`: the inventory's input. `wheel` = mouse wheel steps this frame (+ = up).
    pub fn inventory(&mut self, wheel: i32) {
        let Some(goat) = self.goat else { return };
        if self.vm.get(goat, "mInventory").obj().is_none() {
            INVENTORY_OPEN.store(false, Ordering::Relaxed);
            return;
        }
        let Some(pc) = self.vm.get(goat, "Controller").obj() else { return };
        let Some(pi) = self.player_input() else { return };
        let downs = self.downs.clone();
        let (key_i, key_one) = (self.vm.sym("I"), self.vm.sym("one"));
        let is_open = |h: &mut Host| h.vm.get(pc, "mInventoryOpen").truthy();
        if downs.contains(&key_i) {
            self.vm.call_named(pi, "ToggleInventory", vec![]);
        } else if is_open(self) {
            let hud = self.vm.get(pc, "myHUD").obj().and_then(|h| self.vm.get(h, "mHUDMovie").obj());
            if let Some(hud) = hud {
                for k in downs {
                    // EInputEvent IE_Pressed
                    self.vm.call_named(hud, "FilterButtonInput", vec![Value::Int(0), Value::Name(k), Value::Byte(0)]);
                }
            }
            if wheel != 0 {
                self.vm.call_named(pi, "CameraZoom", vec![Value::Int(-wheel.signum())]);
            }
        } else if downs.contains(&key_one) && self.vm.get(goat, "mGrabbedItem").obj().is_some() {
            self.vm.call_named(goat, "AddGrabbedItemToInventory", vec![]);
        }
        INVENTORY_OPEN.store(is_open(self), Ordering::Relaxed);
    }
}
