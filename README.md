# Goat Simulator — Rust rebuild

A from-scratch Rust reimplementation of Goat Simulator (Coffee Stain, 2014, Unreal Engine 3 / UDK)
that loads its data **from your own installed copy** of the game. No game files are included in
this repository.

* `upk/` reads Goat Simulator's cooked UE3 packages (file version 868, LZO-compressed):
  names/imports/exports, tagged properties, textures (package + `Textures.tfc`), static meshes,
  skeletal meshes, animation sets, physics assets, materials and maps (`.goat`).
* `goat/` is the game itself: Bevy 0.19 renderer, rapier3d 0.36 physics standing in for PhysX.

## Running

```bash
cargo run -p goat --release
```

The game is expected at `C:\Program Files (x86)\Steam\steamapps\common\GoatSimulator`.
Set `GOATSIM_DIR` to point elsewhere. `--map Maps/Level_01.goat` (default) chooses the map and
`--start N` the player start.

Controls (from the game's `DefaultInput.ini`): WASD move, Shift sprint, Space jump (press again
near the top for a double jump; press just before landing for a Perfect Landing), left mouse
headbutt, E lick/grab, Q ragdoll, 1 baa, F slow motion, mouse wheel zoom, Esc releases the mouse.
In the air hold the right mouse button: W/S flip, A/D spin. Jump into a wall and keep holding a direction to wall-run
(straight at it runs up, along it runs sideways); jump again to wall-jump off it.
R respawns (the original binds R to "Special"; respawn lives in its pause menu).
M opens the mod menu: Steam Workshop mods for Goat Simulator (preview, description, download;
they're for the original game and don't run here yet) and mods for this version
(`mods/rust/<id>/mod.json`, gameplay tweaks you can enable). See `mods/README.md`.

## Tools

```bash
cargo run -p upk --bin upkdump -- <package> summary|exports [filter]|props [filter]|tex|mesh|skel|anims|phys|level|scripts <outdir>
```

`upkdump GoatGame.u scripts out/scripts/GoatGame` extracts the UnrealScript source that ships inside
the package (TextBuffer exports); gameplay is reimplemented from it. Dumps go to `out/`, which is
gitignored.

## Tests

Scripted runs open on the main monitor (`--second-monitor` to move them), never capture the mouse, print a log and exit with a
status code (they abort early when the goat leaves the expected path):

```bash
cargo run -p goat --release -- --test smoke --shot out/smoke.png
```

Scenarios: `view`, `aerial`, `smoke`, `goat`, `goatidle`, `headbutt`, `ragdoll`, `npc`, `flip`,
`jumpover`, `reactions`, `trip`, `trampoline`, `mods` (needs internet), and on Level_02 by default `wallrun`, `attack`, `route`.
`GOAT_TRACE=1` prints per-frame goat state in the trick/NPC scenarios.
Unit tests: `cargo test -p goat` (axis conversion, combo scoring).
`GOAT_PROFILE=1` prints physics and frame timings; `GOAT_SHADOWS=0` disables shadows.
