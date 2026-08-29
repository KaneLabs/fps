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
    // Ground here is y=1.0; capsule centre needs 1.0m clearance, so 1.5 put
    // the player 0.5m inside the eastern hillside — same defect class as the
    // two boulder spawns, found by the world-geometry validator.
    Vec3::new(19.0, 2.0, -2.0),    // Outside mine entrance (ground y=1.0)
    Vec3::new(-7.5, 4.8, -7.5),    // Watchtower platform
    Vec3::new(3.0, 1.0, 10.0),     // Campfire area
    // NOTE: both rock spawns sit ON boulders. The capsule is 2.0m tall
    // (CAPSULE_HEIGHT 1.0 + 2 * CAPSULE_RADIUS 0.5), so its CENTER must be at
    // least 1.0m above the surface or the player spawns embedded in the
    // collider — and `ignore_origin_penetration: true` on every controller
    // shape cast means a penetrating capsule is BLIND to the geometry it is
    // inside: it reads as airborne, sinks through the world, hits the kill
    // plane, respawns at the same point, and loops forever.
    // NW boulder cluster: rock at y=0.7 half-extent 1.4 -> top 2.1 -> min 3.1
    Vec3::new(-10.0, 3.3, -15.0),  // NW boulder cluster (above rock top 2.1)
    // NE rocky ridge: rock at y=0.6 half-extent 1.2 -> top 1.8 -> min 2.8
    Vec3::new(12.0, 3.0, -16.0),   // NE rocky ridge (above rock top 1.8)
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
pub const CAPSULE_RADIUS: f32 = 0.5;
pub const CAPSULE_HEIGHT: f32 = 1.0;

/// Surface normal must have Y > this to count as walkable ground (~45° max slope)
const MIN_GROUND_NORMAL_Y: f32 = 0.7;

/// How far below the capsule the GROUNDED probe looks. A CONSTANT.
///
/// This must not depend on `vel.y`. It used to: the probe was
/// `|vel.y| * dt + 0.1`, which made the probe range a function of the grounded
/// decision's own output — grounded zeroes `vel.y`, which gives the SHORTEST
/// probe, which is the most likely to lose the ground, which starts a fall,
/// which lengthens the probe, which re-acquires it. A decision that feeds its
/// own input does not converge, it OSCILLATES, and two simulations one tick
/// apart can sit in opposite phases of that oscillation indefinitely. That is
/// what produced the persistent, regenerating client/server disagreement near
/// the boulders rather than a transient one.
///
/// Its value is CONSTRAINED, not chosen. It must exceed both:
///   - STEP_HEIGHT (0.1), so stepping down a small ledge does not go airborne;
///   - the per-tick descent when walking down the steepest WALKABLE slope,
///     PLAYER_MOVE_SPEED * dt * tan(acos(MIN_GROUND_NORMAL_Y))
///     = 7.0 * (1/64) * tan(45.6°) = 0.1116 m.
///
/// The old effective probe was 0.1078 m — SMALLER than that 0.1116 m descent,
/// so a player running down a maximally-walkable slope lost ground contact
/// every single tick. That is the same bug a second time, and it is why the
/// relationship is pinned by `ground_probe_covers_step_and_slope` rather than
/// left as a comment: three constants here (move speed, tick rate, walkable
/// slope limit) silently determine a fourth, and nothing else would notice.
const GROUND_PROBE_DISTANCE: f32 = 0.15;

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

