"""GoatRust progress board: a sunburst of every planned piece of the base game (and the DLC maps),
in the style of a project board. Writes out/promo/progress.html; render it to PNG with a
headless browser. Status: 2 = done, 1 = partly done, 0 = open."""
import html, math, os

D, P, O = 2, 1, 0
BOARD = {
    "Game files": [
        ("UE3 packages + LZO", D), ("Textures (DXT, G8, ...)", D), ("Static meshes", D),
        ("Skeletal meshes + animations", D), ("Physics assets / ragdolls", D), ("BSP geometry", D),
        ("Landscapes + paint layers", D), ("Instanced foliage", D), ("Materials (layers, masks, normal maps, specular)", D),
        ("Sounds (Ogg) + cues", D), ("Particle systems", D), ("APEX destructibles", D),
        ("Matinee", D), ("Map Kismet", D),
    ],
    "Script machine": [
        ("UnrealScript VM", D), ("Natives used by GoatGame", D), ("Kismet sequences", D),
        ("Timers + latent states", D), ("Workshop mutators", D), ("Workshop maps", D),
        ("Scaleform menus (Ruffle)", D),
    ],
    "Goat": [
        ("Walk / sprint / jumps", D), ("Ragdoll + stand up", D), ("Headbutt / kick", D),
        ("Lick / grab", D), ("Wall run + wall jump", D), ("Ladders", D), ("Air flips + spins", D),
        ("Perfect landing", D), ("The manual", D), ("Orbit camera", D), ("Inventory (I)", D),
    ],
    "World": [
        ("Props + KActors", D), ("Collision chains", D), ("Breakables", D), ("Explosives + wrecks", D),
        ("Fire", D), ("Trampolines / fans", D), ("Gravity + physics volumes", D), ("Water", D),
        ("Bicycles", D), ("Longboards", D), ("Pitching machine", D), ("Shopping cart", D),
        ("Movers (elevators, doors)", D),
    ],
    "People": [
        ("Routes + idles", D), ("Notice / applaud / angry", D), ("Knock down + ragdoll", D),
        ("Panic + flee", D), ("Attack the goat", D), ("Drowning", D), ("Script-driven NPCs", D),
        ("Race goat rider", D),
    ],
    "Score + progress": [
        ("Tricks + combos", D), ("Combo HUD", D), ("Quests", D), ("Achievements", D),
        ("Trophies (collectibles)", D), ("Saves", D), ("Races", D), ("Race leaderboards (local)", D),
    ],
    "Sound + look": [
        ("Footsteps + impacts", D), ("Ambient sounds", D), ("Music manager", D), ("Lightmaps", D),
        ("Baked sun shadows", D), ("Colour grading + fog", D), ("Decals", D),
        ("Landscape materials", D), ("Depth of field / grain", D),
    ],
    "Menus": [
        ("Main menu", D), ("Custom game + mutators", D), ("Options (saved)", D), ("Key bindings", D),
        ("Pause menu", D), ("Credits", D), ("Workshop views", D), ("News panel (offline)", D),
    ],
    "Maps": [
        ("Goatville", D), ("Goat City Bay", D), ("Goat MMO Simulator", O), ("GoatZ", O),
        ("Goat in Space", O), ("Payday", O),
    ],
}

COL = {D: "#4a86e8", P: "#c9902a", O: "#3a3a3a"}
items = [(sec, name, st) for sec, lst in BOARD.items() for name, st in lst]
n = len(items)
done = sum(1 for i in items if i[2] == D)
part = sum(1 for i in items if i[2] == P)
open_ = n - done - part
pct = (done + 0.5 * part) / n * 100
base = [i for i in items if i[0] != "Maps" or i[1] in ("Goatville", "Goat City Bay")]
base_pct = (sum(1 for i in base if i[2] == D) + 0.5 * sum(1 for i in base if i[2] == P)) / len(base) * 100

