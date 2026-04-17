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

/// Shared plugin added by both client and server:
/// registers protocol, physics, frame interpolation, and shared movement.
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // Protocol: components + leafwing input registration
        app.add_plugins(protocol::ProtocolPlugin);

        // Avian3d physics with lightyear integration.
        //
        // `Position` mode: lightyear replicates & rolls back `Position`/`Rotation`
        // directly (matching the `lightyear/examples/avian_3d_character` pattern).
        // This is the right choice for a dynamic rigid body controller because
        // avian's integrator is the authority on Position, and lightyear's
        // prediction replays the same forces/impulses through the integrator to
        // get a deterministic result — no bespoke Position writebacks to clash
        // with lightyear.
        app.add_plugins(LightyearAvianPlugin {
            replication_mode: AvianReplicationMode::Position,
            ..default()
        });
        app.add_plugins(
            PhysicsPlugins::default()
                .build()
                .disable::<PhysicsTransformPlugin>()
                .disable::<PhysicsInterpolationPlugin>()
                // Sleeping can stash state that doesn't survive rollback cleanly.
                .disable::<IslandPlugin>()
                .disable::<IslandSleepingPlugin>(),
        );

        // Real gravity — avian integrates it into dynamic bodies each tick.
        // The old code zeroed gravity because our custom kinematic controller
        // applied its own; now avian owns the whole vertical axis.
        app.insert_resource(Gravity(Vec3::new(0.0, -9.81, 0.0)));

        // Shared FixedUpdate systems. `handle_character_actions` replaces our
        // old custom kinematic controller: it walks every player, reads the
        // leafwing ActionState, and applies forces/impulses via avian's
        // `Forces` query data. The avian `PhysicsSchedule` (in FixedPostUpdate)
        // then integrates those forces into Position — the same integration
        // runs again on replay, which is what keeps us rollback-deterministic.
        app.add_systems(
            FixedUpdate,
            (
                player::shared_look_system,
                player::handle_character_actions,
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
