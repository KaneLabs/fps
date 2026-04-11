use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::*;
use bevy::{
    input::mouse::AccumulatedMouseMotion,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use bevy_enhanced_input::prelude::*;

use avian3d::prelude::Rotation;
use lightyear::prelude::{Controlled, Interpolated};

use crate::protocol::{CharacterVelocity, JumpAction, LookAction, MoveAction, PlayerContext, PlayerDead, PlayerEquipped, PlayerHealth, PlayerPitch, PlayerYaw};

pub const PLAYER_MOVE_SPEED: f32 = 7.0;
pub const JUMP_SPEED: f32 = 10.0;
pub const GRAVITY: f32 = 32.0;
pub const SKIN_WIDTH: f32 = 0.02;
pub const STEP_HEIGHT: f32 = 0.1;
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

/// Capsule dimensions (must match Collider in physics bundle)
const CAPSULE_RADIUS: f32 = 0.5;
const CAPSULE_HEIGHT: f32 = 1.0;

/// Surface normal must have Y > this to count as walkable ground (~45° max slope)
const MIN_GROUND_NORMAL_Y: f32 = 0.7;

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

/// Physics components for a player entity. Kinematic — we control Position directly
/// via the character controller. Avian detects collisions but doesn't move us.
pub fn player_physics_bundle() -> impl Bundle {
    (
        Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT),
        RigidBody::Kinematic,
    )
}

/// Replicated gameplay state for a player entity.
/// Server spawns these; client receives them via lightyear replication.
pub fn player_replicated_bundle(client_id: u64) -> impl Bundle {
    (
        PlayerContext,
        crate::protocol::PlayerId(client_id),
        crate::protocol::PlayerYaw::default(),
        PlayerPitch::default(),
        PlayerEquipped::default(),
        crate::protocol::PlayerInventory::default(),
        PlayerHealth::default(),
        crate::protocol::LastDamagedBy::default(),
        crate::protocol::LastShot::default(),
        CharacterVelocity::default(),
        Position(PLAYER_SPAWN_POS),
        Rotation::default(),
    )
}

// --- Shared Movement (observer, runs on both client + server) ---

/// Handles MoveAction fire events. Input is already in world-space
/// (pre-rotated by camera yaw on the client before BEI buffers it).
pub fn shared_movement(
    trigger: On<Fire<MoveAction>>,
    mut query: Query<(&mut CharacterVelocity, Has<Interpolated>, Has<PlayerDead>)>,
) {
    let input = trigger.value;

    if let Ok((mut vel, is_interpolated, is_dead)) = query.get_mut(trigger.context) {
        if is_interpolated || is_dead { return; }
        if input == Vec2::ZERO {
            vel.0.x = 0.0;
            vel.0.z = 0.0;
            return;
        }

        let move_dir = Vec2::new(input.x, input.y).normalize_or_zero();
        vel.0.x = move_dir.x * PLAYER_MOVE_SPEED;
        vel.0.z = move_dir.y * PLAYER_MOVE_SPEED;
    }
}

/// Zeros XZ velocity every fixed tick. Fire<MoveAction> re-applies if keys are held.
pub fn clear_xz_velocity(
    mut query: Query<&mut CharacterVelocity, (With<PlayerContext>, With<Collider>, Without<Interpolated>)>,
) {
    for mut vel in query.iter_mut() {
        vel.0.x = 0.0;
        vel.0.z = 0.0;
    }
}

/// Jump: set upward velocity if grounded. Shared between client + server.
/// Does its own inline ground check (shape cast + normal filter) to avoid
/// relying on deferred commands that may not flush between rollback ticks.
pub fn shared_jump(
    trigger: On<Fire<JumpAction>>,
    mut query: Query<(&mut CharacterVelocity, &Position, Has<Interpolated>, Has<PlayerDead>)>,
    spatial_query: SpatialQuery,
) {
    let Ok((mut vel, position, is_interpolated, is_dead)) = query.get_mut(trigger.context) else {
        return;
    };
    if is_interpolated || is_dead { return; }
    if vel.0.y > 0.5 {
        return;
    }

    let capsule = Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT);
    let config = ShapeCastConfig {
        max_distance: 0.15,
        target_distance: SKIN_WIDTH,
        compute_contact_on_penetration: true,
        ignore_origin_penetration: true,
    };
    let filter = SpatialQueryFilter::from_excluded_entities([trigger.context]);

    if let Some(hit) = spatial_query.cast_shape(
        &capsule, position.0, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter,
    ) {
        if hit.normal1.y > MIN_GROUND_NORMAL_Y {
            vel.0.y = JUMP_SPEED;
        }
    }
}

