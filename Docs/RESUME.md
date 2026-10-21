# Goat Simulator Rust rebuild — where things stand

Started 2026-10-16. The game reads the user's Steam install (`upk::game_dir()`, env `GOATSIM_DIR`).

## Done

- **Package reader (`upk`)**: UE3 v868 summary, LZO/zlib chunks, names/imports/exports,
  tagged properties (state-frame aware for actors), struct arrays, archetype-merged props.
- **Textures**: Texture2D mips from package or `Textures.tfc` (LZO chunks), DXT1/3/5, BC5,
  A8R8G8B8, G8. 2500/2501 textures in Level_01 decode (PF_V8U8 missing).
- **Static meshes**: all 430 in Level_01 (positions, packed normals, half/float UVs, sections).
- **Materials**: diffuse texture/colour found by walking the expression graph from DiffuseColor
  (static switches, MIC texture/vector parameter overrides), masked/translucent/two-sided.
- **Level**: StaticMeshActors, KActors (GGKActor), landscape (heights from the heightfield
  collision components), player starts, lights. Level_01 loads in ~0.7 s (release).
- **Baked lighting** (`goat/src/lightmap.rs`, 2026-10-18): the cooked FLightMap2D of every
  static StaticMeshComponent (LODData[0]), foliage instance (component lightmap + per-instance
  LightmapUVBias), LandscapeComponent (lightmap first in its native data; vertex UVs from UE's
  GetTerrainExpandPatchCount padding math, unit-tested against Level_01's 1024^2 atlases) and BSP
  node (ModelComponent elements -> node lists, FVert.ShadowTexCoord). Directional lightmaps:
  T0 NormalizedAverageColor x scale0 times avg(T1 DirectionalMaxComponent x scale1) (decoded HQ
  pixel shader, `out/shaders/hq0.asm`); both DXT1 + sRGB; coefficient 2 is the stripped simple
  lightmap. Each atlas pair is baked once with its components' scales, the 683 atlases are
  shelf-packed into 4096-wide RGB9E5 pages (Level_01: 84 Mtexels on 11 pages, ~2.2 s at load) and
  given to Bevy `Lightmap` (UV_1 = the mesh's LightMapCoordinateIndex set). `lightmap_exposure`
  makes 1.0 = a UE3 light of Brightness 1; GlobalAmbientLight skips lightmapped meshes (dynamic
  props, NPCs and the goat keep it). The sun is now the map's DominantDirectionalLight (rotation,
  Brightness 3.5, LightColor), its props after the cooked whole-scene shadow depth map
  (TArray<WORD> before the tagged props). `GOAT_LIGHTMAPS=0` turns the bake off. Needs the
  patched bevy_pbr in `patches/` (stock 0.19 gives every lightmapped entity its own slot: 60 ->
  30 fps). Before/after shots: `out/lightmaps/`.
- **Goat**: skeletal mesh `goat.Mesh.goat` (34 bones) GPU-skinned, 63 animations from
  `Goat_Anim_01` (raw key tracks kept in the cooked data), crossfades + one-shots (Ram, Bite).
- **Movement** (ported from the native code, see "Decompiled game"): PlayerMove acceleration on
  the pawn's own axes (A/D strafe, S backs up) and UpdateRotation's RInterpTo to the camera yaw
  (mPawnTurnInterpSpeed 5); GGPawnBase walking CalcVelocity 0x19501c0 (turn friction 8, brake when
  the input is >72 deg off the motion), its braking 0x1952bd0 (slides when fast and facing the
  motion), max speed by direction 0x1952ff0 (forward/strafe 250/reverse 200 blended by d^2);
  physFalling air accel AccelRate x AirControl (bLimitFallAccel). Braking event: Brake animation +
  Goat_Slide_Loop_Cue above walk speed. Values from `Default__GGGoat` (walk 500, sprint 1200,
  accel 1175, JumpZ 650, double jump +100 within 500 of apex, gravity -750). Unit tests in
  goat.rs `movement_tests`.
- **Camera** (GGCameraModeOrbital): zoom 420 (20-1500, 50 per notch, lerp x3), pitch -3072 at start,
  ragdoll target offset easing, walk/sprint bounce, PreventClipping (world + people block, props
  don't), horizontal FOV 70 (GGEngine settings).
- **Abilities**: headbutt (GGAbilityHorn: range 50, impulse 70000), lick-grab (range 100, joint).
- **Ragdoll**: goat_Physics PhysicsAsset (27 bodies, 26 limited joints) driven onto the skeleton.
- **Physics props**: KActors use their RB_BodySetup convex hulls + PhysicalMaterial density;
  dormant (fixed) until touched, like UE3 KActors that start asleep. Physics ~0.02 ms idle.
- **Registry** (`upk::registry`): map + GoatGame.u + Startup.upk with cross-package import and
  archetype resolution (script-package imports drop their file-name prefix).
- **NPCs**: 92 GGNpc in Level_01 with their meshes, material overrides, placed animations
  (Characters_Anim_01 etc.), physics-asset ragdolls; knocked by headbutt or sprinting into them,
  stand up after mStandUpDelay (7.5 s), panic-run at 700 uu/s, then idle.
- **Audio**: SoundCue -> SoundNodeWave Ogg (all 186 goat sounds decode); baa (1), jump, land,
  NPC knocked/panic. Muted in test runs.
- **Score**: GGComboManager port (5 s alive, 0.35 repeat decay, max frequency 4, multiplier
  formula), tricks: ability (headbutt/lick, air x1.2), ragdoll (1 per 0.5 s), perfect landing 750.
  Prop/NPC scores and names from GGPhysicalMaterialProperty. HUD top-right.
- Slow motion (F) = game speed 0.25 like GGGameInfo.SetSlomo.
- **Landscape layers**: weightmaps -> per-vertex weights; layer textures + tiling read from the
  landscape material (TextureSampleParameter2D x ScalarParameter / TerrainLayerCoords
  MappingScale); blended in `goat/src/landscape.wgsl` (ExtendedMaterial on StandardMaterial).
- **Air tricks** (`goat/src/air.rs`): free-look (RMB) flips/spins at the GGPlayerInputGame rate
  (100000 uu/s, interp 2.2 -> 5); GGTrickRotation naming/scoring ("Double BackFlip", 180s);
  landing rules from GGGoat.Landed (bad rotation, facing away at speed, impact > 1780 uu/s all
  ragdoll; Perfect Landing only saves a hard landing); jump-over (90 uu under the feet).
- **Wall run** (PHYS_WallRun), checked against the native code with REA/Ghidra
  (`out/rea/GoatGame-native.exe`):
  - start `0xb2c810` (from physFalling `0xb39400` hit handling): world geometry, CanWallRun,
    mReadyToWallRun, Falling; strict -0.1 < nz < 0.8, Vz > -100, facing.normal2D < 0.25 ->
    Vz = 300 (or 300 - 200*dot when dot < mTresholdWallRunDot, GGGoat 0); Floor = hit normal;
    horizontal velocity kept.
  - per frame `0xb30060` (vtable slot after physWalking): input projected on the wall, horizontal,
    at AccelRate; gravity; velocity kept on the wall plane; horizontal speed capped at
    mWallRunSpeed * clamp(1 + facing.normal2D, 0, 1) (head-on = straight up); falls off when
    Vz <= -100, no input, wall farther than mMaxStepOverWallBump/2 (10 uu) or facing.normal > 0.25.
  - wall jump = Pawn.DoWallJump (Vz = 0, += 1000 * Normal(Normal2D(Floor) + up)).
  - "Wall Run" 75 after 0.7 s, "Wall Jump" 50. `GOAT_TRACE=1` prints why a wall run ended.
- **NPC reactions** (GGAIController): notice once per goat (look 5 s + sound), applaud
  Rotation/PerfectLanding tricks and wall runs (50% applaud anim / 50% point), interval
  cool-downs (notice 25-35 s, applaud 10-20 s, interaction 7-15 s); guards (mProtectItems, 18 in
  Level_02) chase the goat off, kick (600 momentum, 25% ragdoll), angry anim, return home.
- **NPC routes** (GGAIController FollowRoute): mGeneratedPath PathNodes, SRT_LOOP (default and
  deprecated mLoopScriptedRoute) / SRT_STANDARD (ping-pong) / SRT_RANDOM (with the distance/speed
  timeout); speed = mDefaultAnimationInfo.MovementSpeed (else 600, x0.5 when <= 100). Interact
  items at their NearestPathNode (face the actor or InteractionRotation, play the interaction
  anim for InteractionDuration). Routes resume from the nearest node after notice/ragdoll/panic;
  mDelayedRouteStart routes (Kismet) are skipped. 34 walkers on Level_02, 5 on Level_01.
- **Trip** (GGGoat.HitWall + CanCauseGoatToTrip): running into a KActor whose physical material
  has mShouldTripGoat faster than mRagdollCollisionSpeed (850 uu/s) ragdolls the goat (85 of the
  132 materials in Level_01, 1676 props).
- **Surface sounds**: every collider gets a surface from its physical material (PhysMaterialOverride,
  BodySetup, else section material; landscape: dominant paint layer per triangle via
  LandscapeLayerInfoObject.PhysMaterial) -> GGPhysicalMaterialProperty.MaterialType + PhysicalMaterial
  ImpactSound/Threshold/ReFireDelay (138 surfaces, 42 impact cues on Level_01).
  - Footsteps from the animations' AnimNotify_Footstep (Animator.steps), GGPawnSoundGroup tables
    (goat FootstepSounds by MaterialType, NPC HumanNPCFootstepSounds/default; sprinting in water
    uses Goat_Run_Water_Cue). NPCs only within 30 m.
  - GGGoat.HitWall impact sound (mHitWallInterval 1.25 s); KActors play their ImpactSound when their
    velocity jumps by more than ImpactThreshold (once per ImpactReFireDelay).
  - Ragdoll impacts (PlayImpactSoundAtRagdoll): a sudden pelvis velocity change while touching a
    surface plays its ImpactSound (mHitRigidBodyInterval 1.25 s).
  - AnimNotify_Sound cues on animations (e.g. HeadButt_Cue on Ram) play from looping clips and
    one-shots.
  - `SoundLog` counts every requested cue (also in silent test runs); the trip test checks it.
    `GOAT_SOUNDS=1` prints each requested sound.
- **Mod menu** (M, `goat/src/mods.rs`): Steam Workshop tab lists all 379 items (browse pages ->
  keyless ISteamRemoteStorage/GetPublishedFileDetails; cached in mods/cache), previews, and downloads
  the legacy Workshop bundles straight from Steam's CDN, unpacked to mods/workshop/<id>/ (bundle:
  [u32 0][u32 n] then [u32 len][name] and chunks [u32 size][data][u32 more], more = 1 while
  another 2 MB chunk of the same file follows). They're UnrealScript mutators/maps;
  downloaded mutators get a "Run mutator" button (enabled.json key `ws-<id>`) and run in the
  UnrealScript VM (below). Goat Rust tab: mods/rust/<id>/mod.json with
  tweak multipliers (walk/sprint/jump/gravity/air control/headbutt) read through `mods::tweak()`;
  enable/disable saves mods/enabled.json; optional remote index (mods/index_url.txt). Examples:
  Moon Goat, Speedy Goat. `--test mods` (needs internet).
- **Foliage** (InstancedFoliageActor): each InstancedStaticMeshComponent's PerInstanceSMData (bulk
  array at the end of its native data: `[u32 80][u32 n]`, n x FMatrix + 2 UV biases, world space)
  becomes MeshInstances with `matrix` (convert::matrix). 614 on Level_01 (pines, oaks, grass,
  rhubarb); none collide (CollideActors false, like the original).
