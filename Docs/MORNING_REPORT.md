# Morning report: 2026-10-18 overnight

Everything below runs on the original game data and scripts from your Steam install. Each
feature has a `--test` scenario, and they all pass (see the end).

## What's new

**The game starts in the original main menu.** Launch `goat.exe` with no arguments.
- Play opens Custom Game, which lists both maps and the mutators you've unlocked.
- Play! with Goatville and no mutators starts playing right there.
- Picking another map or any mutators restarts the game on that map with them.
- Options are saved to `save/settings.json`: FOV, volumes and mouse settings stick between runs.
- `--map ...` or `--no-menu` skips the menu, as before.

**Quests, achievements and trophies.** These run the game's own quest manager.
- Goatville has 37 quests.
- The HUD shows the "COMPLETED" banner when you finish one.
- The pause menu's Quests panel lists them.
- Progress is saved to `save/progression.json`.
- The 30 hidden goat trophies are in the world, spinning and bobbing. Walk into one to pick it up.
- Achievements unlock locally into `save/achievements.json` and show a message. They also unlock
  mutators, the way Steam achievements do in the original. To unlock every mutator, set
  `GOAT_UNLOCK_ALL=1`.

**Breakables.** 154 vases, plates, melons, fences, greenhouse windows and gas station parts
break into real pieces, plus 124 props that become breakables when hit. You break them by
headbutting or sprinting into them, or by throwing things at them. Each one scores "Broke ...".

**Explosions.** 137 gas cans, barrels, gas pumps and cars explode when headbutted, or when hit
hard enough.
- Blasts push props, knock NPCs over and launch the goat.
- They set off other explosives nearby, with fire and smoke effects and sound.
- Cars turn into their burnt-out wreck instead of vanishing.

**Sound and music.**
- 46 placed ambient sounds now play around Goatville, fading with distance.
- Music follows the game's music manager. Goatville has no background music in the original
  either; try the Deadgoa7 mutator and press R.

**Level volumes.**
- Ladders are climbable: walk into one and hold W. 23 on Goatville.
- Invisible walls (blocking volumes) now block.
- Zero/low-gravity areas and wind/current zones work.

**Fire.** Anything caught in a blast burns for about 9 seconds (the goat included). Burning NPCs
run around in panic, and burning pawns set others alight. Explosions also flash an orange light.

**Visuals.**
- Baked lightmaps, from yesterday evening.
- Glass is see-through now.
- The map's own colour grading (its LUT: a bit more contrast and saturation) and its height fog
  (a light blue haze). Explosions leave scorch marks.
- Particle smoke and fire no longer cast black shadow blobs.
- Explosion effects use their real textures (they were white/pink squares).
- 235 props that were missing appear now, including most gas cans: their meshes live in
  Startup.upk.

**More scoring, like the original.**
- Props you hit keep scoring as they crash into things. The game calls this the collision chain:
  a crate you headbutt into a fence, and the fence into a car, all score.
- A prop flying over 20 m before it hits something or someone scores "Skillshot".
- Trampolines score: "jump on Trampoline" (50), and "jump from Trampoline to Trampoline" (500)
  when you bounce between two of them. Mattresses and fans score half.

**Bikes and longboards.** Walk up to a bicycle (7 in Goatville, 12 in Goat City Bay) or a
longboard (Goat City Bay) and press R to get on; R again gets off. On a bike:
- W pedals and S brakes, and the bike steers where the camera looks.
- Space jumps; A/D in the air spin the bike.
- On the ground, A/D play the original balance game: the bike keeps tipping one way, and you
  press the other way to right it. Fall over, crash hard or tip over and you're thrown off as a
  ragdoll.

Longboards steer with A/D. See Docs/DevLog/2026-10-18_bike.png and _longboard.png.

**Shopping Goat.** Turn on the Shopping Goat mutator (M → Game mutators) and you start in a
shopping cart, as in the original: W/S drive, A/D steer, Space hops, and you can't get out (R does
nothing). Like the original, the goat itself is hidden; the cart is all you see. It plays the
cart's own rolling, jump, landing and crash sounds. `goat --test cart` checks it. See
Docs/DevLog/2026-10-18_shopping_cart.png.