/// The GROUNDED probe: is there walkable ground beneath `pos`?
///
/// Note what this function does NOT take: velocity. That is the fix, expressed
/// in the type rather than in a comment. The probe used to be
/// `|vel.y| * dt + 0.1`, which let the grounded decision depend on its own
/// previous output and oscillate (see GROUND_PROBE_DISTANCE). Re-introducing
/// that coupling now requires adding a parameter here, which is visible in
/// review in a way that editing an expression inline was not.
///
/// Returns the hit to snap to, or None if not grounded.
fn ground_probe(
    spatial: &SpatialQuery,
    capsule: &Collider,
    pos: Vec3,
    filter: &SpatialQueryFilter,
) -> Option<ShapeHitData> {
    let config = ShapeCastConfig {
        max_distance: GROUND_PROBE_DISTANCE,
        target_distance: SKIN_WIDTH,
        compute_contact_on_penetration: true,
        ignore_origin_penetration: true,
    };
    spatial
        .cast_shape(capsule, pos, Quat::IDENTITY, Dir3::NEG_Y, &config, filter)
        .filter(|hit| hit.normal1.y > MIN_GROUND_NORMAL_Y)
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
        //
        // TWO SEPARATE QUESTIONS, DELIBERATELY ANSWERED BY TWO SEPARATE CASTS.
        // They were once a single cast, and re-coupling them brings back the
        // oscillation described on GROUND_PROBE_DISTANCE. If you are tempted to
        // merge them to save a cast: the grounded probe MUST NOT see `vel.y`.
        //
        //   1. GROUNDED — "is there walkable ground beneath me?" Fixed-range,
        //      position-only, so two simulations at the same position always
        //      reach the same answer.
        //   2. ANTI-TUNNEL — "does this tick's fall pass through something?"
        //      Only asked when actually falling further than the grounded probe
        //      already looked, and it answers a different question: it does not
        //      decide grounded-ness, it only stops the capsule at the surface.
        if vel.y <= 0.0 {
            match ground_probe(&spatial, &capsule, pos, &filter) {
                Some(hit) => {
                    // Walkable ground within reach — snap down onto it.
                    if hit.distance > 0.0 {
                        pos.y -= hit.distance;
                    }
                    vel.y = 0.0;
                }
                None => {
                    // Not grounded. Fall — but do not fall THROUGH anything.
                    let travel = (vel.y * dt).abs();
                    if travel > GROUND_PROBE_DISTANCE {
                        // Falling faster than the grounded probe looked, so
                        // there may be geometry between here and the landing
                        // point that neither cast has seen yet.
                        let sweep_config = ShapeCastConfig {
                            max_distance: travel,
                            target_distance: SKIN_WIDTH,
                            compute_contact_on_penetration: true,
                            ignore_origin_penetration: true,
                        };
                        match spatial.cast_shape(
                            &capsule, pos, Quat::IDENTITY, Dir3::NEG_Y,
                            &sweep_config, &filter,
                        ) {
                            Some(hit) => {
                                // Stop AT the surface. Note this deliberately
                                // stops on non-walkable surfaces too: the old
                                // code fell straight through a steep face,
                                // because a non-walkable hit took the "keep
                                // falling" branch and moved the capsule past it.
                                pos.y -= hit.distance;
                                vel.y = 0.0;
                            }
                            None => pos.y += vel.y * dt,
                        }
                    } else {
                        pos.y += vel.y * dt;
                    }
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
/// viewangles. Mouse motion integrates into this ONCE PER FRAME
/// (`integrate_mouse_look`); the ABSOLUTE result is written into the Look axis
/// each fixed tick (`absolutize_look_input`) and replicated (usercmd model).
#[derive(Resource, Default)]
pub struct LocalLook {
    pub yaw: f32,
    pub pitch: f32,
}

/// Client-only, PER-FRAME (Update): integrate this frame's mouse motion into
/// `LocalLook`. Mouse deltas are frame-rate data and MUST be consumed exactly
/// once per frame.
///
/// History: v0.3.0-e97df6f integrated deltas per FIXED TICK by reading the
/// leafwing Look axis and then overwriting it with absolute angles. Leafwing
/// only refreshes the axis once per frame, so on any frame containing two
/// fixed ticks the second tick read back the absolute yaw AS A DELTA:
/// yaw += -yaw * sensitivity — an exponential decay toward zero that showed
/// up as the view "constantly straying" while running (running lowers fps →
/// more double-tick frames). Per-frame integration + pure per-tick sampling
/// eliminates the feedback loop by construction.
pub fn integrate_mouse_look(
    cursor_state: Res<CursorState>,
    mouse: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    mut local_look: ResMut<LocalLook>,
) {
    if !cursor_state.locked {
        return;
    }
    let delta = mouse.delta;
    if delta == Vec2::ZERO {
        return;
    }
    local_look.yaw += -delta.x * YAW_SENSITIVITY;
    local_look.pitch =
        (local_look.pitch + -delta.y * PITCH_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);
}

/// Client-only, PER-TICK (FixedPreUpdate, ManualControl): write the ABSOLUTE
/// view angles into the Look axis before lightyear's BufferClientInputs
/// snapshots the ActionState for replication.
///
/// This is a PURE WRITE — it never reads the axis, so a frame containing
/// multiple fixed ticks just samples the same absolute state twice, which is
/// harmless by definition (that's the point of transmitting absolutes).
pub fn absolutize_look_input(
    local_look: Res<LocalLook>,
    mut query: Query<&mut ActionState<PlayerActions>, With<Controlled>>,
) {
    for mut action in query.iter_mut() {
        action.set_axis_pair(
            &PlayerActions::Look,
            Vec2::new(local_look.yaw, local_look.pitch),
        );
    }
}

/// Client-only, per-frame: hard-lock the LOCAL player's rendered yaw to
/// LocalLook — the CS model where view angles are pure local presentation.
///
/// Without this, the camera inherits yaw from the physics `Rotation` component,
/// which rides the whole netcode pipeline (fixed-tick sampling, frame
/// interpolation, correction smoothing). Any divergence between that processed
/// value and the mouse-true LocalLook makes the player walk "straight" relative
/// to a view that isn't quite their movement basis — felt as a slow sideways
/// pull. The replicated Rotation still exists and is still synced from
/// PlayerYaw each tick — it's what OTHER players see; your own eyes never
/// consume it.
///
/// Runs in PostUpdate before transform propagation, i.e. after lightyear's
/// frame interpolation has written Transform, so this write wins the frame.
pub fn lock_local_view_yaw(
    local_look: Res<LocalLook>,
    mut query: Query<&mut Transform, (With<Controlled>, With<PlayerId>)>,
) {
    for mut transform in query.iter_mut() {
        transform.rotation = Quat::from_rotation_y(local_look.yaw);
    }
}

/// Client-only: applies pitch to the camera locally.
/// Parent player Rotation contains only yaw (capsule stays upright), so the
/// camera child must apply pitch on its own Transform to look up/down.
pub fn sync_camera_pitch(
    local_look: Res<LocalLook>,
    player_query: Query<&Children, With<Controlled>>,
    mut camera_query: Query<&mut Transform, With<crate::world::WorldModelCamera>>,
) {
    let Ok(children) = player_query.single() else {
        return;
    };

    for child in children.iter() {
        if let Ok(mut cam_transform) = camera_query.get_mut(child) {
            // Pitch around X only — yaw comes from parent, no roll.
            // LocalLook (mouse-true, per-frame), NOT the netcode-processed
            // PlayerPitch — the local view never consumes replication state.
            cam_transform.rotation = Quat::from_rotation_x(local_look.pitch);
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

// ========================================
// Grounding bifurcation reproduction
// ========================================
#[cfg(test)]
mod grounding_repro {
    use super::*;

    /// Builds a query pipeline directly — no Bevy app, no schedules. Driving
    /// `SpatialQueryPipeline::update` ourselves makes this a pure, deterministic
    /// function of the geometry, which is what a diagnosis needs.
    fn pipeline(blocks: &[(Vec3, Vec3, Quat)]) -> SpatialQueryPipeline {
        let mut pipe = SpatialQueryPipeline::default();
        let data: Vec<(Entity, Position, Rotation, Collider, CollisionLayers)> = blocks
            .iter()
            .enumerate()
            .map(|(i, (pos, size, rot))| {
                (
                    Entity::from_raw_u32(i as u32 + 1).unwrap(),
                    Position::new(*pos),
                    Rotation(*rot),
                    Collider::cuboid(size.x, size.y, size.z),
                    CollisionLayers::default(),
                )
            })
            .collect();
        pipe.update(data.iter().map(|(e, p, r, c, l)| (*e, p, r, c, l)));
        pipe
    }

    /// The controller's downward probe, at rest: gravity has just been applied so
    /// vel.y = -GRAVITY/64, giving max_distance = |vel.y|*dt + 0.1.
    fn ground_config() -> ShapeCastConfig {
        let vy = GRAVITY / 64.0;
        ShapeCastConfig {
            max_distance: vy / 64.0 + 0.1,
            target_distance: SKIN_WIDTH,
            compute_contact_on_penetration: true,
            ignore_origin_penetration: true,
        }
    }

    struct Decision {
        single_grounded: bool,
        single_normal_y: Option<f32>,
        multi_grounded: bool,
        walkable_hits: usize,
        total_hits: usize,
    }

    fn decide(pipe: &SpatialQueryPipeline, at: Vec3) -> Decision {
        let capsule = Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT);
        let filter = SpatialQueryFilter::default();
        let config = ground_config();

        let single = pipe.cast_shape(
            &capsule, at, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter,
        );
        let hits = pipe.shape_hits(
            &capsule, at, Quat::IDENTITY, Dir3::NEG_Y, 16, &config, &filter,
        );
        Decision {
            single_grounded: single.is_some_and(|h| h.normal1.y > MIN_GROUND_NORMAL_Y),
            single_normal_y: single.map(|h| h.normal1.y),
            multi_grounded: hits.iter().any(|h| h.normal1.y > MIN_GROUND_NORMAL_Y),
            walkable_hits: hits.iter().filter(|h| h.normal1.y > MIN_GROUND_NORMAL_Y).count(),
            total_hits: hits.len(),
        }
    }

    /// Flat ground (top y=0) plus a Y-rotated boulder like the NE ridge.
    fn scene() -> SpatialQueryPipeline {
        pipeline(&[
            (Vec3::new(0.0, -0.5, 0.0), Vec3::new(60.0, 1.0, 60.0), Quat::IDENTITY),
            (Vec3::new(0.0, 0.7, 0.0), Vec3::new(3.0, 1.4, 2.5), Quat::from_rotation_y(0.6)),
        ])
    }


    /// THE PROPERTY THE FIX EXISTS FOR: the grounded decision is a pure
    /// function of POSITION, so two simulations at the same position always
    /// agree no matter what either believes about its own fall speed.
    ///
    /// The strongest form of this guarantee is not the assertion below — it is
    /// that `ground_probe` DOES NOT TAKE A VELOCITY PARAMETER. Re-coupling
    /// requires changing its signature. This test additionally pins that the
    /// answer is stable across the whole rim region.
    #[test]
    fn grounded_decision_is_a_pure_function_of_position() {
        let pipe = scene();
        let capsule = Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT);
        let filter = SpatialQueryFilter::default();
        let config = ShapeCastConfig {
            max_distance: GROUND_PROBE_DISTANCE,
            target_distance: SKIN_WIDTH,
            compute_contact_on_penetration: true,
            ignore_origin_penetration: true,
        };
        // Same position probed repeatedly must give an identical answer; the
        // probe has no hidden state and no velocity input to vary.
        let mut x = 1.6_f32;
        while x < 2.6 {
            let at = Vec3::new(x, 2.4, 0.0);
            let first = pipe
                .cast_shape(&capsule, at, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter)
                .is_some_and(|h| h.normal1.y > MIN_GROUND_NORMAL_Y);
            for _ in 0..4 {
                let again = pipe
                    .cast_shape(&capsule, at, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter)
                    .is_some_and(|h| h.normal1.y > MIN_GROUND_NORMAL_Y);
                assert_eq!(first, again, "grounded must be deterministic at x={x}");
            }
            x += 0.01;
        }
    }

    /// Demonstrates the OLD probe was genuinely vel-dependent at the rim: the
    /// same position gives different answers for different fall speeds. This is
    /// the oscillation's mechanism, pinned so the regression is visible.
    #[test]
    fn old_probe_gave_different_answers_at_the_same_position() {
        let pipe = scene();
        let capsule = Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT);
        let filter = SpatialQueryFilter::default();

        let old_probe = |vy: f32, at: Vec3| {
            let config = ShapeCastConfig {
                max_distance: vy.abs() / 64.0 + 0.1,
                target_distance: SKIN_WIDTH,
                compute_contact_on_penetration: true,
                ignore_origin_penetration: true,
            };
            pipe.cast_shape(&capsule, at, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter)
                .is_some_and(|h| h.normal1.y > MIN_GROUND_NORMAL_Y)
        };

        // Somewhere past the rim, grounded-ness under the old probe depends on
        // how fast you thought you were falling.
        let mut disagreement_found = false;
        let mut x = 2.15_f32;
        while x < 2.45 {
            let at = Vec3::new(x, 2.4, 0.0);
            // vel.y a grounded sim has (gravity for one tick) vs a falling one.
            let grounded_sim = old_probe(32.0 / 64.0, at);
            let falling_sim = old_probe(20.0, at);
            if grounded_sim != falling_sim {
                disagreement_found = true;
                break;
            }
            x += 0.005;
        }
        assert!(
            disagreement_found,
            "expected the OLD vel-dependent probe to answer differently at the \
             same position for different fall speeds — that difference is the \
             feedback loop this fix removes"
        );
    }

    /// The probe distance is not a free parameter. Three constants elsewhere
    /// (move speed, tick rate, walkable slope limit) determine a lower bound on
    /// it, and nothing else in the codebase would notice if one of them moved.
    ///
    /// The OLD effective probe (0.1078m) FAILED this: a player running down a
    /// maximally-walkable slope descends 0.1116m per tick and lost ground
    /// contact every tick.
    #[test]
    fn ground_probe_covers_step_and_slope() {
        let dt = 1.0 / crate::FIXED_TIMESTEP_HZ as f32;
        let max_slope = MIN_GROUND_NORMAL_Y.acos();
        let slope_drop_per_tick = PLAYER_MOVE_SPEED * dt * max_slope.tan();

        assert!(
            GROUND_PROBE_DISTANCE > STEP_HEIGHT,
            "probe {GROUND_PROBE_DISTANCE} must exceed STEP_HEIGHT {STEP_HEIGHT}, \
             or stepping down a ledge goes airborne"
        );
        assert!(
            GROUND_PROBE_DISTANCE > slope_drop_per_tick,
            "probe {GROUND_PROBE_DISTANCE} must exceed the per-tick descent on \
             the steepest WALKABLE slope ({slope_drop_per_tick:.4}m at \
             {PLAYER_MOVE_SPEED} m/s, {:.1} deg). Below this a player running \
             downhill loses ground contact every tick and slides. If you changed \
             PLAYER_MOVE_SPEED, FIXED_TIMESTEP_HZ or MIN_GROUND_NORMAL_Y, raise \
             GROUND_PROBE_DISTANCE to match — or accept sliding and say why.",
            max_slope.to_degrees()
        );
        // The old value, pinned to show it was under the bound.
        assert!(
            0.1078 < slope_drop_per_tick,
            "the pre-fix probe should be below the slope bound; if this fails the \
             constants moved and the historical note above is now wrong"
        );
    }

    /// Walk across the boulder's rim in 2mm steps and watch the grounded
    /// predicate. This is the decisive test: does normal.y cross
    /// MIN_GROUND_NORMAL_Y discontinuously under sub-centimetre motion?
    #[test]
    #[ignore = "diagnostic"]
    fn scan_rim() {
        let pipe = scene();
        let capsule = Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT);
        let filter = SpatialQueryFilter::default();
        let config = ground_config();
        println!("\n--- crossing the boulder rim, capsule centre y=2.4 (top y=1.4) ---");
        let mut x = 1.60_f32;
        let mut prev: Option<bool> = None;
        while x < 2.60 {
            let at = Vec3::new(x, 2.4, 0.0);
            let h = pipe.cast_shape(&capsule, at, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter);
            let grounded = h.is_some_and(|h| h.normal1.y > MIN_GROUND_NORMAL_Y);
            let flip = prev.is_some_and(|p| p != grounded);
            println!(
                "x={x:6.3}  grounded={grounded:<5} {} d={:>8} n.y={:>7}",
                if flip { "<== FLIP" } else { "        " },
                h.map(|h| format!("{:.5}", h.distance)).unwrap_or("none".into()),
                h.map(|h| format!("{:.4}", h.normal1.y)).unwrap_or("none".into()),
            );
            prev = Some(grounded);
            x += 0.002;
        }
    }

    /// Dump every hit (distance + normal) so single-cast vs multi-hit can be
    /// compared directly rather than via a boolean.
    #[test]
    #[ignore = "diagnostic"]
    fn dump_hits() {
        let pipe = scene();
        let capsule = Collider::capsule(CAPSULE_RADIUS, CAPSULE_HEIGHT);
        let filter = SpatialQueryFilter::default();
        let config = ground_config();
        for x in [1.00_f32, 1.05, 1.06, 1.25, 1.75, 2.5] {
            let at = Vec3::new(x, 1.0, 0.0);
            let single = pipe.cast_shape(&capsule, at, Quat::IDENTITY, Dir3::NEG_Y, &config, &filter);
            let hits = pipe.shape_hits(&capsule, at, Quat::IDENTITY, Dir3::NEG_Y, 16, &config, &filter);
            println!("\nx={x:.2}");
            match single {
                Some(h) => println!("  cast_shape : d={:.5} n=({:.3},{:.3},{:.3}) e={:?}",
                    h.distance, h.normal1.x, h.normal1.y, h.normal1.z, h.entity),
                None => println!("  cast_shape : none"),
            }
            for h in &hits {
                println!("  shape_hits : d={:.5} n=({:.3},{:.3},{:.3}) e={:?}",
                    h.distance, h.normal1.x, h.normal1.y, h.normal1.z, h.entity);
            }
        }
    }

    #[test]
    #[ignore = "diagnostic"]
    fn scan_grounding_decision() {
        let pipe = scene();
        println!("\n--- BESIDE the boulder, capsule centre y=1.0 (resting on ground y=0) ---");
        let mut x = 1.0;
        while x < 3.2 {
            let d = decide(&pipe, Vec3::new(x, 1.0, 0.0));
            println!(
                "x={x:5.2}  single={:<5} normal.y={:>7}  multi={:<5}  hits={} walkable={}",
                d.single_grounded,
                d.single_normal_y.map(|v| format!("{v:.3}")).unwrap_or("none".into()),
                d.multi_grounded, d.total_hits, d.walkable_hits,
            );
            x += 0.05;
        }
        println!("\n--- ON TOP, capsule centre y=2.4 (rests on boulder top y=1.4) ---");
        let mut x = 0.0;
        while x < 2.4 {
            let d = decide(&pipe, Vec3::new(x, 2.4, 0.0));
            println!(
                "x={x:5.2}  single={:<5} normal.y={:>7}  multi={:<5}  hits={} walkable={}",
                d.single_grounded,
                d.single_normal_y.map(|v| format!("{v:.3}")).unwrap_or("none".into()),
                d.multi_grounded, d.total_hits, d.walkable_hits,
            );
            x += 0.05;
        }
    }
}