// --- Kinematic Character Controller ---

/// Kinematic character controller. Runs every fixed tick on both client + server.
/// Handles gravity, ground detection via shape cast, and move-and-slide collision.
///
/// Uses ParamSet because SpatialQuery reads Position internally (for all colliders),
/// and we also need to write Position for players. We collect→compute→writeback.
/// Kinematic character controller. Runs every fixed tick on both client + server.
/// Handles gravity, ground detection via shape cast, and move-and-slide collision.
///
/// All Position-accessing params must live inside the ParamSet because SpatialQuery
/// reads Position for all colliders, and we need to write Position for players.
/// Flow: collect (p0) → shape cast (p1) → write back (p2).
pub fn character_controller(
    mut params: ParamSet<(
        Query<(Entity, &Position, &CharacterVelocity), (With<PlayerContext>, With<Collider>, Without<Interpolated>)>,
        SpatialQuery,
        Query<(&mut Position, &mut CharacterVelocity), (With<PlayerContext>, With<Collider>, Without<Interpolated>)>,
    )>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    let capsule = Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT);
    // Shorter capsule for horizontal casts — bottom raised by STEP_HEIGHT
    // to prevent scraping the ground and gives basic stair-stepping
    let h_capsule = Collider::capsule(CAPSULE_RADIUS, (CAPSULE_HEIGHT - STEP_HEIGHT * 2.0).max(0.0));

    // 1. Collect current state
    let players: Vec<(Entity, Vec3, Vec3)> = params
        .p0()
        .iter()
        .map(|(e, p, v)| (e, p.0, v.0))
        .collect();

    // 2. Compute new positions using SpatialQuery
    let spatial = params.p1();
    let mut results: Vec<(Entity, Vec3, Vec3)> = Vec::with_capacity(players.len());

    for (entity, mut pos, mut vel) in players {
        let filter = SpatialQueryFilter::from_excluded_entities([entity]);

        // Apply gravity
        vel.y -= GRAVITY * dt;

        // --- Horizontal move-and-slide ---
        let h_vel = Vec3::new(vel.x, 0.0, vel.z);
        if h_vel.length_squared() > 0.0001 {
            let h_delta = h_vel * dt;
            pos += move_and_slide(&spatial, &h_capsule, pos, h_delta, &filter);
        }

        // --- Vertical movement + ground detection ---
        if vel.y <= 0.0 {
            let fall_dist = vel.y.abs() * dt + 0.1;
            let config = ShapeCastConfig {
                max_distance: fall_dist,
                target_distance: SKIN_WIDTH,
                compute_contact_on_penetration: true,
                ignore_origin_penetration: true,
            };

            match spatial.cast_shape(
                &capsule, pos, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter,
            ) {
                Some(hit) if hit.normal1.y > MIN_GROUND_NORMAL_Y => {
                    // Hit walkable ground — snap and zero vertical velocity
                    if hit.distance > 0.0 {
                        pos.y -= hit.distance;
                    }
                    vel.y = 0.0;
                }
                _ => {
                    // Airborne or hit a wall/steep slope — keep falling
                    pos.y += vel.y * dt;
                }
            }
        } else {
            // Moving upward (jumping) — cast for ceiling
            let up_dist = vel.y * dt;
            let config = ShapeCastConfig {
                max_distance: up_dist,
                target_distance: SKIN_WIDTH,
                compute_contact_on_penetration: true,
                ignore_origin_penetration: true,
            };

            match spatial.cast_shape(
                &capsule, pos, Quat::IDENTITY, Dir3::Y, &config, &filter,
            ) {
                Some(hit) => {
                    if hit.distance > 0.0 {
                        pos.y += hit.distance;
                    }
                    vel.y = 0.0;
                }
                None => {
                    pos.y += up_dist;
                }
            }
        }

        results.push((entity, pos, vel));
    }

    // 3. Write back results
    drop(spatial);
    let mut writeback = params.p2();
    for (entity, new_pos, new_vel) in results {
        if let Ok((mut pos, mut vel)) = writeback.get_mut(entity) {
            pos.0 = new_pos;
            vel.0 = new_vel;
        }
    }
}

