# Mods

Open the mod menu in game with **M**.

- `rust/<id>/mod.json` - mods for this version. Fields: `id`, `name`, `author`, `version`,
  `description`, `preview` (image file next to mod.json), `tweaks` (multipliers, 1.0 = original):
  `walk_speed`, `sprint_speed`, `jump_z`, `gravity`, `air_control`, `headbutt`.
- `index_url.txt` - optional URL of a mod index: `{"mods": [{"id", "name", "author", "version",
  "description", "preview_url", "download_url" (a mod.json), "tweaks"}]}`. Its mods show up in the
  menu with a Download button.
- `enabled.json` - which Rust mods are on.
- `workshop/<id>/` - Steam Workshop items downloaded from the menu (unpacked bundles: UnrealScript
  .u packages, ini files, maps). They're for the original game and don't run in this version yet.
- `cache/` - Workshop list and preview images.