- **BSP** (PersistentLevel.Model_0, level.rs parse_bsp): bulk Vectors/Points/Nodes (64 B, NumVertices
  at byte 54), Surfs (TTransArray owner + count, 60 B: material, flags, pBase, vNormal, vTextureU/V,
  ...), Verts (24 B). Rendered per material with UVs ((P - base) . U|V) / 128 and one trimesh
  collider: the empty trampoline pool, concrete walls, roofs (3260 polys on Level_01).
- **Trampoline volumes** (physics/volumes.rs): BrushComponent.BrushAggGeom hulls of every Volume
  actor (level.volumes); GGTrampolineVolume family (trampoline, mattress, fan, huge fan, bouncy
  castle) bounce the falling goat, its ragdoll and loose props per GGTrampolineVolume.PostTouch
  (reflect + mBounceFactor capped by mMaxBounceVelocity, PI/20.3 cone) with the class's sound;
  OnBounce restores double jump / wall run. Settings: actor -> content class default (cooked into the
  map as GoatGameContent.Default__X, missed by the archetype merge) -> GGTrampolineVolume defaults.
  `--test trampoline`. `GOAT_SPAWN=x,y,z` (Unreal units, with `--start 99`) spawns anywhere.
- Meshes whose inherited anim set doesn't fit their skeleton use a set from their own package.
- **Level_02** (Goat City Bay) loads too: 19k instances, 242 NPCs, 24 starts (`--map Maps/Level_02.goat`).

## UnrealScript VM (Workshop mutators)

Started 2026-10-18. Runs the game's compiled UnrealScript (Core/Engine/GameFramework/GFxUI/IpDrv/
UDKBase/GoatGame.u and Startup.upk from CookedPC) plus Workshop `Scripts/*.u` packages. Startup.upk
holds the script classes of every DLC (GoatSpace/GoatZombie/GoatHeist/GoatMMO) and the base game's
GoatVanilla mutators, so DLC-dependent Workshop mods link too (all 6346 of its structs decode).

