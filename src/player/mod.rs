use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::*;
use avian3d::prelude::forces::ForcesItem;
use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use leafwing_input_manager::prelude::*;

use avian3d::prelude::Rotation;
use lightyear::prelude::{Controlled, Interpolated};

use crate::protocol::{
    PlayerActions, PlayerDead, PlayerEquipped, PlayerHealth, PlayerId, PlayerPitch, PlayerYaw,
};

// --- Movement feel tunables ---
//
// With avian's dynamic rigid body pattern, motion is produced by applying
// forces (horizontal) and impulses (vertical). These constants tune the feel.
/// Horizontal ground speed cap (m/s).
pub const MAX_SPEED: f32 = 7.0;
/// Max horizontal acceleration (m/s²). Controls how snappy start/stop/strafe feels.
pub const MAX_ACCELERATION: f32 = 50.0;
/// Vertical impulse applied on jump (m/s ⋅ kg — effective jump height scales
/// with inverse mass). With avian's default mass and Gravity(-9.81), ~5.4 lifts
/// us ~1.5 m.
pub const JUMP_IMPULSE: f32 = 5.4;

pub const VIEW_MODEL_RENDER_LAYER: usize = 1;
pub const PLAYER_SPAWN_POS: Vec3 = Vec3::new(0.0, 1.5, 5.0);

/// Spawn points spread across the Colorado wilderness compound.
/// Each position is placed on valid ground with Y offset for the capsule half-height.
pub const SPAWN_POINTS: &[Vec3] = &[
    Vec3::new(0.0, 1.5, 5.0),      // Cabin porch (default spawn)
    Vec3::new(-14.0, 1.2, 2.0),    // Inside the equipment shed
    Vec3::new(19.0, 1.5, -2.0),    // Outside mine entrance
    Vec3::new(-7.5, 4.8, -7.5),    // Watchtower platform
    Vec3::new(3.0, 1.0, 10.0),     // Campfire area
    Vec3::new(-10.0, 1.5, -15.0),  // NW boulder cluster
    Vec3::new(12.0, 1.5, -16.0),   // NE rocky ridge
    Vec3::new(10.0, 2.0, 3.0),     // Near the old truck
];

/// Pick the spawn point furthest from all living players.
/// Falls back to a random spawn point if no other players exist.
pub fn select_spawn_point(living_positions: &[Vec3]) -> Vec3 {
    if living_positions.is_empty() {
        // No other players — pick a random spawn point
        let idx = rand::random::<usize>() % SPAWN_POINTS.len();
        return SPAWN_POINTS[idx];
    }

    // Pick the spawn point with the greatest minimum distance to any living player
    SPAWN_POINTS
        .iter()
        .max_by(|a, b| {
            let min_dist_a = living_positions.iter().map(|p| a.distance(*p)).fold(f32::MAX, f32::min);
            let min_dist_b = living_positions.iter().map(|p| b.distance(*p)).fold(f32::MAX, f32::min);
            min_dist_a.partial_cmp(&min_dist_b).unwrap_or(std::cmp::Ordering::Equal)
        })
        .copied()
        .unwrap_or(PLAYER_SPAWN_POS)
}

/// Capsule dimensions (must match Collider in physics bundle).
/// These are the half-extents avian uses: total height ≈ 2*radius + height = ~2m.
pub const CAPSULE_RADIUS: f32 = 0.5;
pub const CAPSULE_HEIGHT: f32 = 1.0;

// --- Shared Components (used by both server + client) ---

#[derive(Debug, Component)]
pub struct Player {
    pub id: u64,
}

// --- Client-Only Components ---

#[derive(Debug, Component, Deref, DerefMut)]
pub struct CameraSensitivity(Vec2);

impl Default for CameraSensitivity {
    fn default() -> Self {
        Self(Vec2::new(0.003, 0.002))
    }
}

#[derive(Resource)]
pub struct CursorState {
    pub locked: bool,
}

impl Default for CursorState {
    fn default() -> Self {
        Self { locked: true }
    }
}


// --- Shared Bundles ---
// These ensure server and client have identical physics/gameplay components.
// Define once here, use in both server.rs and client.rs.

