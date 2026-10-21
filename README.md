# Goat Simulator — Rust rebuild

A from-scratch Rust rebuild of Goat Simulator (Coffee Stain, 2014, Unreal Engine 3) that plays
the game **from your own installed copy**. No game files, extracted assets or decompiled code
are in this repository: you need Goat Simulator on Steam.

It reads the original UE3 packages straight from the install (meshes, textures, sounds,
animations, maps, Kismet and the game's compiled UnrealScript) and runs them in a new engine
built on Bevy and Rapier physics.

## What works

- Goatville and Goat City Bay: BSP, landscapes, foliage, baked lightmaps with their specular,
  sun shadows, normal maps and the original post-processing (depth of field, grain, post-process
  volumes).
- The goat: sprint, double jump, ragdoll, headbutt, lick/grab, wall runs, ladders, flips, perfect
  landings, flying for bird mutators, bikes, longboards, the Shopping Goat cart, water.
- An UnrealScript VM that runs the game's own scripts: quests, achievements, trophies,
  collectibles, music, Kismet, matinee, races (with the jockey), the inventory, the game's own
  mutators and Steam Workshop mutators and maps.
- The original Scaleform menus and HUD (main menu, custom game, options, key bindings, pause,
  credits, combo HUD) through Ruffle.
- Physics props, collision chains, APEX glass and destructibles, explosions, fire, trampolines.
- UE3 particle effects (sprites, mesh particles, ribbons, beams).
- NPCs that wander, react, panic, fight back and drown.
- Tricks, combos and score, saves, local leaderboards.
- Keyboard/mouse and controller (the original's gamepad layout).

Not done: the DLC maps, split-screen co-op, online leaderboards.

## Running

Needs Windows 10/11, a DirectX 12 or Vulkan GPU, Rust (stable) and Goat Simulator installed
through Steam.

```bash
cargo run -p goat --release
```

The game is found in your Steam library folders automatically. If it is somewhere else, set
`GOATSIM_DIR` to the folder holding `GoatGame`, or put that path in `goatsim_dir.txt` next to the
executable. A plain launch opens the original main menu; `--map Maps/Level_02.goat` starts a map
directly. `tools/package.py` builds a release folder and zip (and refuses to package game data).

## Controls

The original's defaults. Keyboard: WASD move, Shift sprint, Space jump (again in the air for a
double jump, just before landing for a perfect landing), left mouse headbutt, E lick/grab,
Q ragdoll, 1 baa, F slow motion, R use (bikes, longboards), mouse wheel zoom, Esc pause menu,
I inventory (with the Inventory mutator), M mod menu, F1 developer overlay. In the air hold the
right mouse button: W/S flip, A/D spin.

Controller: left stick move (click to sprint), right stick camera, A jump, B ragdoll, X lick,
Y use, RT headbutt, LT free look, LB baa, RB inventory, D-pad zoom, Back slow motion, Start pause.

## Mods

Steam Workshop mutators for Goat Simulator run through the VM (the mod menu, M, downloads and
enables them). Mods for this version live in `mods/rust/<id>/mod.json`; see `mods/README.md`.

## Tools

```bash
cargo run -p upk --bin upkdump -- <package> summary|exports|props|tex|mesh|skel|anims|level|scripts|disasm|font ...
```

`upkdump` reads the cooked packages; output goes to `out/`, which is gitignored and never
committed.

## Tests

`goat --test <scenario>` plays the game with scripted input, prints a log and exits with a status
code (runs abort as soon as the goat leaves the expected path). Test windows open on the main
monitor (`--second-monitor` for the other), never capture the mouse and are muted. Some
scenarios: `view`, `flip`, `wallrun`, `npc`, `reactions`, `attack`, `quests`, `race`, `mainmenu`,
`pausemenu`, `hud`, `inventory`, `mutators`, `kismet`, `pad`, `fly`. `--shot out/x.png` saves a
screenshot. Unit tests: `cargo test -p goat`.

Docs/RESUME.md has the detailed state of every system.

This is a fan project, not affiliated with or endorsed by Coffee Stain. Goat Simulator and its
content belong to Coffee Stain Studios.