- **Bytecode** (`upk/src/script.rs`): UStruct/UFunction/UState/UClass/UProperty/UEnum exports and
  the UE3 EExprToken stream as an `Expr` tree per statement. `upkdump <pkg> disasm Class.Function`
  (or a whole `Class`, with params/locals) prints it; `upkdump <pkg> scriptcheck` decodes every
  struct: all ~13,800 in the 9 game script packages + 8 Workshop packages decode and their decoded
  in-memory size matches the size the game recorded. Checked by hand against out/scripts
  (GGMutator.ModifyPlayer, GGGoat.IsMutatorValidForGoat/ValidateBoneScaleInfos,
  GGAIController.ArrivedAtPathNode switch, Workshop sources in mods/workshop/*/Src).
  Format facts (v868, 64-bit game):
  - Disk object refs are 4 bytes, memory 8; FNames 8 both. **Jump/skip offsets count memory bytes**,
    so `Stmt.mem` is tracked separately from the disk offset.
  - UObject header before UField data: NetIndex (i32) + empty tagged props ("None"); UClass has
    only the NetIndex. UStruct: Next, SuperField, ScriptText, Children, CppText, Line, TextPos,
    memory size, disk size, bytes. UFunction: iNative u16, OperPrecedence u8, FunctionFlags u32,
    (RepOffset u16 if FUNC_Net), FriendlyName. UState: ProbeMask u32, LabelTableOffset u16,
    StateFlags u32, FuncMap. UProperty: Next, ArrayDim, PropertyFlags u64, Category, ArraySizeEnum,
    (RepOffset), then kind refs.
  - EX_New has 5 operands (outer, name, flags, class, template); EX_StructMember is prop, struct,
    2 bytes, expr; EX_Context/ClassContext are expr, u16 skip, prop ref, u8 size, expr; EX_DynArrayAdd
    has no skip word (AddItem/RemoveItem/InsertItem/Find/Sort do); every native call ends with
    EX_EndFunctionParms; cast tokens are the UE3 ECastToken table (0x36 InterfaceToObject..).
  - A function with optional params starts with one statement per optional param: EX_Nothing (no
    default) or EX_DefaultParmValue (evaluated when the caller passed EX_EmptyParmValue).
- **VM** (`goat/src/uvm/`): `mod.rs` classes/structs/enums loaded lazily through the Registry
  (cross-package imports), objects = class + property map (case-insensitive `Sym` names) + state;
  class default objects merge Default__X tagged props down the class chain (component templates
  are copied per instance). `exec.rs`: statements (Let, Jump/JumpIfNot, Switch/Case, foreach
  Iterator and dynamic-array foreach, Return), expressions (context/class context, virtual/final/
  global/native-index calls, state function lookup, out params written back, optional defaults,
  `&&`/`||` short-circuit, casts incl. metaclass, dynamic array ops, structs, delegates, `new`).
  Object.Class/Name/Outer are built-ins. Unknown natives, missing functions, Accessed None and
  out-of-range array reads are logged once each and return zero; a 2M-statement budget and call
  depth 160 stop runaway events. `natives.rs`: Core operators decoded from their names
  (Add_IntInt, Subtract_PreFloat, AddEqual_FloatFloat...), math/string/vector natives, IsA,
  DynamicLoadObject, GotoState, Spawn (runs PreBeginPlay/PostBeginPlay), Destroy, SetTimer/
  ClearTimer, SetMaterial/SetSkeletalMesh (host events), WorldInfo/Engine getters, iterators
  (AllActors etc. over VM actors). `overrides` replace script functions with Rust (used for
  GGGameViewportClient.GetViewportClient).
- **Host** (`bridge.rs`): builds a script world (GameEngine + GGLocalPlayer, GGGameViewportClient,
  WorldInfo, GGGameInfo, GGPlayerControllerGame + GGPlayerInputGame, DefaultPhysicsVolume, GGGoat
  slot 0), reads `+mAllMutators=(FileName="Pkg.Class")` from each item's GoatMod.ini, then runs the
  game's own flow: GGGameViewportClient.AddMutatorEntry -> GameInfo.AddMutator (DynamicLoadObject,
  Spawn) -> GameInfo.SetPlayerDefaults -> BaseMutator.ModifyPlayer chain. Every frame: mutator
  Tick, GGGameInfo.mMutatorComponents[0] TickMutatorComponent, timers. The script goat's
  mWalkSpeed/mSprintSpeed/mStrafeSpeed/mReverseSpeed/JumpZ/MultiJumpBoost/AirControl and
  MaxMultiJump (relative to its start value) drive the Rust goat through `bridge::movement()`.
  `plugin.rs`: loads packages on a thread when mutators are enabled, forwards the log (every call
  during startup and the first two frames, then warnings) to mods/uvm_log.txt.
- Per frame (`sync.rs`, `plugin.rs`; VM entry points run on a 512 MB-stack thread because a
  mutator chain recurses once per mutator, call depth limit 1000):
  - Goat -> script goat: Location, Velocity, Rotation (yaw), Physics (1 walk, 2 fall, 10 ragdoll,
    15 wall run), mIsRagdoll, mIsSprinting. Script changes to Velocity/Location go back to the
    Rust goat (ragdoll changes from script are only logged).
  - Keys: held keys as DefaultInput.ini names (SpaceBar, LeftShift, LeftMouseButton, one...; test
    runs map ScriptedInput the same way) -> PlayerInput.PressedKeys, GGPlayerInput's
    NotifyKeyStateListeners (KS_Pressed every frame, KS_Down/KS_Up on change) and the deprecated
    mKeyDown/mKeyUp/mKeyPressed delegate lists, like GGPlayerInput.PlayerInput.
  - Game events through the real GGGameInfo.OnX (listeners registered by RegisterDesiredEvents:
    every mOnX list whose OnX the listener implements with a body): OnLanded, OnAirborne/OnGround
    (every frame), OnRagdoll, OnWallRunning, OnWallJump, OnBaa, OnUseAbility (GGAbilityHorn/Bite
    helper objects), OnTrickMade (a GGTrick* object per TrickEvents id), OnPlayerRespawn.
- Config (`config.rs`): `var config` properties from Engine/Config/Base<Name>.ini ->
  GoatGame/Config/Common -> Vanilla Default<Name>.ini (+/./-/! array ops), each class reading its
  own [Package.Class] section; UE3 import text for structs/arrays/objects. Gives PlayerInput its
  Bindings, so `GGPlayerInputGame.IsKeyIsPressed("GBA_Jump", key)` works.
- Host effects: SetMaterial (diffuse only), SetSkeletalMesh on the goat (rig reloaded with the goat
  anims/physics asset, material kept; Ultra Ripped shows goat.Mesh.GoatRipped), AddSkelControl/
  FindSkelControl + BoneScale x ControlStrength applied to the goat's joints after animation
  (Fatter Goat), Broadcast/TeamMessage/ClientMessage shown bottom-left for 6 s, AudioComponent.Play
  cues loaded from the VM's packages into the game's SoundQueue, console `set <Class> <Prop> <Value>`
  (other console commands logged), MatchRefBone/GetBoneName from the real goat skeleton,
  SetRagdoll / SetPhysics(PHYS_RigidBody) on the goat start or end the real ragdoll.
- Script actors in the world (`actors.rs`): Spawn'ed actors with a StaticMeshComponent mesh get a
  Bevy entity (mesh from the VM's packages, else the game's registry = map + GoatGame + Startup,
  shared as the GameRegistry resource); Physics falling/projectile/rigid-body or KActor -> dynamic
  rapier body (convex hull), whose position/velocity is written back into the script actor each
  frame (script SetLocation teleports it); Destroy despawns, bHidden hides. Easter Goat's eggs
  appear around the map. AttachComponentToSocket on the goat's mesh: the socket
  (SkeletalMeshSocket exports under the mesh: bone + relative location/rotation/scale) gets the
  component's static mesh as a child of that bone (Astrogoat's helmet sits on headSocket);
  DetachComponent removes it. CreateAndSetMaterialInstanceConstant + Set*ParameterValue tint the
  props/goat using that instance (first LinearColor parameter). Goat DrawScale scales its mesh.
- Traces: Trace/FastTrace cast rays through rapier during each VM call (`actors::tracer`, a raw
  pointer to Physics that is only valid while the Bevy system waits for the VM thread), returning
  the hit script actor or WorldInfo for level geometry; radius iterators (VisibleCollidingActors...)
  filter VM actors by Location.
- Proxies for game objects (`proxies.rs`): every NPC gets a script actor (class from its actor name,
  GGNpc_12 -> GGNpc) when mutators start, so AllActors(class'GGNpc') finds them; level props get a
  GGKActor proxy when a Trace hits them or a radius iterator (VisibleCollidingActors...) reaches them
  (`vm.overlap`, a rapier ball query). Each frame the NPC/prop state goes into the proxy
  (Location, Velocity, mIsRagdoll, Physics); script SetRagdoll(true) or a Velocity change knocks the
  real NPC, AddImpulse (KActor/GGNpc TakeDamage end in it) pushes the prop/NPC/goat, SetLocation
  teleports a prop, Destroy removes it. Sleeping props the script didn't touch are skipped.
  `goat --test forcepush` (Omniscient Goat 3729292931, R = GBA_Special): its Force Push runs the real
  GGNpc/GGKActor TakeDamage -> 14 NPCs knocked and ~770 props flying. Its 100 km cone wakes ~1400
  props at once: physics then costs ~80 ms/frame until they sleep (the VM side ~2.5 ms).
  Tests can press any key through `Mutators.test_keys`; GOAT_PROFILE=1 prints VM systems over 2 ms.
- Script world extras: PlayerCamera (GGCamera, its Location/Rotation/CameraCache.POV follow the game
  camera), myHUD (GGHUD; debug drawing ignored), BroadcastHandler, GGGameInfo.mMusicManager.
- Run: mod menu (M) -> Workshop -> "Run mutator" on a downloaded mutator; headless
  `goat --uvm [item ids]` prints the full call log, a scripted 8-frame sequence (jump key, landing,
  trick, headbutt, baa, ragdoll) with what the scripts changed, the cost of a frame (0.5 ms with 55
  mutators, 0.1 ms with 3), the script goat before/after and the unsupported list
  (`UVM_DEBUG=Class:prop` prints a class chain and default).
  Tests: `goat --test mutators` (Speedy Goat 3130339802 + Infinite Jumps 3128865778, override with
  GOAT_MUTATORS=id,id) checks the values changed, sprints past the original cap (~3200 uu/s) and
  does 3 air jumps; `goat --test moonjump` (456408519) holds jump: MoonJumpComponent's KeyDown
  listener must see GBA_Jump and its tick must lift the goat (rises ~23-29 m, normal jump 5.6 m).
- Mutators tried: the 50 smallest Workshop mutator items (55 mutators) all start together
  (`goat --uvm` with everything in mods/workshop). Effects verified in game: Speedy/Super Speedy,
  Infinite Jumps, Moon Jump, Ultra Ripped (mesh), Fatter Goat (bone scale). Left: the Space DLC
  spacesuit item (its GoatSpace package isn't installed), Invincible (needs the Zombie DLC goat,
  reads past its empty array like the real game), Darkness Goat (post-process not applied; its
  music cue is never assigned in its own script), Flappy Wings (GGGoat flying isn't ported).
- Game mutators: mod menu (M) -> "Game mutators" tab lists the base game's own mutators from
  Config/Common/DefaultDLC.ini mAllMutators (Tall/Feather/Giant/Space/Devil/Angel Goat, Jet Pack,
  Goat Queen, Ripped/Robot Goat, Blue Streak, Goatborn, Tornado/Repulsive/Classy Goat, Deadgoa7,
  Builder/Shopping Goat, Rocket-Skate, Uncle (grapple), Hitchhiker, Double jump, Goast, Inventory:
  25) with "Run mutator" (enabled.json key `gm-<FileName>`); `goat --uvm game` / `gm-<FileName>`
  runs them headless (0.29 ms/frame for all 25). `goat --test jetpack`: R ragdolls the goat and its
  RB_Thruster lifts it (~24 m in 3 s; JETPACK_JUMP=1 fires it mid-jump).
- Engine physics actors from script (`proxies.rs` force_actors): RB_Thruster based on the goat
  drives the ragdoll's climb at ThrustStrength x 0.2 uu/s (a rapier force can't lift a ragdoll
  lying on the ground; direction approximated as up), RB_RadialForceActor (Tornado Goat) pulls/
  swirls props in ForceRadius (linear falloff). Actors SetBase'd to the goat follow it.
- Also: implemented interfaces (UClass Interfaces list, CLASS_Interface) for InterfaceCast and
  DynamicCast to interfaces; array Sort with the script compare delegate; `var localized` values
  from Localization/INT/<Package>.int (UTF-16); skeletal-mesh attachments to goat sockets in
  reference pose (Jet Pack's pack on JetPackSocket, hair); GGCamera.PostBeginPlay camera modes and
  GGCameraModeOrbital zoom handed to the game camera (Tall Goat's 250-1500 limits); InitRagdoll
  succeeds (Giant Goat keeps its goat ragdolled like the original); vector >>/<< rotator;
  WorldInfo.MyEmitterPool (particles: see below); TraceActors iterator.
  Instances read missing properties through to their class defaults (copy-on-write), and prop
  proxies copy only their components.
- Workshop maps: downloaded map items get "Play map" in the mod menu (GoatMod.ini mAllMaps FileName,
  else any .goat/.udk/.umap in the item); the game restarts on it (`--map <absolute path>`; one level
  per run) and registers the item's own Textures_*.tfc (`upk::texture::add_tfc_dir`). The Game
  mutators tab has Goatville / Goat City Bay buttons to go back. Checked: Box Of Fans (740 static
  meshes), kaboom place (landscape + NPCs + car), goat dream map (own tfc), Demo (BSP only).
  Landscapes without DrawScale3D use Engine's Default__LandscapeProxy (128, 128, 256) (upk level.rs).
  Their own script classes and Kismet run (below).
- State code (`exec::tick_states`): GotoState(state, label='Begin') starts the state's label code;
  `goto 'Label'`, stop, and latent Sleep (resumes when the time runs out) work; the movement waits
  (MoveTo...) are under "NPC behaviour" below; FinishAnim returns at once. `UVM_STATE=Class:State goat --uvm ...` runs one for 3 s
  (GGAIDriverController.DriveToLocation: Sleep(0.5) + goto 'Begin' -> 6 loops).
- Kismet (`kismet.rs`, `touch.rs`): the VM now also links the current map package (`Host::with_map`;
  a host is built for every map even without mutators, GOAT_NO_KISMET=1 turns it off). Map actors,
  their components and sequence objects become VM instances with their saved properties (NPC/prop
  proxies reuse the map's actor objects, `vm.ref_of` maps back to the export). At start the
  SeqEvent_LevelLoaded / GGSeqEvent_StartIntroMovie events fire. SeqEvent_Touch: the goat (Pawn
  ClassProximityTypes, the default) or moving physics props (GGKActor types, instigator = the prop's
  proxy) entering/leaving the originator volume (BrushComponent BrushAggGeom hulls) or Trigger
  cylinder fire Touched/UnTouched. Events honour bEnabled/MaxTriggerCount and set the Instigator
  variable; outputs follow Links (ActivateDelay). Variables (SeqVar_Object/Int/Bool/Float/String/
  Vector/Player) are published into op properties by VariableLinks.PropertyName and written back by
  the Set* actions. In Rust: SeqCond_CompareInt/Float/Bool/Object, Gate, Delay, SetInt/Bool/Float/
  Object/String, AddInt, Toggle (actors' bEnabled + linked events), PlaySound, SetVelocity, Log;
  SeqAct_Interp plays matinee (below); SetCameraTarget and GFxAction_OpenMovie complete at once. Other ops run
  their script `Activated` (GoatGame's GGSeqAct_SetRagdoll, GiveScore, UnlockAchievement...) and
  then all outputs. GGGameInfo.OnKismetActivated is overridden: a GiveScore whose Targets include
  the goat becomes a Kismet trick in the combo (TrickId::Kismet; font tags/brackets stripped).
  Goatville: 635 sequence objects, 57 touch volumes. `goat --test kismet` puts the goat in a volume
  whose chain reaches a GiveScore without conditions: treadmill -> ragdoll + "TREADMILL1" +100 +
  launch. Headless: `UVM_MAP=Maps/Level_01.goat goat --uvm x`; UVM_KISMET_TRACE=1 prints every
  activation, UVM_KISMET_DUMP=<object> its properties.
  Cost: packages, mutator start-up and Kismet loading all run on the loader thread (~0.3 s + 46 ms
  start); with no mutators/timers/state code/pending Kismet the per-frame VM call is skipped
  (`Host::idle`); touch volumes build in ~2 ms. Map actors from the map package never get a second
  entity from script_actors (`vm.ref_of`). Diagnostics: GOAT_NO_KISMET=1, GOAT_NO_TOUCH=1,
  GOAT_UVM_OFF=proxies,actors.
  Note: `--test flip` is timing-sensitive (landing a scripted backflip): it fails now and then
  with or without Kismet, more often when the flip starts later than t = 1 s (checked with Kismet
  off). Its phase-6 end was recomputed every frame (it only ended through the 60 s harness); it now
  ends 0.5 s after the spin and aborts if the spin jump stalls.
- Workshop maps' own script classes (`kismet.rs` map_scripts_*): actors placed in the map whose
  class the map package defines get PreBeginPlay/PostBeginPlay at start and Tick every frame (the
  host stops idling while one ticks); Touch/UnTouch fire when the goat enters/leaves their collision
  (`touch.rs`), and ones with physics bodies get prop proxies so Location/Velocity sync both ways.
  Checked headless on Flapmaster and the Rocket League map.
- Smaller gaps closed: the goat's collision capsule follows its script DrawScale/DrawScale3D
  (`proxies.rs` -> `bridge::goat_scale`, used by goat.rs; `goat --test resize` with Resizable Goat
  246864068: H doubles the goat, capsule x2, still grounded); MaterialInstanceConstant texture
  parameters (SetTextureParameterValue) swap the texture, not only the tint; skeletal meshes
  attached to the goat that share >= 80 % of its bones (hair, wings) copy its pose every frame
  (`actors::follow_goat_pose`), others hang off the socket in reference pose.
- Particles (`upk/src/particles.rs`, `goat/src/particles.rs`, `uvm/fx.rs`): ParticleSystem ->
  sprite emitters -> LOD 0 modules are read with their archetype chain (cooked modules only store
  what differs from Engine.u's Default__ParticleModuleX, and default objects' archetypes are implied
  by class/super class, so Engine.u is now the last package of the game registry too). Values come
  from the baked RawDistribution LookupTable ([min, max] header, then ChunkSize floats per sample;
  Op 2 = random between min/max), else the Distribution object (Constant/Uniform/ConstantCurve/
  ParticleParameter). Supported: Required (duration, loops, delay, local space, screen alignment,
  SubImages + interpolation), Spawn (rate, rate scale, bursts), Lifetime, Size, SizeMultiplyLife,
  SizeScale, Velocity (+radial), VelocityOverLifetime, Acceleration(+OverLifetime), Color,
  ColorOverLife, ColorScaleOverLife, Rotation, RotationRate, Location, LocationPrimitiveSphere/
  Cylinder, SubUV. Not drawn: mesh/beam/ribbon type data, Orbit, Collision. CPU simulation, one
  camera-facing quad mesh per emitter rebuilt every frame (meshes are only ever replaced whole:
  an empty mesh asset froze Bevy's slab allocator). Material: the texture behind EmissiveColor
  (unlit particle materials), alpha from the texture behind Opacity, blend from the base
  Material (additive -> AlphaMode::Add); HDR vertex colours are rolled off (24x orange stays
  orange). Sources: the map's Emitter actors (Goatville: 109, 95 auto-active; Kismet Toggle turns
  them on/off) and script components: EmitterPool.SpawnEmitter (world spot or actor),
  SpawnEmitterMeshAttachment (goat bone/socket, sockets of meshes attached to the goat, script
  actors), Actor.AttachComponent / SkeletalMeshComponent.AttachComponent(ToSocket) with a
  ParticleSystemComponent, ActivateSystem/DeactivateSystem/SetActive/SetHidden/KillParticlesForced/
  SetTemplate. One-shots despawn when done. `upkdump <pkg> particles <filter>` prints parsed
  systems. `goat --test fx` spawns Jet Pack thrust, fire and fireworks next to the goat (each must
  have live particles); `--test jetpack` now also checks the two thrust plumes play. GOAT_NO_FX=1
  skips map emitters.
- NPC behaviour through the VM (`uvm/npcs.rs`, opt-in: GOAT_NPC_VM=1, forced by `--test npcvm`):
  every NPC's proxy pawn is possessed by its real ControllerClass (GGAIController, Agressive,
  GangMember, PassiveGoat or a mod's) with PreBeginPlay/PostBeginPlay/Possess. Its Tick, timers and
  state code run in the VM; the game feeds SeePlayer(goat) every 0.5 s when in sight (clear line,
  60 m; the script applies PawnInRange/GeometryBetween), OnRagdoll(pawn, on/off) to every
  controller, and the pawn's Rotation. Latent MoveTo/MoveToward/MoveToDirectNonPathPos now suspend
  state code until the pawn arrives (flat distance <= max(offset, 40 uu), 15 s timeout);
  FinishRotation waits 0.3 s, WaitForLanding until not falling, StopLatentExecution clears the
  wait (`exec::Latent`). GGNpc.SetAnimationInfoStruct is native-overridden (picks the animation,
  sets GroundSpeed/bIsWalking like the script, returns the clip length from the NPC rigs). npc.rs
  then walks/turns/animates a scripted NPC via `Npc.script` (ScriptDrive); knock-downs, ragdolls,
  scoring stay in Rust. GGAIControllerBase.ShouldCheckVisibility is native (interval ran out), and
  WorldInfo.TimeSeconds/RealTimeSeconds/DeltaSeconds now advance every VM tick (they were stuck at
  0 for all scripts). `goat --test npcvm`: 92 NPCs possessed, all 5 route NPCs walk their routes
  through FollowRoute state code, an aggressive NPC enters ChaseGoat, and the idle NPC the goat is
  put in front of sees it and reacts (gestures). Cost: < 2 ms per frame for 92 controllers, 26 ms
  once when possessing. The default stays the Rust AI (npc.rs) until the VM path matches it.
- Matinee (`upk/src/matinee.rs`, `uvm/matinee.rs`): SeqAct_Interp reads its InterpData (the "Data"
  variable link; groups -> InterpTrackMove PosTrack/EulerTrack, or the InterpTrackMoveAxis
  SubTracks; InterpTrackEvent keys) and binds each group to the actors on the variable link named
  like the group. Inputs Play/Reverse/Stop/Pause/Change Dir, PlayRate, bLooping,
  bRewindIfAlreadyPlaying; outputs Completed (end) / Reversed (start) and the event-track outputs
  named by EventName. Curves: linear, constant, Hermite with the key tangents (CurveAuto/User/
  Break/AutoClamped). IMF_RelativeToInitial: the track is in the actor's frame at its first matinee
  (world = track * initial); EulerTrack is (roll, pitch, yaw) degrees. The actors' VM Location/
  Rotation are written and HostEvent::Mover moves every entity named like the actor (keeping each
  part's offset) and its static trimesh colliders (world.rs records *InterpActor* colliders in
  `Physics::movers`). Animation, director, fade and property tracks are not played. Goatville: 34
  matinees, 10 with keyed movers (looping cars, a blimp/plane path, rotating props, a UFO with
  StartBeam/EndBeam events). `upkdump <map> matinee <filter>` prints parsed tracks; `goat --test
  matinee` plays the farthest-travelling one through the Kismet queue and checks its entity and
  collider move (GGInterpActor_7: 132 m in 3 s).
- Not yet: replication, post-process (logged), Apex destructibles, matinee animation/camera
  tracks, the goat riding moving platforms, mesh/beam/ribbon particles, static mesh sockets
  (effects use the mesh origin), HearNoise (baa) for VM NPCs.
- Next: compare VM NPCs with npc.rs reactions (panic on headbutt, applaud on tricks) and switch
  the default once they match; carry the goat on matinee movers; mesh particle emitters.

## Destructibles, explosives, progress, music, front end (2026-10-18)

- **APEX destructibles** (`upk/src/apex.rs`, `goat/src/destruct.rs`): ApexDestructibleAsset's native
  data is `[u32 1][u32 n][name][u32 len]` + a binary NvParameterized (APB) file cooked on x86-64
  MSVC: big-endian header (magic 5A5B5C5D, version 1.3, dataOffset, relocOffset), an object table
  `{ptr obj, ptr class, ptr name, ptr file}`, ObjHeaders `{u32 rel, ptr class, ptr name, bool
  included, u32 version, u32 checksum size, ptr checksum}` and each object as an NvParameters
  (0x48 bytes) + ParametersStruct memory image (pointers = file offsets; DynamicArray1D = ptr,
  bool, i32 elemSize, i32 size, 24 bytes). Layouts from reference/PhysX-3.4/APEX_1.4 autogen
  headers: DestructibleAssetParameters 0.12 (chunks at +48, destructibleParameters at +120,
  renderMeshAsset at +312), RenderMeshAssetParameters 0.0, SubmeshParameters 0.0,
  VertexBufferParameters 0.1, VertexFormatParameters 0.0 (BufferFormat 40 bytes, semantic names),
  BufferF32x3/x2. All 17 assets on Level_01 parse (`cargo run -p upk --example apex_assets`).
  Intact = chunk 0's part, static collision = the leaf chunks' hulls; fracture = leaf chunks as
  dynamic bodies (hull per chunk, mesh offset by its centroid). Rules from
  GGApexDestructibleActor: GGDamageTypeAbility damage >= DamageThreshold (headbutt mDamage 200),
  or GGDamageTypeCollision momentum >= mTriggerFractureMomentum 2000 (goat: Mass 45 x speed, only
  above mSprintSpeed 1200; props: mass x speed). GGKActors with mApexActor (vases, pots) shut down
  and spawn the archetype broken (ConvertToApex). Fracture sound (phys property mFractureSound),
  GGTrickFracturingApex "Broke <name>" with the DefaultPhysMaterial score. 154 placed + 124
  breakable KActors on Level_01. `goat --test apex` (headbutts the greenhouse glass: 4 panes, 38
  debris). Physics gained `hits` (TakeDamage calls: collider, damage, momentum, kind, by goat),
  cleared in Last; headbutts push them.
- **Explosives** (`goat/src/explosive.rs`): GGExplosiveActorContent/Wreckable props (now dynamic
  KActors; 137 on Level_01). ShouldExplode: damage >= mExplosionTriggerDamage 30, collision
  momentum >= 1800 (goat above 1.2 x sprint speed; props: a hard stop from >= 1800 uu/s). Explode =
  HurtRadius with full damage: the phys property's mExplosionDamage / Radius / mExplosiveMomentum
  (50000-200000 kg uu/s): NPCs knocked (momentum / 80 kg), the goat ragdolled and launched
  (/ 45 kg, capped 30 m/s), props pushed, other explosives chain (Hit kind Explosion); sound,
  particle effect (mExplosionEffectTemplate), GGTrickExplosion 10 + GGTrickHurtByExplosion per
  scored victim. Wreckables (cars) swap to mWreckageMesh/mWreckageMaterials (built at load) and get
  mExplosiveMomentumOnSelf. `goat --test explode`. Physics `world_events` carry Exploded/Fractured
  (map export) to the VM: GGGameInfo.OnExplosion / OnFractured + Kismet GGSeqEvent_Explosion on
  the actor (quests like BlowUpGasStation, the Demo Goat achievement).
- **Level fixes on the way**: mesh instances keep the StaticMesh object index and resolve it
  through the registry (meshes imported from Startup.upk were dropped: +235 instances, e.g. 103
  gas cans); world.rs keys meshes/materials by ObjRef. Translucent materials draw with a glass
  alpha (0.3). Particle materials follow MaterialExpressionParticleSubUV/FlipBookSample textures
  (explosion fire/smoke were white quads); distortion-only emitters (heat haze) are skipped.
- **Quests, achievements, collectibles** (`goat/src/uvm/progress.rs`, `collectibles.rs`): at host
  start GGLocalPlayer.mSaveProgression = a GGProgression loaded from save/progression.json (every
  plain property, JSON), GGEngine.SaveProgression writes it back, the controller's
  GGAchievementHandler is registered (GGGameInfo.Login) and GGGameInfo.InitGameComponents creates
  the real GGQuestManager (37 quests on Goatville; WorldInfo.GetMapName now returns the map
  package name). QuestTick runs every frame (the host no longer idles once quests exist, ~3-6 ms
  VM frame). UnlockAchievement is recorded in save/achievements.json and shown as a message;
  GGMutator.IsUnlocked reads that file (GOAT_UNLOCK_ALL=1 unlocks every mutator). Cooked enum
  values named "None" load as 255 (UE3 INDEX_NONE), not 0 (that unlocked Boulder of Death).
  GGCollectibleActors (30 trophies) run their PostBeginPlay before the quests count them, are drawn
  with GGPickUpActor's wobble and picked up within sqrt(mPickUpRadiusSq) of the goat's feet via the
  script's PickedUp (save, OnCollectibleFound, achievements). KActor.GetKActorPhysMaterial native.
  `goat --test quests` (Baa x3 completes QI_Baa, trophy pickup counts 1/30 and is saved; tests use
  out/test_save unless GOAT_SAVE_DIR is set).
- **Music and ambience** (`goat/src/ambient.rs`): the VM's GGMusicManager decides (track queue,
  PlayCustomTrack, Kismet ChangeMusicTrack, mutes); its top track (mTracks[0].Cue unless muted)
  plays as a looping stream with its 0.8/s cross-fade at the Options music volume. Goatville has no
  map music (like the original); Deadgoa7's party plays deadmau5 (`goat --test music`). Placed
  AmbientSoundSimple/AmbientSound(Movable) actors (46 on Goatville) loop with UE3 linear falloff
  between RadiusMin/RadiusMax (SoundNodeAmbient slots / SoundNodeAttenuation; upk sound.rs
  cue_attenuation, cue_waves follows SoundSlots). GOAT_NO_AMBIENT=1 turns them off.
- **Front end** (`--test mainmenu`): a plain launch boots into the original main menu
  (GGHUD.StartMainMenu, opened on the game thread with the window size; the level is paused under
  it and the debug overlay hidden); `--map`, `--no-menu` and tests skip it. Play -> Custom Game
  (GGEngine.GetCSSDLCManager / DoesPackageFileExist natives; the ini parser keeps repeated plain
  `Key=` lines as array entries, so both maps list) -> Play! with the current map and no mutators
  runs StartClean (menu closes, HUD starts); another map or mutators restarts the process on that
  map with GOAT_MUTATORS=gm-<class> and --no-menu (`ConsoleCommand("start ...")` ->
  MenuAction::Start). Exit quits. Options -> Apply saves save/settings.json (profile settings,
  FOV/named engine settings, mouse sensitivity/inversion), loaded at start. Fixes: menus advance at
  most ~120 times a second and new movies render every frame for their first 120 frames; the main
  menu's news text area is re-placed at (1385, 200) while the menu is new (when its widget init
  landed after SubPostWidgetInit it sat at (0, 0) over the buttons and took their clicks).
  `goat --uvm-menu-click main,Play` (headless; UVM_MAP / UVM_START reproduce the in-game host).

- **Volumes, fire, lights, Steam achievements** (later the same night):
  - `physics/volumes.rs` handles the rest of the level's volumes: BlockingVolume -> static
    colliders; PhysicsVolume ZoneVelocity carries the goat (currents/updrafts); GravityVolume
    GravityZ replaces gravity for the goat and awake props (zero-g rooms; GGDynamicGravityVolume
    honours bEnabled); LadderVolume = PHYS_Ladder (Pawn.LadderSpeed 200: push into WallDir to grab,
    forward/back climbs, no gravity, jump lets go; WallRunUp legs as the climb animation - the goat
    has none). `Goat::env` (GoatEnv) carries them into goat_move. 87 such volumes on Goatville.
    `goat --test ladder`.
  - `fire.rs`: explosion victims burn (GGPawn.ShouldBeOnFire) for 8-10 s with Effects_Fire_01 and
    the burn loop; burning pawns ignite pawns within 0.9 m; burning NPCs panic. Explosions flash an
    orange point light for 0.4 s (UDKExplosionLight TimeShift).
  - `progress.rs` reads the player's Steam achievements read-only from
    <Steam>/appcache/stats/UserGameStatsSchema_265930.bin + the newest UserGameStats_*_265930.bin
    (binary KeyValues; ACHIEVEMENTS stats are bitfields, names = the game's EAchievementsID names)
    and treats them as unlocked (mutators unlock like the player's own game; test runs and
    GOAT_NO_STEAM_ACHIEVEMENTS skip it). `goat --achievements` lists them.

- **Look** (`look.rs`): WorldInfo.DefaultPostProcessSettings' ColorGrading_LookupTable
  (ColorLookup.Textures.level_01, 256x16) is fitted to Bevy ColorGrading (contrast 1.20 from the
  grey ramp, saturation 1.05 from the chroma ratio); the HeightFog with the highest ceiling becomes
  exponential distance fog (Density per uu / 0.02 -> 0.0015/m on Goatville, LightColor).
  Bloom stays off (bEnableBloom false); DOF and image grain are not done. GOAT_NO_LOOK=1 compares.
- **Explosion decals**: the phys property's mExplosionDecalMat (Decals_Dirt_02, BLEND_Modulate:
  tint 0.5/0.4/0.4 x texture, texture alpha) as a Bevy ClusteredDecal (200 x 200 uu, 0.3 m deep)
  on flat ground just under the blast, removed after 62 s; GOAT_NO_DECALS=1 turns them off.
  Particle sprites no longer cast shadows (NotShadowCaster): smoke and fire made dark blobs on walls.

- **Script hooks added later**: GoatGame's GGTriggerVolume* actors in the map get their script
  Touch/Untouch when the goat enters/leaves them (touch.rs: music volumes, race, ritual, time
  trial, CTF, hockey, force-ragdoll volumes); WorldInfo.GetMapInfo returns the map's GGMapInfo
  (Goat City Bay: GGMapMusicInfo with Background / HighCombo tracks); banked combos call
  GGGameInfo.OnEndCombo(combo) and the quest manager's OnNewScore(total) (score/combo quests).

- **Music manager start**: GGMusicManager is created in `progress_start` (InitMusic, global
  "musicmanager") and ticked every frame; map GG*/MapInfo objects under PersistentLevel load as
  instances with their saved properties (obj_for), so GetMapInfo's GGMapMusicInfo has its tracks:
  Goat City Bay plays Temp_Music.Sound.BGM_temp. mHighComboTrack / mHighComboEndedTrack are not
  read by any script (nothing to port).
- **Collision chains** (`chain.rs`, GGCollidableActorInterface): the goat is chain 0; props it
  headbutts/bites (hits by goat, the grabbed item) or bumps get 1, what they hit gets nr + 1
  (UpdateCollisionChainWithGoat both ways). Physics records RigidBodyCollision-style contacts after
  each step (`Physics::contacts`: awake scored props whose contact impulse / mass >= 50 uu/s,
  ScriptRigidBodyCollisionThreshold). Per Collided (mMinTimeBetweenCollisions 1 s, not while
  bitten): GGTrickCollision = the chained prop's score (unnamed, no multiplier) and
  GGTrickCollisionSpecial "Skillshot" = travel since OnWakeRBPhysics / 100 when >= 2000 uu and the
  other side is a prop/NPC. Chains reset on sleep and on combo end (`Score::combos_ended`).
  NPCs: inserting `npc::Knocked` puts the NPC in the goat's chain (observer); ragdolled chained
  NPCs whose bodies hit something hard score their GetScore (mMinTimeBetweenCollisions 2 s) and
  pass the chain on (NPC ragdoll bodies count as the NPC).
  `--test headbutt` now also requires a collision-chain trick.
- **GGTrickBounce** (volumes.rs + score::bounce): mJumpPairInfos from Default__GGTrickBounce -
  trampoline -> other trampoline 500, bouncy volume -> other one 250 ("jump from X to Y", adds
  multiplier), anything -> trampoline 50 / bouncy volume 25 ("jump on X"); a bounce counts as
  landing on the volume (OnBounce -> OnGround), standing in one counts too (TouchingGroundVolume).
  Trampoline = class contains "VolumeTrampoline" (GGTrampolineVolumeTrampolineContent).
  `--test trampoline` requires the trick.
- **The manual** (`manual.rs`): GGPlayerInputGame.CheckManualActivation (back < -0.8, then
  forward > 0.8 within mManualKeyInterval 0.08 s) -> AttemptManual (on the ground, else on the next
  landing). While on the ground: UpdateManual's unstable meter (speed += (50 - value) x -7 x dt,
  W/S add aBaseY x mManualPlayerForce 20 per input tick, scaled to 60 Hz), 0.7 s stuck at an end
  ragdolls; GGTrickManual adds 10 per 0.05 s and keeps the combo alive; movement = facing x 30
  uu/s^2 with the speed steered onto the facing (goat turns toward the camera); the "Manual" clip
  (manualBlendNode). Landing (not in ragdoll) ends it: "manual" + the accumulated score. A ragdoll
  during a manual breaks the combo (Score::break_combo: combo lost, 5 s combo breaker during
  which no trick counts, HUD BreakCombo). The HUD's own manual widget (StartManual/StopManual,
  mManualWidget.UpdatePluppPosition, UpdateManualScore) is fed via HudEvent. `--test manual`.