/// Physics for locally-simulated player entities — the server's authoritative
/// player and the client's predicted (controlled) player.
///
/// Dynamic rigid body driven by avian's integrator. Forces applied via
/// `ForcesItem` give deterministic motion under lightyear's rollback — unlike
/// our old kinematic character controller, whose SpatialQuery-driven writes
/// produced different results when replayed because it sampled the *current*
/// collider positions rather than the replay-tick positions.
///
/// Rotations are locked on all axes so the capsule never tips over; our
/// `sync_rotation_from_yaw` system writes `Rotation` each tick based on the
/// replicated yaw/pitch so the facing direction is still deterministic.
/// Zero friction with Min combine prevents sticky contacts that would cause
/// rubber-banding on sloped geometry.
pub fn player_physics_bundle_dynamic() -> impl Bundle {
    (
        Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT),
        RigidBody::Dynamic,
        LockedAxes::default()
            .lock_rotation_x()
            .lock_rotation_y()
            .lock_rotation_z(),
        Friction::new(0.0).with_combine_rule(CoefficientCombine::Min),
    )
}

/// Physics for the client's view of remote, interpolated players. Kinematic so
/// avian doesn't try to simulate gravity/forces locally — lightyear drives
/// Position/Rotation from interpolated snapshots instead. Collider is still
/// present so local ray-casts (tracers, prediction-only visuals) hit them.
pub fn player_physics_bundle_kinematic() -> impl Bundle {
    (
        Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT),
        RigidBody::Kinematic,
    )
}

/// Back-compat alias used by a couple of sites that don't care which variant
/// — just need a collider + rigid body. Defaults to the kinematic flavour so
/// passive use doesn't accidentally spawn a dynamic body that falls forever.
pub fn player_physics_bundle() -> impl Bundle {
    player_physics_bundle_kinematic()
}

/// Replicated gameplay state for a player entity.
/// Server spawns these; client receives them via lightyear replication.
///
/// `ActionState<PlayerActions>` is the leafwing equivalent of the old BEI
/// `PlayerContext` marker — it's the replicated input component queried each
/// FixedUpdate by shared movement/shoot/etc systems on both ends.
///
/// Note: `LinearVelocity` / `AngularVelocity` / `ComputedMass` are added
/// automatically by avian as required components of `RigidBody`, so we don't
/// include them here. They are still registered for replication in
/// `ProtocolPlugin` so predicted clients see the server's velocity state.
pub fn player_replicated_bundle(client_id: u64) -> impl Bundle {
    (
        ActionState::<PlayerActions>::default(),
        PlayerId(client_id),
        PlayerYaw::default(),
        PlayerPitch::default(),
        PlayerEquipped::default(),
        crate::protocol::PlayerInventory::default(),
        PlayerHealth::default(),
        crate::protocol::LastDamagedBy::default(),
        crate::protocol::LastShot::default(),
        Position(PLAYER_SPAWN_POS),
        Rotation::default(),
    )
}

// --- Shared Character Action Application (FixedUpdate, runs on both client + server) ---

/// Apply the character action for a single entity. Mirrors the
/// `lightyear/examples/avian_3d_character` pattern:
///
/// - Jump: on `just_pressed(Jump)`, raycast down from the capsule bottom; if
///   we're grounded, apply an instantaneous linear impulse upwards.
/// - Move: compute a target ground velocity from the pre-rotated Move axis,
///   then apply the force required to reach that target this tick, bounded by
///   `MAX_ACCELERATION`.
///
/// This is deterministic under rollback because forces are applied to an
/// avian rigid body — avian's integrator consumes the same forces each replay
/// and produces the same position delta. The old kinematic controller's
/// SpatialQuery-driven writes weren't rollback-safe because SpatialQuery reads
/// the *current* collider positions, not the replay-tick positions.
pub fn apply_character_action(
    entity: Entity,
    mass: &ComputedMass,
    time: &Res<Time>,
    spatial_query: &SpatialQuery,
    action_state: &ActionState<PlayerActions>,
    mut forces: ForcesItem,
) {
    // How much horizontal velocity can change in a single tick.
    let max_velocity_delta_per_tick = MAX_ACCELERATION * time.delta_secs();

    // --- Jump ---
    if action_state.just_pressed(&PlayerActions::Jump) {
        // Raycast down from the bottom of the capsule; only jump if we hit
        // something right below us.
        let foot = forces.position().0
            + Vec3::new(0.0, -CAPSULE_HEIGHT / 2.0 - CAPSULE_RADIUS, 0.0);
        let grounded = spatial_query
            .cast_ray(
                foot,
                Dir3::NEG_Y,
                0.01,
                true,
                &SpatialQueryFilter::from_excluded_entities([entity]),
            )
            .is_some();
        if grounded {
            forces.apply_linear_impulse(Vec3::new(0.0, JUMP_IMPULSE, 0.0));
        }
    }

    // --- Move ---
    // The Move axis is already in world space (the client pre-rotates by
    // camera yaw before lightyear buffers ActionState). Convention matches the
    // old pre_rotate: +x = world X (right), +y = world -Z (forward) — pressing
    // W at yaw=0 gives axis (0, -1) which maps to world (0, 0, -1).
    let raw = action_state
        .axis_pair(&PlayerActions::Move)
        .clamp_length_max(1.0);
    let move_dir = Vec3::new(raw.x, 0.0, raw.y);

    let linear_velocity = forces.linear_velocity();
    let ground_linear_velocity = Vec3::new(linear_velocity.x, 0.0, linear_velocity.z);

    let desired_ground_linear_velocity = move_dir * MAX_SPEED;

    let new_ground_linear_velocity = ground_linear_velocity
        .move_towards(desired_ground_linear_velocity, max_velocity_delta_per_tick);

    // Acceleration needed to hit the target this tick. `move_towards` already
    // bounds the delta by `max_velocity_delta_per_tick`, so the magnitude is
    // guaranteed ≤ MAX_ACCELERATION.
    let required_acceleration =
        (new_ground_linear_velocity - ground_linear_velocity) / time.delta_secs();

    forces.apply_force(required_acceleration * mass.value());
}

