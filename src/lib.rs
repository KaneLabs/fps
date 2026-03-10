use avian3d::prelude::*;
use bevy::prelude::*;
use lightyear::avian3d::plugin::AvianReplicationMode;
use lightyear::avian3d::prelude::*;

pub mod auth;
pub mod player;
pub mod protocol;
pub mod world;

pub const PROTOCOL_ID: u64 = 7;
pub const SERVER_PORT: u16 = 5000;
pub const FIXED_TIMESTEP_HZ: f64 = 64.0;

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

        // Zero XZ velocity every tick; Fire<MoveAction> re-applies if keys are held.
        app.add_systems(FixedFirst, player::clear_xz_velocity);

        // Kinematic character controller: gravity, ground detection, move-and-slide.
        app.add_systems(FixedUpdate, player::character_controller);

        // Sync PlayerYaw+PlayerPitch → Rotation for all players. Runs on both client (prediction)
        // and server (authority). Lightyear's Avian plugin syncs Rotation → Transform for rendering.
        app.add_systems(FixedUpdate, player::sync_rotation_from_yaw);

        // Reset stale mining state (detects when player stops holding mine button)
        app.add_systems(FixedUpdate, world::reset_stale_mining);

        // Shared observers (fire on both client predicted + server authoritative via BEI replay)
        app.add_observer(player::shared_look);
        app.add_observer(player::shared_movement);
        app.add_observer(player::shared_jump);
        app.add_observer(world::shared_door_interact);
        app.add_observer(world::shared_equip_interact);
        app.add_observer(world::shared_drop);
        app.add_observer(world::shared_jab);
        app.add_observer(world::shared_primary_action);

    }
}