- **Vehicles** (`vehicle.rs`): placed GGBicycle (7 on Goatville, 12 on Goat City Bay) and
  GGLongboard (Goat City Bay) as rapier DynamicRayCastVehicleController vehicles. Skeletal mesh +
  rig from the SVehicleMesh; chassis colliders from the physics asset minus wheel bodies, mass at
  axle height (PhysX keeps wheel shapes in the chassis; a top-heavy frame tipped over); wheels at
  BoneName + BoneOffset, WheelRadius (bike 35, board 10 uu), SuspensionTravel 5 uu (stiff
  springs), bPoweredWheel per wheel (bike: both tires, support wheels softer/unpowered).
  `vehicle::Def` holds each class's defaults (TorqueVSpeedCurve and MaxSteerAngleCurve from
  their SimulationObject, mJumpForceSize as a velocity change, roll thresholds, socket,
  mDriverPosOffset / longboard (5,0,45), rider animation). R (GBA_Special -> Use;
  FindVehicleToDrive within VehicleCheckRadius 200, facing preferred) gets on/off (R only
  respawns when no vehicle is near). Riding: W pedals (force follows the torque curve), S
  brakes then reverses, bike steers toward the camera (mSteerWithCamera), the board with A/D;
  Space jumps (JumpWithBike), A/D in the air spin the bike (JumpFlipLeft/Right); the balance
  game (Fall_L/Fall_R blend weight, fall/balance/penalty speeds, fall at 0.86) poses the bike
  and kicks the goat off when it falls; crashes >= 600 uu/s, tipping over (roll > 1.4 bike;
  board 1.25 below 500 uu/s) and any ragdoll throw the rider off. The goat sits at
  SaddleSocket/BoardSocket (leaning with the saddle bone) playing "Biker" (rate from
  AdaptBikerSpeed) / Idle; wheels spin and steer on their bones. Physics::vehicles are updated
  before every step. Bikes are score actors ("Bicycle", 10). Rig materials now use a constant
  DiffuseColor when there is no texture (the bikes were white). `--test bike`, `--test longboard`.
  Sounds: the ridden vehicle loops its mTireSoundList cue by SetTireSound's rules (bike:
  DrivenVehicles Bicycle_Pedal/NoPedal/Brake; board: Longboard Speed_Up/Speed_Down/Driving),
  mJumpSound on landing a jump, CollisionSound on a crash. Not done: NPC bikers. Jumps fire once
  per press (KeyState KS_Down), not every frame Space is held.
