# Patched crates

- `bevy_pbr` (0.19.1): `src/lightmap/mod.rs` gives every lightmapped entity sharing a lightmap
  image the same binding-array slot (refcounted: `allocate_shared` / `release_shared`). Stock
  0.19 allocates one slot per entity, so the 10k lightmapped meshes of Level_01 made thousands
  of bind groups and broke batching (60 -> 30 fps). Drop this patch when upstream dedupes.
