# FPS Multiplayer Game

## Stack
Rust, Bevy 0.18, Lightyear 0.26, Avian3d 0.5, bevy-enhanced-input 0.22, bevy_egui 0.39

## Architecture
- `src/lib.rs` — SharedPlugin (protocol, physics, shared observers) used by both binaries
- `src/protocol.rs` — Replicated components, BEI input actions, prediction config
- `src/player/mod.rs` — Player components, shared movement/jump, client-only camera systems
- `src/world/mod.rs` — World geometry, interactables, client-only interaction UI
- `src/bin/server.rs` — Headless server binary
- `src/bin/client.rs` — Client binary with rendering and input

## Critical Rules

### Interactables
See `.claude/INTERACTABLES.md` (auto-loaded). Key rule: **if it has a Collider, the server must know about it. If it changes, the server must change it.**

### Interpolation vs Prediction
See `.claude/INTERPOLATION.md` (auto-loaded). Key rule: **interpolated (remote) entities must never run local physics.** Gate shared systems with `Without<Interpolated>`.

### Shared Bundles
Use `player_physics_bundle()` and `player_replicated_bundle()` from `player/mod.rs` when spawning player entities. Never duplicate physics components between server and client.

### Input Flow
Client WASD → BEI captures raw Vec2 → `pre_rotate_move_input` rotates by camera yaw → BEI buffers world-space Vec2 → lightyear replicates to server → `shared_movement` applies directly. Camera yaw does NOT replicate to server — input is pre-rotated instead.

### Replicated Components
Every component that needs to sync between client and server must be registered in `protocol.rs` via `app.register_component::<T>()`. Add `.add_prediction()` for predicted components.