- **Shopping cart** (`vehicle::SHOPPING_CART`, from Default__GGShoppingCart in GoatGame.u: mesh
  shoppingGoat.Mesh.ShoppingGoat_skele_01 + its PhysicsAsset, 4 powered wheels, curves from its
  SimulationObject). The Shopping Goat mutator's GGMutatorComponentShoppingGoat spawns
  GGShoppingCart in the VM and calls TryToDrive: the `ggsvehicle.trytodrive` override
  (natives_engine::register_overrides; it must exist before mutators start) queues a ScriptDrive;
  `script_vehicles` waits until the goat is grounded, builds the cart at the goat on the ground
  under it (spawn_vehicle, shared with the placed bikes) and `ride` enters it (pending_mount).
  mUsedByMutator -> Bike.locked (DriverLeave refuses), bDriverIsVisible false -> the goat is
  hidden (driver_visibility), mSteerWithCamera false (A/D). SetTireSound: below 400 uu/s
  SpeedUpDown, else DriveLoop; mJumpSound on the jump, mLandFromJumpSound on landing,
  CollisionSound past mCollisionVelocityThreshold 800 (1 s apart). CART_PHYSX_MASS (300) is a
  guess for a ~1 m hop. `goat --test cart` (GOAT_MUTATORS forced): in, drive 3 s, R can't leave,
  jump.