Along the way, characters and props with layered materials look more like the original:
- Wire and mesh parts are see-through now: the masked parts of skinned materials are cut out.
- When a material multiplies textures together or tints them, the result is worked out ahead of
  time instead of showing just the first texture.

Bike and board jumps now fire once per Space press, as in the original. Before, holding Space
could add a second push.

**Inventory.** Turn on the Inventory mutator (M → Game mutators). Lick something and press 1 to
stash it: it disappears and the chest icon at the bottom right counts it. Press I to open the
original inventory grid; W/A/S/D move through it, 1 takes the selected item out in front of the
goat, and I closes the list. The goat stands still while the list is open. The HUD's key hints
now show your keys ("INVENTORY (I)", "NAVIGATE: W S A D"). `goat --test inventory` checks it
(Docs/DevLog/2026-10-18_inventory.png).

**Race jockey.** Races put a rider on the goat again, as the original does: a copy of one of the
map's people, sitting on the goat's back (Docs/DevLog/2026-10-18_race_rider.png).

**Race leaderboard (offline).** The "Finished" popup lists your best time per race
("1. <your Steam name> - 6.07"). Without Steam's online stats there are no friends' times; your
best is kept in save/race_best.json and replaced when you beat it.

**Main menu news.** The news panel shows the original's offline message, "Failed To Load News!",
instead of a placeholder (the game fetched news from the internet; nothing goes online here).

**Depth of field and film grain.** Goatville's post-process settings now apply: a faint softening
of distant things (its blur is only 1 pixel wide, as in the original) and a very light grain.
Goat City Bay doesn't use them. GOAT_NO_POST=1 turns them off.

**Level materials.** Props whose material mixes several textures or tints them now show the mix
instead of just the first texture (31 materials in Goatville, baked in 0.7 s at load).

**Particles.** Exploding things throw real 3D debris (mesh particles), orbiting sparks and smoke
circle as they should, ground-hugging effects lie flat (axis lock), and particles that collide
bounce off the ground and walls.

**Script coverage.** The game scripts' remaining native calls in the base maps are handled; one
of them now fires Kismet events from script (the goat standing up, abilities, player spawned),
which weren't firing before. The landscape materials were checked: neither map's ground has
normal maps or specular in the original, so there was nothing to add there.

**Second round (19 Oct).**
- Workshop → Downloaded shows "Subscribed" for every downloaded mod (the view's refresh timer
  never ran under the paused menu). Timers on menu views now run while the game is paused,
  and one-shot script timers fire once instead of repeating.
- The shopping cart's weight comes from the game data now (its physics boxes x the cart
  material's density 130 = 258 kg).
- PostProcessVolumes work: inside one, its depth of field and colour (highlights, mid tones,
  shadows, desaturation, colorize) replace the level's, blending in over its interpolation time.
  Goatville has 3 (up high and underground), Goat City Bay 8 (the two under the sea give it
  its underwater look).