def arc(cx, cy, r0, r1, a0, a1):
    if a1 - a0 >= 2 * math.pi - 1e-6:
        a1 = a0 + 2 * math.pi - 1e-4
    la = 1 if a1 - a0 > math.pi else 0
    p = lambda r, a: (cx + r * math.cos(a - math.pi / 2), cy + r * math.sin(a - math.pi / 2))
    x0, y0 = p(r1, a0); x1, y1 = p(r1, a1); x2, y2 = p(r0, a1); x3, y3 = p(r0, a0)
    return f"M{x0:.2f},{y0:.2f} A{r1},{r1} 0 {la} 1 {x1:.2f},{y1:.2f} L{x2:.2f},{y2:.2f} A{r0},{r0} 0 {la} 0 {x3:.2f},{y3:.2f}Z"

cx = cy = 300
svg = []
a = 0.0
gap = 0.004
for sec, lst in BOARD.items():
    k = len(lst)
    span = 2 * math.pi * k / n
    sd = sum(1 for x in lst if x[1] == D) + 0.5 * sum(1 for x in lst if x[1] == P)
    frac = sd / k
    # section ring: a progress band
    svg.append(f'<path d="{arc(cx, cy, 150, 196, a + gap, a + span - gap)}" fill="#2b2b2b" stroke="#1b1b1b"/>')
    svg.append(f'<path d="{arc(cx, cy, 186, 196, a + gap, a + gap + (span - 2 * gap) * frac)}" fill="#4a86e8"/>')
    mid = a + span / 2
    tx = cx + 173 * math.cos(mid - math.pi / 2)
    ty = cy + 173 * math.sin(mid - math.pi / 2)
    rot = math.degrees(mid)
    if 90 < rot < 270:
        rot += 180
    svg.append(f'<text x="{tx:.1f}" y="{ty:.1f}" fill="#cfcfcf" font-size="11" text-anchor="middle" dominant-baseline="middle" transform="rotate({rot:.1f} {tx:.1f} {ty:.1f})">{html.escape(sec)}</text>')
    # one outer slice per item
    for j, (name, st) in enumerate(lst):
        a0 = a + span * j / k
        a1 = a + span * (j + 1) / k
        svg.append(f'<path d="{arc(cx, cy, 202, 290, a0 + gap, a1 - gap)}" fill="{COL[st]}" opacity="0.92"><title>{html.escape(sec)}: {html.escape(name)}</title></path>')
    a += span
svg.append(f'<circle cx="{cx}" cy="{cy}" r="144" fill="#262626"/>')
svg.append(f'<text x="{cx}" y="{cy - 34}" fill="#cfcfcf" font-size="15" text-anchor="middle">Whole board</text>')
svg.append(f'<text x="{cx}" y="{cy + 10}" fill="#ffffff" font-size="44" font-weight="700" text-anchor="middle">{pct:.1f}%</text>')
svg.append(f'<text x="{cx}" y="{cy + 38}" fill="#bdbdbd" font-size="13" text-anchor="middle">{done} of {n} items done</text>')

rows = []
for sec, lst in BOARD.items():
    k = len(lst)
    sd = sum(1 for x in lst if x[1] == D)
    sp = sum(1 for x in lst if x[1] == P)
    f = (sd + 0.5 * sp) / k * 100
    rows.append(f'<div class="row"><span class="sec">{html.escape(sec)}</span><div class="bar"><div style="width:{f:.0f}%"></div></div><span class="num">{sd + sp}/{k}</span></div>')