/// Cast the player capsule in `delta` direction. On collision, slide along the surface.
/// Returns the actual displacement to apply. Max 2 iterations (move + slide).
fn move_and_slide(
    spatial_query: &SpatialQuery,
    shape: &Collider,
    mut origin: Vec3,
    mut remaining: Vec3,
    filter: &SpatialQueryFilter,
) -> Vec3 {
    let mut total = Vec3::ZERO;

    for _ in 0..2 {
        let dist = remaining.length();
        if dist < 0.0001 {
            break;
        }

        let Ok(dir) = Dir3::new(remaining / dist) else {
            break;
        };

        let config = ShapeCastConfig {
            max_distance: dist,
            target_distance: SKIN_WIDTH,
            compute_contact_on_penetration: true,
            ignore_origin_penetration: true,
        };

        match spatial_query.cast_shape(shape, origin, Quat::IDENTITY, dir, &config, filter) {
            Some(hit) => {
                // Move up to the surface (distance already accounts for skin via target_distance)
                let step = dir.as_vec3() * hit.distance;
                total += step;
                origin += step;

                // Project remaining movement onto the surface to slide
                let leftover = dist - hit.distance;
                if leftover < 0.001 {
                    break;
                }
                let slide_vec = remaining.normalize() * leftover;
                remaining = slide_vec - hit.normal1 * slide_vec.dot(hit.normal1);
            }
            None => {
                total += remaining;
                break;
            }
        }
    }

    total
}

/// Diagnostic: log player position/velocity every 2 seconds.
pub fn log_player_state(
    query: Query<(Entity, &Position, &CharacterVelocity), (With<PlayerContext>, With<Collider>)>,
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

/// Shared observer: applies mouse look deltas to PlayerYaw/PlayerPitch.
/// Runs on both client (prediction) and server (authority) via BEI input replication.
/// This is how yaw/pitch reach the server — through the input system, not component replication.
pub fn shared_look(
    trigger: On<Fire<LookAction>>,
    mut query: Query<(&mut PlayerYaw, &mut PlayerPitch, Has<Interpolated>, Has<PlayerDead>)>,
) {
    let delta = trigger.value;
    if delta == Vec2::ZERO {
        return;
    }

    let Ok((mut yaw, mut pitch, is_interpolated, is_dead)) = query.get_mut(trigger.context) else {
        return;
    };
    if is_interpolated || is_dead { return; }

    yaw.0 += -delta.x * 0.003;
    const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.01;
    pitch.0 = (pitch.0 + -delta.y * 0.002).clamp(-PITCH_LIMIT, PITCH_LIMIT);
}

/// Shared system: syncs PlayerYaw + PlayerPitch → Rotation so lightyear replicates
/// both facing direction and pitch tilt. Runs in FixedUpdate on both client and server.
/// Remote players display correct pitch tilt via the replicated Rotation.
pub fn sync_rotation_from_yaw(
    mut query: Query<(&PlayerYaw, &PlayerPitch, &mut Rotation), (With<PlayerContext>, Without<Interpolated>)>,
) {
    for (yaw, pitch, mut rot) in query.iter_mut() {
        rot.0 = Quat::from_euler(EulerRot::YXZ, yaw.0, pitch.0, 0.0);
    }
}

// --- Client-Only Systems ---

/// Pre-rotate MoveAction's raw WASD Vec2 by camera yaw so the value sent to the
/// server is already in world-space. Runs between BEI Update and BufferClientInputs.
pub fn pre_rotate_move_input(
    player_query: Query<&PlayerYaw, With<Controlled>>,
    mut action_query: Query<
        (&ActionOf<crate::protocol::PlayerContext>, &mut ActionValue),
        With<Action<MoveAction>>,
    >,
) {
    let Ok(player_yaw) = player_query.single() else {
        return;
    };
    let yaw = player_yaw.0;

    for (_action_of, mut value) in action_query.iter_mut() {
        if let ActionValue::Axis2D(v) = *value {
            if v == Vec2::ZERO {
                continue;
            }
            let forward = Vec2::new(-yaw.sin(), -yaw.cos());
            let right = Vec2::new(yaw.cos(), -yaw.sin());
            let rotated = forward * v.y + right * v.x;
            *value = ActionValue::Axis2D(rotated);
        }
    }
}

/// Client-only: zeros LookAction when cursor is unlocked (e.g. Escape pressed).
/// Prevents mouse deltas from being sent to the server when the player isn't in control.
/// Runs in FixedPreUpdate between BEI Update and BufferClientInputs.
pub fn gate_look_on_cursor(
    cursor_state: Res<CursorState>,
    mut action_query: Query<&mut ActionValue, With<Action<crate::protocol::LookAction>>>,
) {
    if cursor_state.locked {
        return;
    }
    for mut value in action_query.iter_mut() {
        *value = ActionValue::Axis2D(Vec2::ZERO);
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