- **Skinned materials** (rig::load_material): `material::diffuse_graph` parses DiffuseColor into
  a Node tree (texture samples with tiling, channel masks, Multiply/Add/OneMinus/Lerp,
  constants, scalar/vector parameters with instance overrides); anything else -> None and the
  old first-texture path. Graphs beyond one plain texture are baked on the CPU in linear space
  (rig::bake). BLEND_Masked materials take alpha from MatInfo.mask (OpacityMask's texture,
  channel and OneMinus inversion) and render AlphaMode::Mask, two-sided. Static meshes
  (world.rs) don't use the baker yet.
  The test re-heads the bike toward open ground and lifts it 0.3 m while doing so (turned about its
  origin, the longboard deck swung into a kerb trimesh and was pushed under it).
- **Promo** (`tools/progress_board.py` -> out/promo/progress.html, rendered by headless Chrome;
  the Discord pack is out/promo/discord with POST.txt). The clip was recorded by gdigrab of the
  window's client-area desktop region while a test ran with GOAT_CLEAN=1 (hides the debug HUD).
- **Water** (`water.rs`, volumes Kind::Water): GGWaterVolume / GGCoinVolume touch (the pawn's
  cylinder, a COLLISION_HEIGHT margin) = SetIsInWater: fires go out; NPCs drown (ragdoll +
  Goat_Effects.Effects_Bubbles_01 at the jaw, kept down while in the water); the goat plays
  Effect_Goat_Swimming_loop_cue, a ragdolled goat is pushed by its input (GiveWaterBoost:
  mWaterAccelRate 13 x mass on the pelvis), Space in water = DoRagdollJump, on land Space stands
  the goat up (GGPlayerInputGame.Jump), standing up in water only near the shore (ground within
  mIsInAirThreshold 50 uu). Footsteps: GetMaterialBelowFeet's Water/ShallowWater when a water
  surface is under the goat. Goat City Bay's sea floats things with its GravityVolumes (GravityZ
  +250/+1000, Priority, RigidBodyDamping 2/3): PhysicsVolumes now keep Priority (highest wins)
  and RigidBodyDamping (drag -v x damping x mass on bodies inside), and the volume force is
  reset when a body leaves (rapier keeps user forces; zero-g props kept floating before).
  Static meshes collide only with sections whose EnableCollision is set (the sea plane had a
  trimesh that trapped floating bodies under it); meshes with no such section use their
  RB_BodySetup simple collision. `--test water` (Goat City Bay).
- **Pitching machine and baseballs** (`pitch.rs`): the VM's GGPitchingMachineAbstract.Pitch is
  overridden to fire a Rust ball (GGKActorBaseBallContent's mesh, ball collider, 7500 impulse /
  3 kg PhysX mass = 2500 uu/s, LifeSpan 120 s) from PitchingSocket; Kismet (TriggerVolume_30
  Touch -> GGSeqAct_StartPitchingMachine) starts its 2 s timer. Touching the machine straps it to
  the goat's jetPackSocket (rot(0,16384,-1000)); R then pitches forward; goat balls join its
  collision chain (SetCollisionChainGoatNr), ignore the goat's capsule (GROUP_GOAT) and are lit
  when the goat holds a PhysMat_Grill item. GGTrickBaseball in chain.rs: "[Flaming] Beanball at
  <NPC>" = 100 + the NPC's score; NPCs applaud bounce and baseball tricks
  (mApplaudGoatTricks). `--test baseball`.