/// FixedUpdate system: walks every player with an ActionState + Forces and
/// delegates to `apply_character_action`. Runs on both the client (for the
/// locally-predicted controlled entity) and the server (for all players).
///
/// Skips interpolated / dead players.
pub fn handle_character_actions(
    time: Res<Time>,
    spatial_query: SpatialQuery,
    mut query: Query<
        (Entity, &ComputedMass, &ActionState<PlayerActions>, Forces, Has<Interpolated>, Has<PlayerDead>),
        With<PlayerId>,
    >,
) {
    for (entity, mass, action_state, forces, is_interpolated, is_dead) in query.iter_mut() {
        if is_interpolated || is_dead {
            continue;
        }
        apply_character_action(entity, mass, &time, &spatial_query, action_state, forces);
    }
}

/// Reads the Look dual-axis (mouse motion) and applies it to yaw/pitch.
/// Runs on both client (prediction) and server (authority); lightyear's
/// ActionState replication means the server sees the same mouse deltas the
/// client buffered.
pub fn shared_look_system(
    mut query: Query<
        (&ActionState<PlayerActions>, &mut PlayerYaw, &mut PlayerPitch, Has<Interpolated>, Has<PlayerDead>),
        With<PlayerId>,
    >,
) {
    for (action, mut yaw, mut pitch, is_interpolated, is_dead) in query.iter_mut() {
        if is_interpolated || is_dead {
            continue;
        }

        let delta = action.axis_pair(&PlayerActions::Look);
        if delta == Vec2::ZERO {
            continue;
        }

        yaw.0 += -delta.x * 0.003;
        const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.01;
        pitch.0 = (pitch.0 + -delta.y * 0.002).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }
}

/// Diagnostic: log player position/velocity every 2 seconds.
pub fn log_player_state(
    query: Query<(Entity, &Position, &LinearVelocity), (With<PlayerId>, With<Collider>)>,
    time: Res<Time>,
    mut timer: Local<f32>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    for (entity, pos, vel) in query.iter() {
        info!(
            "[DIAG] entity={:?} pos=({:.1}, {:.1}, {:.1}) vel=({:.1}, {:.1}, {:.1})",
            entity, pos.0.x, pos.0.y, pos.0.z, vel.0.x, vel.0.y, vel.0.z
        );
    }
}

// --- Shared Systems ---

/// Shared system: syncs PlayerYaw + PlayerPitch → Rotation so lightyear replicates
/// both facing direction and pitch tilt. Runs in FixedUpdate on both client and server.
/// Remote players display correct pitch tilt via the replicated Rotation.
///
/// Compatible with the dynamic rigid body: `LockedAxes` prevents the avian
/// integrator from rotating the capsule, so our direct Rotation writes persist
/// through the physics step.
pub fn sync_rotation_from_yaw(
    mut query: Query<(&PlayerYaw, &PlayerPitch, &mut Rotation), (With<PlayerId>, Without<Interpolated>)>,
) {
    for (yaw, pitch, mut rot) in query.iter_mut() {
        rot.0 = Quat::from_euler(EulerRot::YXZ, yaw.0, pitch.0, 0.0);
    }
}

