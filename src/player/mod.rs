use std::f32::consts::FRAC_PI_2;

use avian3d::prelude::*;
use bevy::{
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use leafwing_input_manager::prelude::*;

use avian3d::prelude::Rotation;
use lightyear::prelude::{Controlled, Interpolated};

use crate::protocol::{
    CharacterVelocity, PlayerActions, PlayerDead, PlayerEquipped, PlayerHealth, PlayerId,
    PlayerPitch, PlayerYaw,
};

pub const PLAYER_MOVE_SPEED: f32 = 7.0;
/// Max camera pitch (radians). Shared clamp: the client clamps at accumulation
/// time and the server clamps again on apply (anticheat sanity bound).
pub const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.01;
/// Mouse sensitivity: radians of yaw per mouse count.
pub const YAW_SENSITIVITY: f32 = 0.003;
/// Mouse sensitivity: radians of pitch per mouse count.
pub const PITCH_SENSITIVITY: f32 = 0.002;
pub const JUMP_SPEED: f32 = 12.0;  // ~2.25m jump, ~0.75s air time
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
///
/// `ActionState<PlayerActions>` is the leafwing equivalent of the old BEI
/// `PlayerContext` marker — it's the replicated input component queried each
/// FixedUpdate by shared movement/shoot/etc systems on both ends.
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
        CharacterVelocity::default(),
        Position(PLAYER_SPAWN_POS),
        Rotation::default(),
    )
}

// --- Shared Movement (FixedUpdate, runs on both client + server) ---

/// Reads the Move dual-axis from each player's ActionState and applies it to their
/// CharacterVelocity. Input is already world-space (pre-rotated by camera yaw on
/// the client before lightyear buffers the ActionState for replication).
///
/// Runs every FixedUpdate on both client (prediction) and server (authority).
/// Leafwing's ActionState is snapshot/restored cleanly across rollback — so this
/// system can be called during replay without the rubber-banding that plagued BEI.
pub fn shared_movement_system(
    mut query: Query<
        (&ActionState<PlayerActions>, &mut CharacterVelocity, Has<Interpolated>, Has<PlayerDead>),
        With<PlayerId>,
    >,
) {
    for (action, mut vel, is_interpolated, is_dead) in query.iter_mut() {
        if is_interpolated || is_dead {
            continue;
        }

        let input = action.axis_pair(&PlayerActions::Move);

        if input == Vec2::ZERO {
            vel.0.x = 0.0;
            vel.0.z = 0.0;
            continue;
        }

        let move_dir = input.normalize_or_zero();
        vel.0.x = move_dir.x * PLAYER_MOVE_SPEED;
        vel.0.z = move_dir.y * PLAYER_MOVE_SPEED;
    }
}

/// Jump: set upward velocity if grounded. Shared between client + server.
/// Triggered by just_pressed(Jump) so a single keypress fires one jump even
/// though the key may be held across multiple ticks.
pub fn shared_jump_system(
    mut query: Query<
        (Entity, &ActionState<PlayerActions>, &mut CharacterVelocity, &Position, Has<Interpolated>, Has<PlayerDead>),
        With<PlayerId>,
    >,
    spatial_query: SpatialQuery,
) {
    for (entity, action, mut vel, position, is_interpolated, is_dead) in query.iter_mut() {
        if is_interpolated || is_dead {
            continue;
        }
        // Use `pressed` rather than `just_pressed` — `just_pressed` depends on
        // state-transition tracking which is brittle across replication/rollback.
        // The `vel.0.y > 0.5` guard below ensures we only fire once per jump
        // (after liftoff, y velocity > 0.5 until we've fallen back to ground).
        if !action.pressed(&PlayerActions::Jump) {
            continue;
        }
        if vel.0.y > 0.5 {
            continue;
        }

        let capsule = Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT);
        let config = ShapeCastConfig {
            max_distance: 0.15,
            target_distance: SKIN_WIDTH,
            compute_contact_on_penetration: true,
            ignore_origin_penetration: true,
        };
        let filter = SpatialQueryFilter::from_excluded_entities([entity]);

        if let Some(hit) = spatial_query.cast_shape(
            &capsule, position.0, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter,
        ) {
            if hit.normal1.y > MIN_GROUND_NORMAL_Y {
                vel.0.y = JUMP_SPEED;
            }
        }
    }
}

