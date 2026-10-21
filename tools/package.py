"""Build a release package of GoatRust: dist/GoatRust-<date>-win64.zip.

python -I tools/package.py [--no-build]

The package holds the game executable, the example Rust mods and a README. It contains nothing
from Goat Simulator: the game reads the player's own installed copy. The script refuses to
package if any game file, decompiled output or unexpected large file is in the folder.
"""
import datetime
import os
import shutil
import subprocess
import sys
import zipfile

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
DATE = datetime.date.today().strftime("%Y.%m.%d")
NAME = f"GoatRust-{DATE}-win64"
OUT = os.path.join(ROOT, "dist", NAME)

# extensions of Goat Simulator's own data (never shipped)
GAME_EXTS = {".upk", ".u", ".goat", ".umap", ".tfc", ".bik", ".ini", ".int", ".gfx", ".swf", ".bnk", ".ogg", ".wav", ".dds", ".tga", ".psk", ".psa"}

README = """GoatRust {date}
================

A from-scratch rebuild of Goat Simulator (Coffee Stain, 2014) in Rust. It plays the levels,
scripts, sounds and menus from YOUR OWN installed copy of Goat Simulator: nothing from the game
is included here.

Requirements
------------
- Windows 10/11 (64-bit) and a DirectX 12 or Vulkan capable GPU.
- Goat Simulator installed through Steam. The game is found in your Steam library folders
  automatically. If it is somewhere else, put its folder (the one holding "GoatGame") on one
  line in a file called goatsim_dir.txt next to GoatRust.exe, or set GOATSIM_DIR.

Running
-------
Double-click GoatRust.exe. It opens the original main menu; Play starts Goatville.
Options, key bindings, saves and achievements are kept in the "save" folder next to the exe.

Controls (the original's defaults)
----------------------------------
WASD move, Shift sprint, Space jump (again near the top for a double jump), left mouse
headbutt, E lick/grab, Q ragdoll, 1 baa, F slow motion, R use (bikes, longboards) / respawn,
mouse wheel zoom, Esc pause menu. In the air hold the right mouse button: W/S flip, A/D spin.
M opens the mod menu (game mutators, Steam Workshop mutators, mods for this version).
I opens the inventory when the Inventory mutator is on. F1 shows the developer overlay.

What works
----------
Goatville and Goat City Bay with their quests, music, Kismet, races, vehicles, water, the
inventory, mutators (the game's own and many Workshop ones) and the original menus and HUD.
The DLC maps are not done yet.

This is a fan project, not affiliated with Coffee Stain. Goat Simulator is theirs.
"""


def lint(folder):
    """No game data, no decompiled code, nothing unexpectedly big."""
    bad = []
    for dirpath, _, files in os.walk(folder):
        for f in files:
            p = os.path.join(dirpath, f)
            ext = os.path.splitext(f)[1].lower()
            rel = os.path.relpath(p, folder)
            if ext in GAME_EXTS:
                bad.append(f"game data type: {rel}")
            if os.path.getsize(p) > 200 * 1024 * 1024:
                bad.append(f"too big: {rel}")
            if any(k in rel.lower() for k in ("decomp", "scripts", "out" + os.sep)):
                bad.append(f"looks like dumped game code: {rel}")
    return bad


def main():
    if "--no-build" not in sys.argv:
        print("building (dist profile)...", flush=True)
        r = subprocess.run(["cargo", "build", "--profile", "dist", "-p", "goat"], cwd=ROOT)
        if r.returncode != 0:
            sys.exit("build failed")
    exe = os.path.join(ROOT, "target", "dist", "goat.exe")
    if not os.path.isfile(exe):
        sys.exit(f"missing {exe}")
    if os.path.isdir(OUT):
        shutil.rmtree(OUT)
    os.makedirs(OUT)
    shutil.copy2(exe, os.path.join(OUT, "GoatRust.exe"))
    # the mod menu's docs and the example mods for this version (our own files)
    mods = os.path.join(OUT, "mods")
    os.makedirs(mods)
    shutil.copy2(os.path.join(ROOT, "mods", "README.md"), mods)
    shutil.copytree(os.path.join(ROOT, "mods", "rust"), os.path.join(mods, "rust"))
    with open(os.path.join(OUT, "README.txt"), "w", encoding="utf-8", newline="\r\n") as f:
        f.write(README.format(date=DATE))
    bad = lint(OUT)
    if bad:
        sys.exit("refusing to package:\n  " + "\n  ".join(bad))
    zpath = OUT + ".zip"
    with zipfile.ZipFile(zpath, "w", zipfile.ZIP_DEFLATED) as z:
        for dirpath, _, files in os.walk(OUT):
            for f in files:
                p = os.path.join(dirpath, f)
                z.write(p, os.path.join(NAME, os.path.relpath(p, OUT)))
    total = sum(os.path.getsize(os.path.join(d, f)) for d, _, fs in os.walk(OUT) for f in fs)
    print(f"{zpath}: {os.path.getsize(zpath) / 1e6:.1f} MB zipped ({total / 1e6:.1f} MB unpacked), lint clean")


if __name__ == "__main__":
    main()