page = f"""<!doctype html><html><head><meta charset="utf-8"><title>GoatRust progress</title>
<style>
body{{background:#161616;color:#e8e8e8;font-family:Segoe UI,Arial,sans-serif;margin:0;padding:22px 26px;width:1100px}}
h1{{margin:0 0 4px;font-size:26px}} .sub{{color:#a8a8a8;margin-bottom:16px;font-size:14px}}
.cards{{display:flex;gap:14px;margin-bottom:14px}}
.card{{flex:1;background:#1f1f1f;border:1px solid #333;border-radius:8px;padding:14px 16px}}
.card .t{{font-size:13px;color:#cfcfcf}} .card .v{{font-size:34px;font-weight:700;margin:6px 0}}
.card .d{{font-size:12.5px;color:#a8a8a8;line-height:1.4}}
.pbar{{height:7px;background:#333;border-radius:4px;margin-top:10px;overflow:hidden}} .pbar div{{height:100%;background:#4a86e8}}
.note{{color:#a8a8a8;font-size:13px;margin:4px 0 14px;max-width:760px;line-height:1.45}}
.panel{{background:#1f1f1f;border:1px solid #333;border-radius:8px;padding:14px 16px;display:flex;gap:20px}}
.legend span{{display:inline-block;margin-right:14px;font-size:13px;color:#cfcfcf}}
.dot{{display:inline-block;width:10px;height:10px;border-radius:2px;margin-right:6px;vertical-align:-1px}}
.side{{flex:1;padding-top:10px}} .row{{display:flex;align-items:center;gap:10px;margin:7px 0;font-size:13.5px}}
.sec{{width:140px;color:#d8d8d8}} .bar{{flex:1;height:8px;background:#333;border-radius:4px;overflow:hidden}} .bar div{{height:100%;background:#4a86e8}}
.num{{width:44px;text-align:right;color:#bdbdbd}}
</style></head><body>
<h1>Goat Simulator Rust rebuild: how far along</h1>
<div class="sub">Board as of 19 Oct 2026. Runs on the original game files from your own Steam copy; nothing from the game is shipped.</div>
<div class="cards">
 <div class="card"><div class="t">Base game rebuilt (planned items)</div><div class="v">{base_pct:.1f}%</div>
  <div class="d">Goatville + Goat City Bay and everything they use: {sum(1 for i in base if i[2] == D)} of {len(base)} items done. Partly done items count half.</div><div class="pbar"><div style="width:{base_pct:.1f}%"></div></div></div>
 <div class="card"><div class="t">Scripted gameplay tests</div><div class="v">30</div>
  <div class="d">Automated runs that play the game (ride a bike, drive the shopping cart, use the inventory, run a race, break the greenhouse, finish the main menu...) and fail on anything off script.</div><div class="pbar"><div style="width:100%"></div></div></div>
 <div class="card"><div class="t">Maps playable</div><div class="v">2 of 6</div>
  <div class="d">Both base-game maps play with their quests, music and Kismet. The four DLC maps are next.</div><div class="pbar"><div style="width:33.3%"></div></div></div>
</div>
<div class="note">The board lists the game's features I've written down to rebuild, not lines of code: items differ a lot in size, so it says how much of the plan is done rather than an exact share of the game.</div>
<div class="panel">
 <div><div class="legend"><span><i class="dot" style="background:#4a86e8"></i>Done</span><span><i class="dot" style="background:#c9902a"></i>Partly done</span><span><i class="dot" style="background:#3a3a3a"></i>Open</span></div>
 <svg width="600" height="600" viewBox="0 0 600 600">{''.join(svg)}</svg></div>
 <div class="side"><div style="font-size:15px;color:#cfcfcf">Whole board</div><div style="font-size:34px;font-weight:700">{pct:.1f}%</div>
  <div class="pbar" style="margin-bottom:14px"><div style="width:{pct:.1f}%"></div></div>
  <div class="row"><span class="sec"><i class="dot" style="background:#4a86e8"></i>Done</span><span class="num">{done}</span></div>
  <div class="row"><span class="sec"><i class="dot" style="background:#c9902a"></i>Partly done</span><span class="num">{part}</span></div>
  <div class="row"><span class="sec"><i class="dot" style="background:#3a3a3a"></i>Open</span><span class="num">{open_}</span></div>
  <div style="margin:16px 0 6px;color:#a8a8a8;font-size:13px">By section</div>
  {''.join(rows)}
 </div>
</div></body></html>"""
out = os.path.join(os.path.dirname(__file__), "..", "out", "promo", "progress.html")
open(out, "w", encoding="utf-8").write(page)
print(f"{n} items: {done} done, {part} partly, {open_} open; board {pct:.1f}%, base game {base_pct:.1f}%")