/// Applies the Look dual-axis to yaw/pitch. The axis carries ABSOLUTE angles
/// (x = yaw, y = pitch) — the CS/Valorant "usercmd" model: the client integrates
/// mouse deltas locally (see `absolutize_look_input`) and transmits the resulting
/// view angles as ground truth every tick.
///
/// Why absolute instead of deltas: yaw-from-deltas is integrated state, so one
/// lost input causes a PERMANENT client/server aim offset (deadly for hit reg).
/// Absolute angles are stateless — a lost packet costs nothing because the next
/// one carries the full truth. This also makes no-rollback-on-look
/// unconditionally safe (protocol.rs).
///
/// Runs on both client (prediction) and server (authority). The pitch clamp
/// doubles as the server-side sanity bound on client-provided angles.
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

        let angles = action.axis_pair(&PlayerActions::Look);
        // Exactly (0,0) means "no look data this tick" (e.g. input not yet
        // received server-side → leafwing default). Keep previous angles rather
        // than snapping to origin. A real (0,0) after any mouse movement is
        // float-impossible in practice; at spawn it matches the default anyway.
        if angles == Vec2::ZERO {
            continue;
        }

        yaw.0 = angles.x;
        pitch.0 = angles.y.clamp(-PITCH_LIMIT, PITCH_LIMIT);
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
        Query<(Entity, &Position, &CharacterVelocity), (With<PlayerId>, With<Collider>, Without<Interpolated>)>,
        SpatialQuery,
        Query<(&mut Position, &mut CharacterVelocity), (With<PlayerId>, With<Collider>, Without<Interpolated>)>,
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

    // 3. Write back results (NLL ends the `spatial` borrow at its last use above)
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
    query: Query<(Entity, &Position, &CharacterVelocity), (With<PlayerId>, With<Collider>)>,
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
pub fn sync_rotation_from_yaw(
    mut query: Query<(&PlayerYaw, &mut Rotation), (With<PlayerId>, Without<Interpolated>)>,
) {
    // Capsule body stays upright — only yaw rotates the rigid body.
    // Pitch is applied to the camera child locally (see sync_camera_pitch),
    // so the capsule collider never tilts. Tilting the capsule breaks
    // grounded raycasts and ground collision.
    for (yaw, mut rot) in query.iter_mut() {
        rot.0 = Quat::from_rotation_y(yaw.0);
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
    local_look: Res<LocalLook>,
    mut query: Query<&mut ActionState<PlayerActions>, With<Controlled>>,
) {
    let Ok(mut action) = query.single_mut() else {
        return;
    };
    let raw = action.axis_pair(&PlayerActions::Move);
    if raw == Vec2::ZERO {
        return;
    }
    // LocalLook is the freshest yaw (this tick's mouse already integrated by
    // absolutize_look_input, chained before this system) — not the predicted
    // component, which is one tick stale.
    let yaw = local_look.yaw;
    // WASD yields: x = strafe (+right), y = forward (+up on screen = +W).
    // World forward at yaw=0 is -Z, world right at yaw=0 is +X.
    let forward = Vec2::new(-yaw.sin(), -yaw.cos());
    let right = Vec2::new(yaw.cos(), -yaw.sin());
    let rotated = forward * raw.y + right * raw.x;
    action.set_axis_pair(&PlayerActions::Move, rotated);
}

/// Client-side ground truth for view angles — the equivalent of CS's local
/// viewangles. Mouse deltas integrate into this every tick; the ABSOLUTE result
/// is what gets written into the Look axis and replicated (usercmd model).
#[derive(Resource, Default)]
pub struct LocalLook {
    pub yaw: f32,
    pub pitch: f32,
}

/// Client-only: converts the Look axis from raw mouse deltas (populated by
/// leafwing's Update set) to ABSOLUTE view angles before lightyear's
/// BufferClientInputs snapshots the ActionState for replication.
///
/// - Cursor locked: integrate deltas into `LocalLook` (with sensitivity +
///   pitch clamp), then write the absolute angles into the axis.
/// - Cursor unlocked: don't integrate (mouse shouldn't move the camera), but
///   STILL write the current absolute angles — the server keeps receiving
///   ground truth every tick, so there's nothing to drift.
///
/// Runs in FixedPreUpdate in `InputManagerSystem::ManualControl`, chained
/// before `pre_rotate_move_input` (which rotates Move by this yaw).
pub fn absolutize_look_input(
    cursor_state: Res<CursorState>,
    mut local_look: ResMut<LocalLook>,
    mut query: Query<&mut ActionState<PlayerActions>, With<Controlled>>,
) {
    for mut action in query.iter_mut() {
        if cursor_state.locked {
            let delta = action.axis_pair(&PlayerActions::Look);
            local_look.yaw += -delta.x * YAW_SENSITIVITY;
            local_look.pitch =
                (local_look.pitch + -delta.y * PITCH_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }
        action.set_axis_pair(
            &PlayerActions::Look,
            Vec2::new(local_look.yaw, local_look.pitch),
        );
    }
}

/// Client-only: applies pitch to the camera locally.
/// Parent player Rotation contains only yaw (capsule stays upright), so the
/// camera child must apply pitch on its own Transform to look up/down.
pub fn sync_camera_pitch(
    player_query: Query<(&PlayerPitch, &Children), With<Controlled>>,
    mut camera_query: Query<&mut Transform, With<crate::world::WorldModelCamera>>,
) {
    let Ok((pitch, children)) = player_query.single() else {
        return;
    };

    for child in children.iter() {
        if let Ok(mut cam_transform) = camera_query.get_mut(child) {
            // Pitch around X only — yaw comes from parent, no roll.
            cam_transform.rotation = Quat::from_rotation_x(pitch.0);
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