- **Races** (`uvm/race.rs`, `race.rs`): GGTriggerVolumeRace's script Touch opens its popup;
  agreeing runs ConsoleCommand("start <map>?game=GoatGame.GGGameInfoRace?SkipIntro?raceID=<id>")
  -> the process restarts with the URL options in GOAT_GAME_OPTIONS (case kept). The host spawns
  the URL's GameInfo class, runs InitGame(options) (raceID, CountDownTimer), the checkpoints'
  PostBeginPlay (mesh holders, CheckpointSpawned ordering) and PostLogin (state RaceCountdown,
  SpawnGoatRider, the BikeRace bike), with one logged-in controller in the cross-map data;
  race_tick does GGGameInfoRace.Tick's race part (FinalizeCheckpoints, the HUD timer via
  FormatTime(int)). The goat spawns at the race's GGPlayerStartRace (level.race_starts, not in
  player_starts any more), is held still in RaceCountdown, checkpoint meshes (mMeshToUse, hidden
  outside races) show while player 0's holder is visible and not passed; BikeRace moves a map
  bike 150 uu in front of the goat and, when riding, script Touch passes the bike's VM object
  (Controller = the player's) as the pawn. GGRaceCheckpoint* actors get script Touch, which now
  overlaps the goat's capsule (25 x 30 uu) and the segment travelled since last frame instead of
  testing its centre point (the gates are thin slabs). The script does the rest: order,
  OnActivated/OnDeactivated, "GOAT!" countdown juice, the timer widget, EndRace's
  MID_RACE_ENDED popup and restart. `--test race` (RegularRace, 20 gates), `--test bikerace`
  (14 gates), `--test racepopup`.
- **Baked sun shadows** (`lightmap.rs`, patches/bevy_pbr): every component's first ShadowMap2D
  (static meshes / foliage: native LOD data's ShadowMaps; BSP: FModelElement ShadowMaps;
  landscape: the ShadowMaps property) rides along in `LightMap2D.shadow`. The bake samples it
  (G8; bIsShadowFactorTexture false = distance field, edge at 0.5, smoothstep 0.45..0.55) for
  every lightmap texel through UV = (atlas UV - lm bias) / lm scale * sm scale + sm bias, grows
  it into the gutters with the colour, and stores it half resolution in each page's mip 1
  (grey RGB9E5, +25% lightmap memory). The bevy_pbr patch adds `lightmap_shadow()` (mip 1, 1.0
  for single-mip images) and PbrInput.lightmap_shadow; directional light 0 on lightmapped meshes
  uses min(realtime cascade, baked) - static shadows at any distance, dynamic casters (goat,
  NPCs, props) still shadow the ground in realtime. Translucent sections (glass, water) no longer
  cast realtime shadows (they have none baked). GOAT_NO_BAKED_SHADOWS=1 / GOAT_SHADOWS=0 compare.
  Bake 1.8 -> 2.6 s.
- **Main menu leftovers**: Credits scroll the original CreditsText clip (slowly, from below the
  screen); Workshop -> Downloaded lists the items downloaded with the mod menu (GGWorkshop
  mUGCDetails filled from mods/workshop/*/workshop.json; mWorkshop created when the instanced
  subobject is missing); DLC, Leaderboards (mLeaderboardURL), Achievements and Workshop
  Browse/guide (OpenWebURL / ShowAchievementsUI in the original) open the Steam pages in the
  default browser (MenuAction::OpenUrl, never in tests). Options -> Apply's `setres WxH[f|w]` now
  resizes the window / toggles borderless fullscreen (settings::apply_setres). Special Kismet
  scores no longer also print in the debug overlay (the HUD's juice shows them).
  `goat --uvm-menu-click` steps accept `wait:<seconds>`.
- **Manual -> scripts**: GameEvent::Manual -> GGGameInfo.OnManual(goat, doing, success).
- **APEX notes**: GGApexDestructibleActor.Fracture applies mDamageThreshold x 10^7 damage, so the
  original always shatters completely (no partial fractures to port); the assets' flags have
  DEBRIS_TIMEOUT off (debris stays). Race checkpoints' mMeshToUse meshes are hidden
  (GGRaceCheckpointAbstract.PostBeginPlay).
- **Load time**: world volumes looked up each volume's Default__ object by scanning every export
  path (3.5 s); now a map built once (23 ms). Lightmap atlases bake from a work queue with an sRGB
  LUT (2.3 -> 1.8 s). Level spawn 6.8 s -> 3.4 s.

## Conventions worth remembering

- UE->Bevy: (x, y, z) -> (x, z, y) * 0.02 (m per uu). Quaternions: (-x, -z, -y, w).
- Ref-skeleton bone rotations are used as stored; animation keys must be conjugated for every
  bone except the root (tested with `upkdump GoatGame.u skelcheck goat`).
- RotOrigin of the goat mesh is yaw -16384; component Translation (0,0,8); pawn +X = Bevy -Z.
- Physics-asset element matrices store each FPlane row W-first (W, X, Y, Z).
- Constraint frames (Pos1/Pos2) are already in metres.
- rapier applies impulses at the next step: cap impulses by mass rather than clamping velocity.

- **Inventory** (uvm/inventory.rs): the Inventory mutator's scripts keep the slots and HUD list;
  sync_in gives the VM goat mGrabbedItem (the licked prop's proxy, with uvmActorName = its
  in-game name for GGKActorBase.GetActorName), 1 -> AddGrabbedItemToInventory, I ->
  ToggleInventory, open list -> HUD FilterButtonInput per key + CameraZoom for the wheel;
  INVENTORY_OPEN freezes goat input/camera. Prop proxies honour script Physics: PHYS_None ->
  kinematic, colliders off, hidden, lick released; PHYS_RigidBody back. Natives:
  Actor.GetComponentsBoundingBox (vm.bounds closure, collider AABBs), the goat's headSocket.
  gfx: `<Controller:GBA_X/>` tags -> bound keys (controller_tags); widgets attached before a
  movie's first frame lost their CLIK init callbacks -> replay_widget_init at frame 5 when no
  widget announced itself. `--test inventory`.
- **Goat rider** (rider.rs): SpawnGoatRider/RemoveGoatRider overrides -> a random NPC's rig
  (RiderLook: data, materials, component Translation) under the goat's RideSocket bone,
  ChairIdle at 0.1. GOAT_RIDER=1 spawns one for checking.
- **Race leaderboard** (uvm/leaderboard.rs): save/race_best.json per ELeaderBoardID;
  OnlineStatsRead.GetIntStatValueForPlayer returns it, GGUIViewPopupRaceEnded.SetBody override
  writes "time" + "1. <Steam persona from config/loginusers.vdf> - best" and saves a new best.
- **News**: InternetLink.Resolve -> ResolveFailed 0.5 s of menu time later (menus.rs tick).
- **Post** (post.rs + post.wgsl): UE3 DOF (CalcUnfocusedPercent) + grain as a fullscreen pass
  reading the camera depth texture (Camera3d depth_texture_usages + TEXTURE_BINDING, no
  prepass). look.rs reads bEnableDOF/DOF_*/Scene_ImageGrainScale (UE3 struct defaults for the
  rest). GOAT_NO_POST, GOAT_DOF_KERNEL.
- **Materials**: matbake.rs bakes DiffuseColor graphs (material::diffuse_graph) for skinned
  meshes and level materials (world.rs bake_materials, parallel; GOAT_NO_BAKE).
- **Particles**: Orbit, OrientationAxisLock, Collision (rays vs fixed colliders), TypeDataMesh
  (+MeshRotation/Rate, MeshMaterial) drawn as one lit mesh per emitter. Beams/ribbons not drawn.
- **Natives**: GetGameSequence / FindSeqObjectsByClass / SequenceEvent.CheckActivate (queued
  in vm.kismet_activations, fired by kismet_tick), OwnsAppID (false; GOAT_OWN_DLC), ToHex,
  RDiff, ClockwiseFrom, IsChildState (Vm::state_extends), view target, and no-ops for LOD/tick/
  streaming calls.

- **19 Oct round**: Host::run_timers splits UI-object timers (tick_menus, also while paused) from
  world timers; one-shots fire once. Workshop items carry no FileHandle (downloaded).
  CART_PHYSX_MASS 258 from boxes x Density 130. PostProcessVolumes: Kind::PostProcess{priority}
  (world.rs), settings in LevelLook.volume_post (post::PostSettings::from_props: struct defaults,
  bOverride-false fields keep the level's), look::post_volumes picks the highest priority around
  the camera and lerps over DOF_InterpolationDuration; post.wgsl adds scene tone. Materials:
  MatInfo.normal/spec/spec_power; normal maps green-flipped, tangents generated for meshes under
  normal-mapped or specular lightmapped materials; Phong power -> roughness sqrt(2/(p+2)).
  Lightmap mip 1 = (shadow, HL2 basis weight 0, weight 1); patched bevy_pbr lightmap_spec() adds
  colour x 3 x sum(w_i x sat(R.b_i)^16) x SpecularColor (reflectance x 2) in tangent space (UE3
  bitangent flipped). Shadow cascades to 450 m (GOAT_SHADOW_DIST). Particles: Ribbon/Trail type
  data (strip through the trail), Beam2 (source/target/distance, BeamNoise points per
  NoiseLockTime), SpawnPerUnit. Beams need script endpoints (SetBeamSourcePoint/TargetPoint) for
  the DLC.

- **Performance (19 Oct, RTX 4070 SUPER, main monitor)**: GPU 1.3-4 ms a frame
  (GOAT_GPU_TIMING=1 prints per-pass GPU time at the end of a test). The CPU was the limit:
  18.3 ms main update in town. goat_move's rapier KinematicCharacterController iterated its 20-step
  slide loop every frame because the grounded goat pushed down into the ground (6.7 ms); grounded
  walking now moves with y = 0 (snap_to_ground follows slopes) -> 0.1 ms. The Scaleform HUD
  captures every other frame (3.3 -> 1.5-1.8 ms). Main update now ~7 ms. prof.rs scope timers
  (crate::prof::scope) on ~40 systems print "CPU ms per frame by system" at the end of a test;
  GOAT_NO_VSYNC=1 asks for no vsync (the driver still capped windowed mode at 60 here).
- **Release**: tools/package.py builds the `dist` profile (stripped) into
  dist/GoatRust-<date>-win64.zip (GoatRust.exe, mods/README.md, the example Rust mods, README.txt)
  and refuses to package game data. upk::game_dir finds the install via GOATSIM_DIR,
  goatsim_dir.txt next to the exe, the default Steam folder, or any Steam library folder. The
  developer overlay is off when playing (F1 toggles, on in tests, GOAT_DEBUG_HUD=1).

## Next steps (in order)

1. Port the next native/script systems 1:1 using out/decomp (see Docs/FIDELITY.md): props/KActor
   (GGKActor, explosives, Apex destructibles), mutators, vehicles, Kismet, quests, menus.
2. Fan specials; race leaderboards are stubs (no online stats); the goat rider (SpawnGoatRider
   NPC) isn't drawn.
3. NPC bikers. Static-mesh materials through the diffuse_graph baker (world.rs).
5. DLC maps (Heist/MMO/Space/Zombie folders), vehicles; grow the UnrealScript VM (see its
   section). Menus: pause menu, HUD, main menu with all its views run; the Downloaded list's
   status column shows N/A (localized labels), inventory (I) isn't ported.
6. (Landscape: checked, the maps' materials have no normal/specular; the per-component MICs only
   pick layers.)
7. Lighting: DLC maps' lightmaps untested. Beam endpoint natives for DLC scripts.

## Menus: Scaleform GFx through Ruffle

Started 2026-10-18. The game's own pause menu (GGHUDGfxMenuPause -> GGUIViewPauseMenu) runs in game:
Escape -> GGPlayerControllerGame.ShowPauseMenu in the VM builds it through the original scripts,
it is drawn over the 3D view, takes mouse/keys, and its buttons work (Resume, Escape).
The in-game HUD movie (GGHUD.StartHUD -> GGHUDGfxInGame, UI_InGame.UI_HUD, legacy combo HUD) runs
under it all the time: score top left, combo widget with timer bar and trick list, fed by the Rust
combo system through the original scripts. The main menu (GGHUD.StartMainMenu -> GGHUDGFxMenuMain ->
GGUIViewMainMenu) runs headless (`goat --uvm-menu main`), not yet in game.

- reference/ holds third-party checkouts (excluded from the workspace): `ruffle` (Rust Flash
  player), `UEViewer` (UE format reference), `PhysX-3.4` (APEX 1.4 source, NvParameterized - for
  reading GGApexDestructibleActor assets later).
- The menus/HUD are SwfMovie objects (AS3, SWF 21, Scaleform CLIK). `gfxui` crate:
  `gfx_to_swf` (GFX -> FWS and drop Scaleform tags 1000+, which sit before FileAttributes and made
  Ruffle treat the movies as AVM1), `write_movie` (package folders so `../UI_Common/gfxfontlib.swf`
  imports resolve via Ruffle's file navigator), `register_fonts`, `Movie` (load_file_with_images,
  tick, input: mouse_move/mouse_button/mouse_wheel/key/text, set_transparent, set_size, capture ->
  RGBA, and the handle API below). A `Movie` is `Send` by an unsafe impl (Ruffle uses Rc inside; the
  VM uses it from one thread at a time).
- Ruffle patches (tools/ruffle-goatrust.patch on the commit in tools/ruffle-goatrust.base; regenerate
  with `git -C reference/ruffle diff > tools/ruffle-goatrust.patch`):
  - `core/src/gfx_bridge.rs`: AS3 values held by `Handle`s (gc-arena DynamicRoot): root, definition,
    get/set (array elements by index name), invoke, call, construct ("Object", "Array",
    "scaleform.clik.data.DataProvider"), attach_movie (class by linkage, addChild/addChildAt + named
    property), create_empty_movie_clip, `host_function(id)`: an AS3 Function that queues
    `HostCall::Function { id, args }` (a copy of the builtin variadic `trace` with another native
    body, bound to receiver = id; `Method::with_native_impl` in avm2/method.rs, `BoundMethod::scope`
    in avm2/function.rs). `take_calls()` drains the queue (thread local; drained right after each
    Player call). `enable_host_calls(true)` routes `ExternalInterface.call(name, ...)` to
    `HostCall::External` (external_interface.rs) and makes `ExternalInterface.available` true.
  - backend/navigator.rs: `NullNavigatorBackend::image_base_path` serves `img://Pkg.Tex` URLs from
    `<folder>/<pkg.tex>` (the URL host, lower case).
  - a Scaleform-style getDefinitionByName fallback (unqualified name, root domain: CLIK lists ask
    for "gg.menu.MainListRenderer").
- VM natives (`goat/src/uvm/gfx.rs`, state in `Vm::gfx`): GFxMoviePlayer Start (MovieInfo RawData ->
  <temp>/goatrust_gfx/<Package>/<Name>.swf + UI_Common/gfxfontlib.swf, written atomically and shared
  by all game processes; fonts from GfxUI.int [FontLib]/[Fonts] as `$Alias` device fonts; root clip
  after up to 30 frames), Advance, PostAdvance (-> OnPostAdvance delegate), Close (-> OnClose, unload),
  GetVariable*/SetVariable* by path ("root.mRootObject"), Get/SetVariable*Array, CreateObject,
  CreateArray, Invoke, ActionScriptVoid/Int/Float/String/Object/Constructor, ActionScriptSetFunction;
  GFxObject Get/Set (+Bool/Float/Int/String/Object), GetText/SetText, SetFunction, Invoke,
  ActionScript*/ActionScriptArray, ActionScriptSetFunction(On), AttachMovie, CreateEmptyMovieClip,
  GotoAndPlay/Stop(I), SetVisible, Get/SetPosition, Get/SetDisplayInfo (scale/alpha in percent),
  all element / element-member accessors, TranslateString; the rest are no-ops (capture keys,
  viewport, focus, 3D matrices, colour transforms).
  - GFxObjects: every Get/Attach returns a fresh script object of the asked class (default
    GFxObject), Outer = its movie player (GFxObject is `within GFxMoviePlayer`, so widgets call
    Close/GetPC/ActionScriptConstructor on it), bound to a Handle (`Gfx::objs`).
  - ActionScript* natives pass the calling script function's own parameters (`vm.frames.last()`);
    ActionScriptSetFunction(On) takes its first delegate parameter. That is how
    GFxClikWidget/GGUIWidget.AddEventListener ends in CLIK addEventListener with a host function.
  - Calls from ActionScript (`pump`): host functions -> the delegate's function with AS args
    converted to its parameter types (struct params member by member, `_this` = the object itself,
    which is what EventData._this checks); CLIK's init callback (Extensions.CLIK_addedToStageCallback,
    set at Start, with isGFxPlayer = true) -> WidgetInitialized(name, path, widget of the
    WidgetBindings class) and PostWidgetInit after that Advance (GGHUDGfxMenu -> SubPostWidgetInit ->
    widget Setup); ExternalInterface.call(name) -> function `name` on the movie's ExternalInterface.
  - Strings starting with `img://` passed into a movie are decoded first (Texture2D from the VM's
    packages -> PNG in <temp>/goatrust_gfx/img/<path lower case>).
  - Ruffle vs Scaleform: the menus' software cursor (root.mCursor, gg.menu.Cursor follows the pointer)
    is made mouseEnabled = false at Start: Ruffle picked it under the pointer and it swallowed every
    press (buttons only saw rollOut/dragOut).
- Movies with bCaptureInput False (the HUD) are only drawn: `gfx::top()` (input target) is the
  newest movie with bCaptureInput, `gfx::menu_open()` = any such movie is open (ui_open, pausing).
- HUD: the loader thread (plugin.rs sync_mutators) calls `Host::start_hud` after the host starts
  (GGPlayerControllerGame.StartHUD -> GGHUD.StartHUD; ~0.3 s, off the game thread; GOAT_NO_HUD=1
  skips it). `score::Score` queues `HudEvent`s the way GGComboManager reports to the HUD and
  `Host::hud_feed` replays them each frame: per trick `mHUDMovie.AddCombo(TrickInfo, trickScore,
  CalculateTotalScore(false), mMultiplier, eligible, name)` (TrickInfo built from AddCombo's param
  struct), on a banked combo GGGameInfo.mTotalScoreData = [{PC, Score}] then EndCombo + UpdateScore
  (the score widget waits 1.5 s via a PC timer, then counts up per TickScore), every frame
  UpdateComboBar(since_last, 5) + TickScore (GGHUD.DrawHUD). The Bevy debug score text is hidden
  while `menus::hud_shown()`, and the help text moves to the bottom left.
- Host (`goat/src/uvm/menus.rs`): `Host::open_pause_menu` (ShowPauseMenu on the controller),
  `open_main_menu` / `main_menu`, `start_hud` / `hud_movie` / `hud_feed`, `tick_menus`,
  `pause_buttons` / `menu_buttons(menu)` (labels + screen rects of the top view's mMenuButtons). In
  game `scaleform_menus` (plugin.rs, before read_input) runs while any movie is open: Escape opens
  the pause menu when no menu/mod menu is open; while a menu is open it gets the mouse (window pixels, ShowAll 1920x1080 stage), wheel and keys
  (Escape/Enter/arrows/Tab/Space/Backspace as Unreal names: FilterButtonInput first, then a Flash
  key code; a key release only goes to the menu if it saw the press, else the Escape that opened the
  pause menu would close it again via GGUIViewPauseMenu.SubFilterButtonInput), advances with real
  time (WorldInfo.RealTimeSeconds too). One full-window ImageNode layer per open movie (GlobalZIndex
  40 + i): movies without bCaptureInput (the HUD) first, then the menus in opening order, so a
  GGHUDGfxPopup keeps the pause menu visible under its dim ModalBG (`--test menuflow` shot). A movie is rendered + read back (Ruffle offscreen) only when Ruffle's
  `needs_render` says a movie frame ran or input changed it (`gfx::capture_if_changed`), else the
  layer keeps its last image: ~2 ms/frame for HUD + VM feed at 1600x900 (logged as `[gfx]
  Scaleform movies: x ms/frame` every 300 frames). Premultiplied -> straight alpha,
  and partly transparent pixels get alpha 1 - (1 - a)^2.2 so Bevy's linear blending darkens like
  Scaleform's gamma-space blending. `uvm::menus::ui_open()` keeps the goat, camera and mouse grab off;
  virtual time is paused while the menu is open (the game freezes under it).
- Checks: `goat --uvm-menu pause|hud|main [out.png]` (headless: opens the pause menu / HUD / main
  menu through the scripts, prints each movie's bCaptureInput; for the pause menu:
  prints its buttons, saves the frame, clicks Resume and checks the menu closed; UVM_GFX_LOG=1 logs
  every call/callback with the AS event type, UVM_GFX_PROBE=1 logs the stage's mouse event targets,
  UVM_GFX_IMG=Characters.Textures.KA_Dif_01 loads that texture through the HUD movie's
  CraftingHelp.SetTexture (img://) and prints its size; the full log goes to out/uvm_menu_log.txt).
  `goat --test pausemenu --shot out/pausemenu.png`: opens it in game (test input only, never the
  user's mouse/keys), checks 9 buttons and that the game is paused, screenshots, clicks Resume (must
  close and unpause), reopens and closes it with Escape.
  `goat --test hud --shot out/hud.png`: waits for the HUD movie, reports three tricks into
  `score::Score`, checks the legacy combo widget's multiplier matches (screenshot: "245 X 3" + the
  trick list), then that the score widget counts up to the banked total (fails if it stalls ~4 s).
  `cargo run -p gfxui --release --example menu_probe -- out/swf out/x.png` (bridge alone: init
  callbacks, listeners, a click), `--example abc_dump <swf> <class filter> [method]` disassembles
  AS3 classes (out/frontend_abc.txt, out/hud_abc.txt hold full dumps).
- Fixed on the way: Object.ParseStringIntoArray took the delimiter from the out-array slot (SplitString
  returned nothing; the combo widget's FloatToString printed an empty multiplier).
- Not yet: the main menu in game (needs the menu level and Play -> Custom Game -> map -> level load;
  headless Play pushes GGUIViewCustomGame, whose map list logs Accessed None / out of bounds and
  GGEngine.GetCSSDLCManager is missing), HUD pieces not fed: combo breaker (BreakCombo), juice
  for special tricks (AddJuice), quest widget, inventory, secrets counter, driving HUD; scoreText's
  unclosed `</font` makes Ruffle warn "tag not closed" (it still renders), quests (no
  GGQuestManager: empty quest list), 3D rotationY on the button backgrounds (Ruffle ignores it),
  menu sounds go through PlaySound (silent in tests), Options' resolution/fullscreen (`setres` is
  logged, the window is not changed), language change, Leaderboards/Achievements (Steam UI).

### Pause menu views, popups, actions and settings (2026-10-18)

- Views: Options and Key bindings open from the pause menu and Escape goes back (GGUIViewOptions
  fully populated: FOV 70, gamma, volumes, resolution list, language; Key bindings lists the real
  bindings). Popups: Restart / Main menu / Exit Game show GGHUDGfxPopup (UI_Popup) with their
  localized text; Yes/No run the scripts' callbacks.
- What made them work:
  - Ruffle swf reader (`swf/src/read.rs`): a PlaceObject3 with only a class name (no character id,
    no move; UI_Popup places s_ScrollBar_track/thumb that way) parses as a modify instead of
    failing the whole sprite (it lost the popup's header/body).
  - Ruffle `movie_clip.rs`: gotoAndPlay/Stop on a missing frame label only warns while host calls
    are enabled (Scaleform; Options' MenuValueSlider asks for "default").
  - gfx_bridge `err()` reports the thrown AS3 error (class #id: message).
  - gfx.rs Start: the CLIK init callback is installed before each early frame, then
    `replay_widget_init` re-sends it for widgets already on stage with enableInitCallback (timeline
    widgets announce themselves in the frame that defines the classes, before any hook exists).
    ExternalInterface.call goes to the movie player itself when ExternalInterface is None (UE3;
    the popup's Callback_PopupActivated).
  - CLIK widgets finish setup (configUI, mouse listeners) on Flash's RENDER event, which Ruffle
    sends when a frame is rendered: headless checks must render after ticks (`menu_actions::frame`).
  - gfxui: all movies share one wgpu device (`offscreen_renderer`); `gfxui::warm_up()` creates it
    on a thread when the host loads (plugin.rs), so opening a menu costs ~50 ms instead of 0.3-0.7 s.
- `uvm/settings.rs`: the script world's engine is a GGEngine (bridge.rs) with mSettings
  (GGSystemSettings: GetAvailableResolutions from common sizes + the window, named Int/Float/Bool
  settings stored on the object, fullscreen off / vsync on by default); the controller gets a
  GGProfileSettings (ProfileSettings from DefaultSettings; Get/SetProfileSettingValueFloat/Int/Id,
  floats as their bits in Data.Value1); UIManager.GetUIManager -> one GGUIManager; LocalPlayers'
  Outer = the engine; GetLanguage "INT"; SetAudioGroupVolume / SetAllowMatureLanguage. Options ->
  Apply publishes `settings::applied()`: FOV goes to the camera, SFX/voice volume to the sound
  player (npc_* cues = voice), MouseSensitivity (/60) and invert X/Y to mouse look.
- `uvm/menu_actions.rs`: `find_labeled` (a visible widget by label/text anywhere in the top movie)
  and `Host::click_label`; Respawn (GGUIViewPauseMenu.RespawnPressed override: respawns the Rust goat,
  closes the menu; the respawn reaches scripts as the usual OnPlayerRespawn event), ClientTravel
  (Restart -> Yes: a new process on the same map, like the mod menu's map switch), ConsoleCommand
  "exit"/"quit" (Exit Game -> Yes: AppExit). Test runs only log restart/quit (MenuActionsDone).
- Checks: `goat --uvm-menu-click "Options,key:Escape,Restart,No" [out.png]` (headless; per step
  the open movies and view stack, warnings, then the queued actions and applied settings; full log
  out/uvm_menu_click_log.txt). `goat --test menuflow --shot out/menuflow.png` (in game: Options +
  Escape, Restart popup screenshot + No, Respawn brings the goat back from 8 m away).
- Builds: this work used `--target-dir target/pm` while another session ran target/release.

## Decompiled game (local only, out/ — never distribute)

Everything is decompiled; see `out/decomp/README.md`:
- `out/scripts/<Package>/*.uc`: all 2353 UnrealScript classes (shipped source text).
- `out/decomp/native/`: 239k native functions as Ghidra pseudo-C (full run; vtable-only functions,
  tools/ghidra/DecompileMissing.java; orphan code + timeouts, tools/ghidra/SecondPass.java). 11 giant
  editor/console/compiler functions only as assembly (`asm_<addr>.s`). `python -I tools/fn.py
  <addr|name>` prints one. Names (140k): `Class_vSLOT[_known]` virtuals + `Class_vtable` +
  `Class_InternalConstructor` (tools/name_classes.py, 1805 class vtables from the UClass
  registrations), 2236 `CppClass_execNative` (tools/name_natives.py), 3379 `FN_*` FName globals,
  5234 `eventXxx` thunks. `class_slots.tsv` resolves `(*param_1 + 0xNNN)` virtual calls.
  Re-run order after a new Ghidra pass: name_natives.py --apply, name_classes.py --apply,
  decomp_index.py.
- `out/decomp/natives.tsv`: native -> script declaration; `out/decomp/managed/`: the exe's C++/CLI
  part (ILSpy, editor tooling only).
- Script-only classes (GGGoat, GGPawn, GGNpc...) run on their nearest native class's vtable:
  AGGPawnBase = 0x26c24b0 (found via its StaticClass registration `L"GGPawnBase"` -> constructor).
  Slots: 0x254 physFalling, 0x258 physWalking, 0x264 physWallRun, 0x4e8 startNewPhysics,
  0x500 NewFallVelocity, 0x504 MaxSpeedModifier, 0x508 GetMaxAccel, 0x524 braking,
  0x528 CalcVelocity (GG: 0x550 walking / 0x554 swimming / 0x558 spider), 0x55c max speed by dir.
- Docs/FIDELITY.md (tools/fidelity.py): every GoatGame class vs. the port.

## REA / Ghidra notes

The exe is mixed-mode, so REA only accepts `out/rea/GoatGame-native.exe` (CLR directory zeroed).
Ghidra needs ~15-20 min: `REA_GHIDRA_STARTUP_TIMEOUT_MS=7200000`,
`REA_GHIDRA_DISABLED_ANALYZERS="Decompiler Parameter ID"` (set in the rea MCP env). Use one
open_binary session. UE3 FName globals are set up in big registration functions; eventX wrappers
reference them (CanWallRun FName 0x2bb76e0 -> eventCanWallRun 0xb21230). APawn field dwords:
0x51 Velocity.Z, 0x75 flags (8 = mReadyToWallRun), 0x78-0x81 mMaxRunnableWallZ..mMaxStepOverWallBump,
0xd1 AccelRate, 0xdd Floor. GGGoat vtable around 0x26b13e4 (physFalling, physWalking, ...).