// --- Client-Only Systems ---

/// Client-only: rotates the Move axis from player-local (WASD) frame to world frame
/// using the current PlayerYaw, BEFORE lightyear's BufferClientInputs captures the
/// ActionState for replication. This way both the server and the prediction system
/// see the same world-space movement vector — the server never has to rotate by yaw
/// itself.
///
/// Runs in FixedPreUpdate in the `InputManagerSystem::ManualControl` set (i.e. after
/// leafwing's Update set populated the raw WASD axis) and before
/// `InputSystems::BufferClientInputs` (so the rotated value is what gets replicated).
pub fn pre_rotate_move_input(
    mut query: Query<(&PlayerYaw, &mut ActionState<PlayerActions>), With<Controlled>>,
) {
    let Ok((player_yaw, mut action)) = query.single_mut() else {
        return;
    };
    let raw = action.axis_pair(&PlayerActions::Move);
    if raw == Vec2::ZERO {
        return;
    }
    let yaw = player_yaw.0;
    // WASD yields: x = strafe (+right), y = forward (+W).
    // World forward at yaw=0 is -Z, world right at yaw=0 is +X.
    let forward = Vec2::new(-yaw.sin(), -yaw.cos());
    let right = Vec2::new(yaw.cos(), -yaw.sin());
    let rotated = forward * raw.y + right * raw.x;
    action.set_axis_pair(&PlayerActions::Move, rotated);
}

/// Client-only: zeros the Look axis when the cursor is unlocked (e.g. Escape pressed).
/// Prevents mouse deltas from being sent to the server when the player isn't in control.
/// Runs in FixedPreUpdate in the `InputManagerSystem::ManualControl` set (after leafwing
/// Update has populated the raw mouse motion) and before BufferClientInputs.
pub fn gate_look_on_cursor(
    cursor_state: Res<CursorState>,
    mut query: Query<&mut ActionState<PlayerActions>, With<Controlled>>,
) {
    if cursor_state.locked {
        return;
    }
    for mut action in query.iter_mut() {
        action.set_axis_pair(&PlayerActions::Look, Vec2::ZERO);
    }
}

/// Client-only: ensures the camera child has identity rotation.
/// The parent's Rotation now includes both yaw and pitch (via sync_rotation_from_yaw),
/// so the camera child inherits the correct orientation automatically.
pub fn sync_camera_pitch(
    player_query: Query<&Children, With<Controlled>>,
    mut camera_query: Query<&mut Transform, With<crate::world::WorldModelCamera>>,
) {
    let Ok(children) = player_query.single() else {
        return;
    };

    for child in children.iter() {
        if let Ok(mut cam_transform) = camera_query.get_mut(child) {
            cam_transform.rotation = Quat::IDENTITY;
        }
    }
}

/// Grab/release cursor on click/escape
pub fn grab_mouse(
    mut cursor_options: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    key: Res<ButtonInput<KeyCode>>,
    mut cursor_state: ResMut<CursorState>,
) {
    let Ok(mut options) = cursor_options.single_mut() else {
        return;
    };

    if key.just_pressed(KeyCode::Escape) && cursor_state.locked {
        cursor_state.locked = false;
    } else if mouse.just_pressed(MouseButton::Left) && !cursor_state.locked {
        cursor_state.locked = true;
    }

    if cursor_state.locked {
        options.visible = false;
        options.grab_mode = CursorGrabMode::Locked;
    } else {
        options.visible = true;
        options.grab_mode = CursorGrabMode::None;
    }
}

/// Adjust FOV with arrow keys
pub fn change_fov(
    input: Res<ButtonInput<KeyCode>>,
    mut camera: Query<&mut Projection, With<crate::world::WorldModelCamera>>,
) {
    if let Ok(mut projection) = camera.single_mut() {
        let Projection::Perspective(ref mut perspective) = projection.as_mut() else {
            return;
        };

        if input.pressed(KeyCode::ArrowUp) {
            perspective.fov -= 1.0_f32.to_radians();
            perspective.fov = perspective.fov.max(20.0_f32.to_radians());
        }
        if input.pressed(KeyCode::ArrowDown) {
            perspective.fov += 1.0_f32.to_radians();
            perspective.fov = perspective.fov.min(160.0_f32.to_radians());
        }
    }
}
