use avian3d::prelude::*;
use bevy::prelude::*;
use lightyear::avian3d::plugin::AvianReplicationMode;
use lightyear::avian3d::prelude::*;

pub mod auth;
pub mod player;
pub mod protocol;
pub mod solana;
pub mod world;

pub const PROTOCOL_ID: u64 = 7;
pub const SERVER_PORT: u16 = 5000;
pub const FIXED_TIMESTEP_HZ: f64 = 64.0;

/// Locate the absolute path to the `assets/` directory at runtime.
///
/// Pass the returned string to `AssetPlugin { file_path, .. }` so Bevy's
/// asset server reads from the right place regardless of how the binary was
/// launched.
///
/// Search order (first match wins):
/// 1. `BEVY_ASSET_ROOT` env var → explicit user/bundle override
/// 2. CWD-relative `./assets/` (`cargo run`, systemd WorkingDirectory, etc.)
/// 3. `<exe_dir>/assets/` (production install layout)
/// 4. `<exe_dir>/../Resources/assets/` (macOS `.app` bundle layout)
/// 5. `CARGO_MANIFEST_DIR/assets/` baked at compile time (dev fallback)
pub fn asset_root_path() -> String {
    let ok = |p: std::path::PathBuf| {
        if p.is_dir() { Some(p.to_string_lossy().into_owned()) } else { None }
    };

    if let Ok(root) = std::env::var("BEVY_ASSET_ROOT") {
        if let Some(p) = ok(std::path::PathBuf::from(&root).join("assets"))
            .or_else(|| ok(std::path::PathBuf::from(&root)))
        {
            return p;
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(p) = ok(cwd.join("assets")) { return p; }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if let Some(p) = ok(exe_dir.join("assets")) { return p; }
            if let Some(contents) = exe_dir.parent() {
                if let Some(p) = ok(contents.join("Resources").join("assets")) { return p; }
            }
        }
    }
    if let Some(p) = ok(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")) {
        return p;
    }
    "assets".to_string()
}

/// Shared plugin added by both client and server:
/// registers protocol, physics, frame interpolation, and shared movement.
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // Protocol: components + BEI input registration
        app.add_plugins(protocol::ProtocolPlugin);

        // Avian3d physics with lightyear integration
        // PositionButInterpolateTransform: lightyear handles Position→Transform sync
        // with smooth correction for predicted entities and interpolation for remote entities.
        app.add_plugins(LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::PositionButInterpolateTransform,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>()
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        );

        // Disable gravity for kinematic players (we handle gravity ourselves).
        // Other dynamic entities (like ore chunks) still use default gravity.
        app.insert_resource(Gravity(Vec3::new(0.0, -9.81, 0.0)));

        // Note: FrameInterpolationPlugin is NOT needed — PositionButInterpolateTransform
        // mode handles Position→Transform and Rotation→Transform sync with smooth correction.

        // Shared FixedUpdate systems. These replace the old BEI `On<Fire<...>>`
        // observers — leafwing's ActionState is snapshot/restored cleanly during
        // lightyear rollback, so movement/look/jump/interact/etc can be replayed
        // without the rubber-banding that event-based observers caused.
        //
        // Movement reads the world-space Move axis (rotated on the client before
        // BufferClientInputs). With zero input the movement system zeros XZ vel
        // directly — no separate clear_xz_velocity step required.
        app.add_systems(
            FixedUpdate,
            (
                player::shared_look_system,
                player::shared_movement_system,
                player::shared_jump_system,
                player::character_controller,
                player::sync_rotation_from_yaw,
                world::shared_door_interact_system,
                world::shared_equip_interact_system,
                world::shared_drop_system,
                world::shared_jab_system,
                world::shared_primary_action_system,
                world::reset_stale_mining,
            )
                .chain(),
        );
    }
}