- Normal maps on the level's props and buildings (221 meshes; GOAT_NO_NORMALMAPS=1 compares),
  and specular where a material has it, including the original's specular from its directional
  lightmaps (the baked light's glint, from the decoded UE3 lightmap shader).
- Sun shadows now reach 450 m, so people, props and cars far away still sit in the buildings'
  shadow (UE3 used a coarse precomputed shadow map for that). Doors and elevators are lit like
  other moving things, as in the original.
- Ribbon trails (the Blue Streak mutator's) and beams (used by the DLC maps) are drawn, and
  particles can spawn by distance moved (SpawnPerUnit).

**Baked sun shadows.** Buildings and trees now cast the original game's baked, soft sun
shadows at any distance, not just near the camera. The goat, people and props still cast
live shadows on top. Glass no longer throws a solid shadow.
(Docs/DevLog/2026-10-18_baked_shadows_*.png)

**Main menu, complete.** Every main menu button now does something:
- Credits roll the original credits (they start slowly at the bottom).
- Workshop → Downloaded lists the 55 mods you've downloaded with the mod menu (M).
- DLC, Leaderboards, Achievements and Workshop → Browse open the matching Steam page in your
  browser, which is what the Steam overlay did.
- Options → Apply now really changes the resolution and fullscreen.

**Races (Goat City Bay).** Walk into a race start (the regular race or the bike race), say yes
to the popup, and the map restarts in the original race mode:
- a 3-2-1 "GOAT!" countdown, then a timer;
- checkpoint gates that light up one after another;
- a "Finished" popup with your time and a restart option.
The bike race puts a bike in front of you, and only counts the gates you pass on it.

**Water (Goat City Bay).** The sea works like the original now.
- It floats you; a ragdolled goat paddles along with WASD, Space hops out, and you can only
  get up near the shore.
- Fire goes out in water.
- People who end up in the sea flail and blow bubbles until they're out.
(Docs/DevLog/2026-10-18_water.png)

**Pitching machine (Goatville baseball field).** Step into the batter's box and it starts
firing balls at you. Walk into the machine to wear it on your back, then press R to pitch. Hit
someone for "Beanball at …"; hold something grilled for "Flaming Beanball".
(Docs/DevLog/2026-10-18_pitching_machine.png)

**The manual.** Tap S then W quickly to rear up on your hind legs. Keep the meter in the middle
with W/S while you glide along. It scores 200 a second and keeps the combo going. Jump and land
to bank it. Fall off and you lose the whole combo, and no new combo can start for 5 s. The
original HUD meter shows while you balance (Docs/DevLog/2026-10-18_manual.png).

**Goat City Bay has its music now.** The game's music manager starts with the map and plays the
map's background track.

**Faster loading.** The level loads in about 3.4 s instead of 6.8 s.

## Try it

- Plain launch: main menu, then Play, then Play!.
- In game, find a greenhouse and headbutt the glass.
- Headbutt a red gas can.
- Look for ladders on house walls.
- Press Esc, then Quests.

Tests (silent, on the main monitor):

```
goat --test mainmenu | quests | music | apex | explode | ladder | manual | bike | longboard | cart | inventory | water | baseball | race | bikerace | racepopup
```

Older ones still pass: pausemenu, menuflow, hud, headbutt (now also checks the collision chain),
trampoline (now also checks the bounce trick), kismet, jetpack, mutators, view.

## Discord post

Everything for the post is in `out/promo/discord`: 7 screenshots (JPG), the 15 s gameplay clip
(`gameplay_15s.mp4`), the progress graph (`8_progress.png`) and the post text (`POST.txt`, tags
Rewrite / Port · WIP, following the channel rules: no game files, no download links). It's 8.8 MB
in all. To rebuild the graph after more work, edit the board in `tools/progress_board.py`, run
it, and render `out/promo/progress.html` with headless Chrome.

The longboard test had started failing: the test turns the board to face open ground, and the
turned deck swung into a kerb and got pushed under the pavement. The test now lifts the board
30 cm when it turns it, and passes again.

## Known issues / not done

- DLC maps (Goat MMO, GoatZ, Goat in Space, Payday) are saved for last, as you asked.
- There's no climbing animation in the goat's set, so ladders reuse the wall-run-up legs.
- Breakables always shatter completely, and their pieces stay. The original does the same: it
  hits APEX with 10 million damage.
- Race leaderboards only show your own best (friends' times are online stats).
- DLC ownership can't be read offline, so DLC-gated Kismet counts as not owned
  (GOAT_OWN_DLC=1 says owned).
- No NPCs ride bikes here (none do on these maps in the original either).
- Workshop → Downloaded shows "N/A" in its status column.
- The flip test sometimes lands the backflip in a ragdoll; it's timing-sensitive, and it passes
  on reruns.
- With quests running, the VM costs about 3 ms per frame.
- Your Steam achievements are read from Steam's local cache (read-only): you have Angelgoat and
  Demogoat, so those mutators are unlocked. Use `GOAT_UNLOCK_ALL=1` to unlock the rest.

## Asset wishlist

None needed: everything comes from the game files.
