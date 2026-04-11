use avian3d::prelude::*;
use bevy::camera::visibility::RenderLayers;
use bevy::color::palettes::tailwind;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::player::VIEW_MODEL_RENDER_LAYER;
use crate::protocol::{DropAction, InteractAction, JabAction, PrimaryAction, PlayerEquipped, PlayerHealth, PlayerPitch, PlayerYaw};

#[derive(Debug, Component)]
pub struct WorldModelCamera;

pub const DEFAULT_RENDER_LAYER: usize = 0;

/// Component for items that can be equipped by the player.
/// Replicated from server to all clients.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Equippable {
    pub name: String,
    pub model_path: String,
    pub interaction_distance: f32,
    pub scale: f32,
    /// Euler rotation [x, y, z] in radians for the model's native orientation.
    /// Applied to both FPS view model and third-person remote view.
    pub model_rotation: [f32; 3],
    /// Muzzle offset in camera-local space (where the barrel tip is).
    /// For guns this is where tracers originate. None for non-guns.
    pub muzzle_offset: Option<[f32; 3]>,
}

/// Component for the currently equipped view model (client-only).
#[derive(Component)]
pub struct EquippedItem {
    pub name: String,
}

/// Marker for bullet tracer meshes — despawns after a short lifetime.
#[derive(Component)]
pub struct BulletTracer {
    pub spawn_time: f32,
    pub lifetime: f32,
}

/// Event fired when a shot happens — client uses this to spawn visual tracer.
#[derive(Event)]
pub struct ShotFired {
    pub muzzle: Vec3,
    pub hit_point: Vec3,
}

/// Client-only observer: spawns a red tracer mesh when a shot is fired.
pub fn spawn_tracer(
    trigger: On<ShotFired>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
) {
    let shot = trigger.event();
    let diff = shot.hit_point - shot.muzzle;
    let length = diff.length();
    let dir = diff / length;
    let midpoint = shot.muzzle + dir * (length / 2.0);

    // Cylinder extends along local Y — rotate so Y aligns with shot direction
    let rotation = Quat::from_rotation_arc(Vec3::Y, dir);

    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(0.01, length))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(1.0, 0.1, 0.1),
            emissive: bevy::color::LinearRgba::new(5.0, 0.2, 0.2, 1.0),
            unlit: true,
            ..default()
        })),
        Transform::from_translation(midpoint).with_rotation(rotation),
        BulletTracer {
            spawn_time: time.elapsed_secs(),
            lifetime: 0.08,
        },
    ));
}

/// Client-only: spawns tracers for remote players when their LastShot changes.
pub fn remote_shot_tracers(
    query: Query<&crate::protocol::LastShot, (Changed<crate::protocol::LastShot>, With<Interpolated>)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
) {
    for shot in query.iter() {
        if shot.tick == 0 { continue; } // default, no shot yet
        // Spawn tracer
        let diff = shot.hit_point - shot.muzzle;
        let length = diff.length();
        if length < 0.01 { continue; }
        let dir = diff / length;
        let midpoint = shot.muzzle + dir * (length / 2.0);
        let rotation = Quat::from_rotation_arc(Vec3::Y, dir);

        commands.spawn((
            Mesh3d(meshes.add(Cylinder::new(0.01, length))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(1.0, 0.1, 0.1),
                emissive: bevy::color::LinearRgba::new(5.0, 0.2, 0.2, 1.0),
                unlit: true,
                ..default()
            })),
            Transform::from_translation(midpoint).with_rotation(rotation),
            BulletTracer {
                spawn_time: time.elapsed_secs(),
                lifetime: 0.08,
            },
        ));
    }
}

/// Client-only: despawns tracers after their lifetime expires.
pub fn cleanup_tracers(
    query: Query<(Entity, &BulletTracer)>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    for (entity, tracer) in query.iter() {
        if now - tracer.spawn_time > tracer.lifetime {
            commands.entity(entity).despawn();
        }
    }
}

// ========================================
// Jab (melee) system
// ========================================

const JAB_DAMAGE: i32 = 15;
const JAB_RANGE: f32 = 2.5;
const JAB_COOLDOWN: f32 = 0.4;
const JAB_DURATION: f32 = 0.3;

/// Marker for the left hand mesh used for jab animation.
#[derive(Component)]
pub struct LeftHand;

/// Tracks the jab animation state. Added to the LeftHand entity when jabbing.
#[derive(Component)]
pub struct JabAnimation {
    pub start_time: f32,
}

/// Shared observer: jab melee attack — short range punch, server applies damage.
pub fn shared_jab(
    trigger: On<Fire<JabAction>>,
    player_query: Query<(&Position, &PlayerYaw, &PlayerPitch, &crate::protocol::PlayerId, Has<Predicted>, Has<Interpolated>)>,
    mut health_query: Query<(Entity, &mut PlayerHealth, &Position, Option<&mut crate::protocol::LastDamagedBy>)>,
    spatial_query: SpatialQuery,
    mut commands: Commands,
    mut last_jab: Local<f32>,
    time: Res<Time>,
) {
    let Ok((player_pos, yaw, pitch, attacker_id, is_predicted, is_interpolated)) = player_query.get(trigger.context) else {
        return;
    };
    if is_interpolated { return; }

    let current = time.elapsed_secs();
    if current - *last_jab < JAB_COOLDOWN {
        return;
    }
    *last_jab = current;

    let eye_pos = player_pos.0 + Vec3::Y * 0.8;
    let ray_dir = Quat::from_euler(EulerRot::YXZ, yaw.0, pitch.0, 0.0) * Vec3::NEG_Z;
    let filter = SpatialQueryFilter::from_excluded_entities([trigger.context]);

    info!(
        "[JAB] Punch! pos={:?} dir={:?} predicted={}",
        eye_pos, ray_dir, is_predicted
    );

    if let Some(hit) = spatial_query.cast_ray(
        eye_pos,
        Dir3::new(ray_dir).unwrap_or(Dir3::NEG_Z),
        JAB_RANGE,
        true,
        &filter,
    ) {
        info!("[JAB] Hit entity {:?} at distance {:.1}", hit.entity, hit.distance);
        if !is_predicted {
            if let Ok((_entity, mut health, _pos, last_damaged)) = health_query.get_mut(hit.entity) {
                health.0 -= JAB_DAMAGE;
                if let Some(mut last) = last_damaged {
                    last.0 = attacker_id.0;
                }
                info!("[JAB] {} damage applied, health now: {}", JAB_DAMAGE, health.0);
            } else {
                info!("[JAB] Hit entity {:?} but it has no PlayerHealth", hit.entity);
            }
        }
    } else {
        info!("[JAB] Miss — no hit within range {}", JAB_RANGE);
    }

    // Trigger animation event (client picks this up)
    commands.trigger(JabFired);
}

/// Event for client-side jab animation.
#[derive(Event)]
pub struct JabFired;

/// Client-only observer: starts the jab animation on the left hand.
pub fn start_jab_animation(
    _trigger: On<JabFired>,
    hand_query: Query<Entity, With<LeftHand>>,
    mut commands: Commands,
    time: Res<Time>,
) {
    let Ok(hand) = hand_query.single() else { return; };
    // Insert/overwrite animation component to restart
    commands.entity(hand).insert(JabAnimation {
        start_time: time.elapsed_secs(),
    });
}

/// Client-only: animates the left hand during a jab.
/// Slides in from off-screen left, punches forward, retracts.
pub fn animate_jab(
    mut hand_query: Query<(&mut Transform, &JabAnimation, Entity), With<LeftHand>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let Ok((mut transform, anim, entity)) = hand_query.single_mut() else { return; };

    let elapsed = time.elapsed_secs() - anim.start_time;
    let t = (elapsed / JAB_DURATION).clamp(0.0, 1.0);

    if t >= 1.0 {
        // Animation done — return to rest position (off-screen)
        transform.translation = Vec3::new(-0.8, -0.3, -0.2);
        commands.entity(entity).remove::<JabAnimation>();
        return;
    }

    // Three-phase animation using smoothstep:
    // 0.0–0.3: slide in from left
    // 0.3–0.6: punch forward
    // 0.6–1.0: retract back out
    let (x, y, z) = if t < 0.3 {
        // Slide in: (-0.8, -0.3, -0.2) → (-0.25, -0.15, -0.3)
        let p = smoothstep(t / 0.3);
        lerp3((-0.8, -0.3, -0.2), (-0.25, -0.15, -0.3), p)
    } else if t < 0.6 {
        // Punch forward: (-0.25, -0.15, -0.3) → (-0.15, -0.1, -0.7)
        let p = smoothstep((t - 0.3) / 0.3);
        lerp3((-0.25, -0.15, -0.3), (-0.15, -0.1, -0.7), p)
    } else {
        // Retract: (-0.15, -0.1, -0.7) → (-0.8, -0.3, -0.2)
        let p = smoothstep((t - 0.6) / 0.4);
        lerp3((-0.15, -0.1, -0.7), (-0.8, -0.3, -0.2), p)
    };

    transform.translation = Vec3::new(x, y, z);
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp3(a: (f32, f32, f32), b: (f32, f32, f32), t: f32) -> (f32, f32, f32) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

/// Component for objects that can be interacted with using tools.
/// Replicated from server to all clients.
///
/// Uses `mine_start_secs` (absolute game time) instead of accumulating progress.
/// Progress is computed as `current_time - mine_start_secs`, which is idempotent
/// and rollback-safe — replaying the same ticks gives the same result.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Interactable {
    pub required_tool: Option<String>,
    pub interaction_distance: f32,
    pub interaction_time: f32,
    pub model_path: String,
    pub scale: f32,
    /// Absolute game time (elapsed_secs) when mining started. None if not being mined.
    pub mine_start_secs: Option<f32>,
    /// Last game time the mine action fired — used to detect interruption.
    pub last_mine_secs: Option<f32>,
}

impl Default for Interactable {
    fn default() -> Self {
        Self {
            required_tool: None,
            interaction_distance: 2.0,
            interaction_time: 1.0,
            model_path: String::new(),
            scale: 1.0,
            mine_start_secs: None,
            last_mine_secs: None,
        }
    }
}

impl Interactable {
    /// Compute current mining progress (0.0 to interaction_time).
    pub fn progress(&self, current_secs: f32) -> f32 {
        match self.mine_start_secs {
            Some(start) => (current_secs - start).min(self.interaction_time),
            None => 0.0,
        }
    }
}

/// Networked door state — replicated from server to all clients.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DoorState {
    pub open: bool,
}

const DOOR_INTERACT_DISTANCE: f32 = 4.0;

/// Server-only: spawns physics colliders for all static world geometry.
/// No meshes, materials, or render layers — headless server doesn't render.
///
/// MAP: Abandoned cabin compound in post-apocalyptic Colorado wilderness, 2031.
/// Layout (~80x80m playable area):
///   - Central clearing with main cabin + porch
///   - Equipment shed to the west
///   - Mine entrance carved into eastern hillside
///   - Scattered supply crates, logs, rocky outcrops
///   - Uneven terrain with elevation changes
///   - Pine tree trunks throughout the perimeter
pub fn spawn_world_physics(mut commands: Commands) {
    // Helper for static collider spawning
    let sc = |commands: &mut Commands, pos: Vec3, size: Vec3, friction: f32| {
        commands.spawn((
            Transform::from_translation(pos),
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            Friction::new(friction),
        ));
    };
    let sc_rot = |commands: &mut Commands, pos: Vec3, rot: Quat, size: Vec3, friction: f32| {
        commands.spawn((
            Transform::from_translation(pos).with_rotation(rot),
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            Friction::new(friction),
        ));
    };

    // ========================================
    // TERRAIN — multi-level ground with rocky terrain
    // ========================================

    // Main ground plane (slightly below 0 so terrain sits on top)
    sc(&mut commands, Vec3::new(0.0, -0.05, -20.0), Vec3::new(120.0, 0.1, 120.0), 0.5);

    // Dirt clearing around cabin (slightly raised, packed earth)
    sc(&mut commands, Vec3::new(0.0, 0.05, 0.0), Vec3::new(20.0, 0.1, 16.0), 0.4);

    // Eastern hillside (stepped terrain rising toward mine)
    // Ground level — full width but stops before mine tunnel entrance
    sc(&mut commands, Vec3::new(18.0, 0.5, -8.0), Vec3::new(12.0, 1.0, 20.0), 0.6);
    // Mid-level — split to leave gap for mine entrance (tunnel is x=20.5-23.5, z=-2 to -10)
    sc(&mut commands, Vec3::new(24.0, 1.5, -14.0), Vec3::new(8.0, 3.0, 6.0), 0.6);  // behind mine
    sc(&mut commands, Vec3::new(27.0, 1.5, -4.0), Vec3::new(4.0, 3.0, 10.0), 0.6);  // right of mine
    // High ridge — far back
    sc(&mut commands, Vec3::new(29.0, 3.0, -8.0), Vec3::new(6.0, 6.0, 16.0), 0.6);

    // Western ridge (gentle slope)
    sc(&mut commands, Vec3::new(-20.0, 0.3, -10.0), Vec3::new(10.0, 0.6, 24.0), 0.5);
    sc(&mut commands, Vec3::new(-26.0, 0.8, -10.0), Vec3::new(6.0, 1.6, 20.0), 0.5);

    // Northern rocky slope
    sc(&mut commands, Vec3::new(0.0, 0.4, -28.0), Vec3::new(30.0, 0.8, 10.0), 0.6);
    sc(&mut commands, Vec3::new(0.0, 1.2, -35.0), Vec3::new(25.0, 2.4, 8.0), 0.6);

    // Southern approach path (trail from the south)
    sc(&mut commands, Vec3::new(0.0, 0.02, 14.0), Vec3::new(4.0, 0.04, 12.0), 0.3);

    // ========================================
    // MAIN CABIN — log cabin, 8x6m, with porch
    // ========================================

    // Cabin floor (raised wooden platform)
    sc(&mut commands, Vec3::new(0.0, 0.3, 0.0), Vec3::new(8.0, 0.2, 6.0), 0.3);

    // Cabin walls — west
    sc(&mut commands, Vec3::new(-4.0, 1.7, 0.0), Vec3::new(0.4, 2.8, 6.0), 0.2);
    // Cabin walls — east
    sc(&mut commands, Vec3::new(4.0, 1.7, 0.0), Vec3::new(0.4, 2.8, 6.0), 0.2);
    // Cabin walls — north (solid back wall)
    sc_rot(&mut commands, Vec3::new(0.0, 1.7, -3.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(0.4, 2.8, 8.0), 0.2);
    // Cabin walls — south left (doorway gap 2.5m wide)
    sc_rot(&mut commands, Vec3::new(-2.75, 1.7, 3.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(0.4, 2.8, 2.5), 0.2);
    // Cabin walls — south right
    sc_rot(&mut commands, Vec3::new(2.75, 1.7, 3.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(0.4, 2.8, 2.5), 0.2);

    // Cabin roof (angled planks — simplified as flat slab)
    sc(&mut commands, Vec3::new(0.0, 3.3, 0.0), Vec3::new(9.0, 0.2, 7.0), 0.2);

    // Front porch (extends south from cabin door)
    sc(&mut commands, Vec3::new(0.0, 0.2, 5.5), Vec3::new(8.0, 0.15, 3.0), 0.3);

    // Porch railing — left
    sc(&mut commands, Vec3::new(-3.9, 0.7, 5.5), Vec3::new(0.2, 0.8, 3.0), 0.2);
    // Porch railing — right
    sc(&mut commands, Vec3::new(3.9, 0.7, 5.5), Vec3::new(0.2, 0.8, 3.0), 0.2);
    // Porch railing — front
    sc_rot(&mut commands, Vec3::new(0.0, 0.7, 7.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(0.2, 0.8, 8.0), 0.2);

    // Porch steps (2 steps down to ground)
    sc(&mut commands, Vec3::new(0.0, 0.12, 7.5), Vec3::new(2.0, 0.12, 0.6), 0.3);
    sc(&mut commands, Vec3::new(0.0, 0.06, 8.0), Vec3::new(2.0, 0.06, 0.6), 0.3);

    // Table inside cabin
    sc(&mut commands, Vec3::new(0.0, 0.4, -1.0), Vec3::new(2.0, 0.8, 1.2), 0.2);

    // Fireplace / hearth on north wall (stone block)
    sc(&mut commands, Vec3::new(0.0, 0.5, -2.5), Vec3::new(2.0, 1.0, 1.0), 0.4);
    // Chimney above fireplace
    sc(&mut commands, Vec3::new(0.0, 3.0, -2.8), Vec3::new(1.2, 3.0, 1.2), 0.4);

    // ========================================
    // EQUIPMENT SHED — west of cabin, smaller structure
    // ========================================

    // Shed floor
    sc(&mut commands, Vec3::new(-14.0, 0.15, 2.0), Vec3::new(5.0, 0.15, 4.0), 0.3);

    // Shed walls — west
    sc(&mut commands, Vec3::new(-16.5, 1.2, 2.0), Vec3::new(0.3, 2.4, 4.0), 0.2);
    // Shed walls — east (open side — just posts)
    sc(&mut commands, Vec3::new(-11.5, 1.2, 4.0), Vec3::new(0.3, 2.4, 0.3), 0.2);
    sc(&mut commands, Vec3::new(-11.5, 1.2, 0.0), Vec3::new(0.3, 2.4, 0.3), 0.2);
    // Shed walls — north
    sc_rot(&mut commands, Vec3::new(-14.0, 1.2, 0.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(0.3, 2.4, 5.0), 0.2);
    // Shed walls — south (with gap)
    sc_rot(&mut commands, Vec3::new(-15.0, 1.2, 4.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(0.3, 2.4, 2.0), 0.2);

    // Shed roof (corrugated metal look — flat collider)
    sc(&mut commands, Vec3::new(-14.0, 2.5, 2.0), Vec3::new(6.0, 0.1, 5.0), 0.2);

    // Workbench inside shed
    sc(&mut commands, Vec3::new(-15.0, 0.4, 1.5), Vec3::new(2.5, 0.8, 0.8), 0.2);

    // ========================================
    // MINE ENTRANCE — carved into eastern hillside
    // ========================================

    // Mine tunnel floor (descending slightly into the hill)
    sc(&mut commands, Vec3::new(22.0, 0.8, -6.0), Vec3::new(3.0, 0.1, 8.0), 0.4);

    // Mine tunnel left wall
    sc(&mut commands, Vec3::new(20.5, 2.0, -6.0), Vec3::new(0.4, 2.4, 8.0), 0.3);
    // Mine tunnel right wall
    sc(&mut commands, Vec3::new(23.5, 2.0, -6.0), Vec3::new(0.4, 2.4, 8.0), 0.3);
    // Mine tunnel ceiling
    sc(&mut commands, Vec3::new(22.0, 3.2, -6.0), Vec3::new(3.0, 0.3, 8.0), 0.3);

    // Mine support beams (timber frames at intervals)
    for z_off in [-3.0, -6.0, -9.0] {
        // Left post
        sc(&mut commands, Vec3::new(20.8, 1.8, z_off), Vec3::new(0.25, 2.0, 0.25), 0.2);
        // Right post
        sc(&mut commands, Vec3::new(23.2, 1.8, z_off), Vec3::new(0.25, 2.0, 0.25), 0.2);
        // Cross beam
        sc(&mut commands, Vec3::new(22.0, 3.0, z_off), Vec3::new(2.8, 0.25, 0.25), 0.2);
    }

    // Mine entrance overhang (rock face)
    sc(&mut commands, Vec3::new(22.0, 3.5, -2.0), Vec3::new(5.0, 1.0, 2.0), 0.5);

    // ========================================
    // SUPPLY CRATES & BARRELS — scattered around compound
    // ========================================

    // Crate stack near shed
    sc(&mut commands, Vec3::new(-12.0, 0.4, 4.0), Vec3::new(1.0, 0.8, 1.0), 0.3);
    sc(&mut commands, Vec3::new(-12.0, 1.0, 4.0), Vec3::new(0.8, 0.4, 0.8), 0.3);
    sc(&mut commands, Vec3::new(-11.0, 0.3, 3.5), Vec3::new(0.6, 0.6, 0.6), 0.3);

    // Crates near cabin porch
    sc(&mut commands, Vec3::new(5.5, 0.35, 6.0), Vec3::new(1.2, 0.7, 0.8), 0.3);
    sc(&mut commands, Vec3::new(6.5, 0.25, 5.5), Vec3::new(0.5, 0.5, 0.5), 0.3);

    // Barrel cluster south of cabin
    sc(&mut commands, Vec3::new(-3.0, 0.5, 8.0), Vec3::new(0.7, 1.0, 0.7), 0.3);
    sc(&mut commands, Vec3::new(-2.0, 0.5, 8.5), Vec3::new(0.7, 1.0, 0.7), 0.3);
    sc(&mut commands, Vec3::new(-2.5, 0.5, 9.2), Vec3::new(0.7, 1.0, 0.7), 0.3);

    // Crate near mine entrance
    sc(&mut commands, Vec3::new(19.5, 1.2, -3.0), Vec3::new(1.0, 0.8, 1.0), 0.3);

    // ========================================
    // ROCKY OUTCROPS & BOULDERS
    // ========================================

    // Large boulder cluster — northwest
    sc(&mut commands, Vec3::new(-10.0, 0.7, -15.0), Vec3::new(3.0, 1.4, 2.5), 0.7);
    sc(&mut commands, Vec3::new(-8.5, 0.4, -13.5), Vec3::new(1.8, 0.8, 1.5), 0.7);
    sc_rot(&mut commands, Vec3::new(-11.5, 0.5, -14.0), Quat::from_rotation_y(0.4), Vec3::new(2.0, 1.0, 1.5), 0.7);

    // Rocky ridge — northeast (natural cover)
    sc_rot(&mut commands, Vec3::new(12.0, 0.6, -16.0), Quat::from_rotation_y(0.3), Vec3::new(4.0, 1.2, 1.5), 0.7);
    sc(&mut commands, Vec3::new(14.0, 0.4, -14.0), Vec3::new(2.0, 0.8, 2.0), 0.7);
    sc_rot(&mut commands, Vec3::new(10.0, 0.3, -18.0), Quat::from_rotation_y(-0.2), Vec3::new(2.5, 0.6, 1.5), 0.7);

    // Scattered mid-field boulders (cover points)
    sc_rot(&mut commands, Vec3::new(7.0, 0.45, -5.0), Quat::from_rotation_y(0.7), Vec3::new(1.8, 0.9, 1.2), 0.7);
    sc(&mut commands, Vec3::new(-6.0, 0.35, -8.0), Vec3::new(1.5, 0.7, 1.5), 0.7);
    sc_rot(&mut commands, Vec3::new(3.0, 0.3, -20.0), Quat::from_rotation_y(1.1), Vec3::new(2.0, 0.6, 1.0), 0.7);

    // ========================================
    // FALLEN LOGS (natural cover & obstacles)
    // ========================================

    sc_rot(&mut commands, Vec3::new(-5.0, 0.25, -12.0), Quat::from_rotation_y(0.6), Vec3::new(0.4, 0.4, 5.0), 0.4);
    sc_rot(&mut commands, Vec3::new(8.0, 0.3, -22.0), Quat::from_rotation_y(-0.8), Vec3::new(0.5, 0.5, 6.0), 0.4);
    sc_rot(&mut commands, Vec3::new(-8.0, 0.2, 5.0), Quat::from_rotation_y(1.2), Vec3::new(0.35, 0.35, 4.0), 0.4);

    // ========================================
    // PINE TREE TRUNKS (collision cylinders approximated as cuboids)
    // ========================================

    let tree_positions = [
        Vec3::new(-18.0, 2.0, -18.0), Vec3::new(-15.0, 2.0, -22.0),
        Vec3::new(-20.0, 2.0, -5.0),  Vec3::new(-22.0, 2.0, 5.0),
        Vec3::new(-17.0, 2.0, 10.0),  Vec3::new(-12.0, 2.0, -20.0),
        Vec3::new(15.0, 2.0, -24.0),  Vec3::new(18.0, 2.0, -20.0),
        Vec3::new(10.0, 2.0, 8.0),    Vec3::new(14.0, 2.0, 5.0),
        Vec3::new(-5.0, 2.0, -25.0),  Vec3::new(5.0, 2.0, -28.0),
        Vec3::new(-25.0, 2.0, -15.0), Vec3::new(20.0, 2.0, 3.0),
        Vec3::new(-8.0, 2.0, 12.0),   Vec3::new(8.0, 2.0, 14.0),
        Vec3::new(0.0, 2.0, -32.0),   Vec3::new(-14.0, 2.0, -28.0),
    ];

    for pos in tree_positions {
        sc(&mut commands, pos, Vec3::new(0.6, 4.0, 0.6), 0.3);
    }

    // ========================================
    // WATCHTOWER — elevated platform NW of cabin
    // ========================================

    // Four posts
    for (x, z) in [(-9.0, -6.0), (-9.0, -9.0), (-6.0, -6.0), (-6.0, -9.0)] {
        sc(&mut commands, Vec3::new(x, 2.0, z), Vec3::new(0.3, 4.0, 0.3), 0.3);
    }
    // Platform
    sc(&mut commands, Vec3::new(-7.5, 3.8, -7.5), Vec3::new(4.0, 0.2, 4.0), 0.3);
    // Ladder (angled plank)
    sc_rot(&mut commands, Vec3::new(-5.5, 1.9, -7.5), Quat::from_rotation_z(0.5), Vec3::new(0.5, 0.15, 1.0), 0.4);

    // Half-walls on watchtower (cover)
    sc(&mut commands, Vec3::new(-9.2, 4.4, -7.5), Vec3::new(0.15, 1.0, 4.0), 0.2);
    sc(&mut commands, Vec3::new(-5.8, 4.4, -7.5), Vec3::new(0.15, 1.0, 4.0), 0.2);
    sc_rot(&mut commands, Vec3::new(-7.5, 4.4, -9.2), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(0.15, 1.0, 4.0), 0.2);

    // ========================================
    // OLD PICKUP TRUCK (rusted, south-east of cabin)
    // ========================================

    // Truck body
    sc_rot(&mut commands, Vec3::new(10.0, 0.6, 3.0), Quat::from_rotation_y(0.3), Vec3::new(2.5, 1.2, 5.0), 0.3);
    // Truck cab (raised section)
    sc_rot(&mut commands, Vec3::new(10.0, 1.5, 1.5), Quat::from_rotation_y(0.3), Vec3::new(2.3, 1.0, 2.5), 0.3);

    // ========================================
    // CAMPFIRE RING — south of cabin (social area)
    // ========================================

    // Stone ring (8 small blocks in a circle)
    let ring_center = Vec3::new(3.0, 0.0, 10.0);
    for i in 0..8 {
        let angle = i as f32 * std::f32::consts::TAU / 8.0;
        let r = 1.2;
        let x = ring_center.x + angle.cos() * r;
        let z = ring_center.z + angle.sin() * r;
        sc(&mut commands, Vec3::new(x, 0.15, z), Vec3::new(0.4, 0.3, 0.4), 0.7);
    }

    // Log seats around campfire
    sc_rot(&mut commands, Vec3::new(1.0, 0.2, 10.0), Quat::from_rotation_y(0.0), Vec3::new(0.3, 0.3, 1.8), 0.4);
    sc_rot(&mut commands, Vec3::new(5.0, 0.2, 10.0), Quat::from_rotation_y(0.0), Vec3::new(0.3, 0.3, 1.8), 0.4);
    sc_rot(&mut commands, Vec3::new(3.0, 0.2, 12.0), Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), Vec3::new(0.3, 0.3, 1.8), 0.4);

    info!("Server: spawned Colorado wilderness compound physics colliders");
}

/// Client-only: spawns static world geometry with rendering + physics.
/// Interactive objects (door, pickaxe, ore) are server-spawned replicated entities.
///
/// MAP: Abandoned cabin compound — Colorado wilderness, 2031.
pub fn spawn_world_model(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // ========================================
    // MATERIAL PALETTE — post-apocalyptic Colorado
    // ========================================
    let dirt = materials.add(StandardMaterial {
        base_color: Color::srgb(0.38, 0.30, 0.20),
        perceptual_roughness: 0.95,
        ..default()
    });
    let dirt_light = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.36, 0.25),
        perceptual_roughness: 0.9,
        ..default()
    });
    let grass = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.35, 0.15),
        perceptual_roughness: 0.9,
        ..default()
    });
    let dead_grass = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.40, 0.22),
        perceptual_roughness: 0.9,
        ..default()
    });
    let log_wood = materials.add(StandardMaterial {
        base_color: Color::srgb(0.35, 0.22, 0.12),
        perceptual_roughness: 0.85,
        ..default()
    });
    let plank_wood = materials.add(StandardMaterial {
        base_color: Color::srgb(0.50, 0.35, 0.20),
        perceptual_roughness: 0.8,
        ..default()
    });
    let aged_wood = materials.add(StandardMaterial {
        base_color: Color::srgb(0.40, 0.30, 0.18),
        perceptual_roughness: 0.85,
        ..default()
    });
    let stone_gray = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.43, 0.40),
        perceptual_roughness: 0.9,
        ..default()
    });
    let stone_dark = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.28, 0.26),
        perceptual_roughness: 0.95,
        ..default()
    });
    let rock_red = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.32, 0.25),
        perceptual_roughness: 0.9,
        ..default()
    });
    let rusted_metal = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.28, 0.18),
        perceptual_roughness: 0.7,
        metallic: 0.3,
        ..default()
    });
    let corrugated_metal = materials.add(StandardMaterial {
        base_color: Color::srgb(0.38, 0.36, 0.34),
        perceptual_roughness: 0.6,
        metallic: 0.5,
        ..default()
    });
    let pine_bark = materials.add(StandardMaterial {
        base_color: Color::srgb(0.28, 0.18, 0.10),
        perceptual_roughness: 0.95,
        ..default()
    });
    let pine_green = materials.add(StandardMaterial {
        base_color: Color::srgb(0.12, 0.28, 0.10),
        perceptual_roughness: 0.85,
        ..default()
    });
    let pine_green_dark = materials.add(StandardMaterial {
        base_color: Color::srgb(0.08, 0.20, 0.07),
        perceptual_roughness: 0.85,
        ..default()
    });
    let crate_wood = materials.add(StandardMaterial {
        base_color: Color::srgb(0.55, 0.40, 0.25),
        perceptual_roughness: 0.8,
        ..default()
    });
    let barrel_metal = materials.add(StandardMaterial {
        base_color: Color::srgb(0.30, 0.32, 0.28),
        perceptual_roughness: 0.65,
        metallic: 0.4,
        ..default()
    });
    let fireplace_stone = materials.add(StandardMaterial {
        base_color: Color::srgb(0.35, 0.30, 0.28),
        perceptual_roughness: 0.95,
        ..default()
    });
    let chimney_stone = materials.add(StandardMaterial {
        base_color: Color::srgb(0.32, 0.28, 0.25),
        perceptual_roughness: 0.95,
        ..default()
    });
    let mine_rock = materials.add(StandardMaterial {
        base_color: Color::srgb(0.25, 0.22, 0.20),
        perceptual_roughness: 0.95,
        ..default()
    });
    let mine_timber = materials.add(StandardMaterial {
        base_color: Color::srgb(0.42, 0.28, 0.15),
        perceptual_roughness: 0.85,
        ..default()
    });
    let embers = materials.add(StandardMaterial {
        base_color: Color::srgb(0.15, 0.08, 0.05),
        emissive: bevy::color::LinearRgba::new(2.0, 0.4, 0.1, 1.0),
        ..default()
    });

    // Helper: spawn a static block with mesh + collider + render layer
    let rl = RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]);

    // ========================================
    // TERRAIN — Colorado wilderness ground
    // ========================================

    // Main ground plane — dead grass / dirt mix
    let floor_mesh = meshes.add(Plane3d::new(Vec3::Y, Vec2::new(60.0, 60.0)));
    commands.spawn((
        Mesh3d(floor_mesh),
        MeshMaterial3d(dead_grass.clone()),
        Transform::from_xyz(0.0, -0.05, -20.0),
        RigidBody::Static,
        Collider::cuboid(120.0, 0.1, 120.0),
        Friction::new(0.5),
        rl.clone(),
        Name::new("Ground"),
    ));

    // Packed dirt clearing around cabin
    let clearing = meshes.add(Cuboid::new(20.0, 0.1, 16.0));
    commands.spawn((
        Mesh3d(clearing),
        MeshMaterial3d(dirt.clone()),
        Transform::from_xyz(0.0, 0.05, 0.0),
        RigidBody::Static,
        Collider::cuboid(20.0, 0.1, 16.0),
        Friction::new(0.4),
        rl.clone(),
        Name::new("Clearing"),
    ));

    // Eastern hillside (stepped terrain rising toward mine)
    let hill_meshes = [
        (Vec3::new(18.0, 0.5, -8.0), Vec3::new(12.0, 1.0, 20.0)),
        (Vec3::new(24.0, 1.5, -8.0), Vec3::new(8.0, 3.0, 18.0)),
        (Vec3::new(29.0, 3.0, -8.0), Vec3::new(6.0, 6.0, 16.0)),
    ];
    for (pos, size) in hill_meshes {
        let m = meshes.add(Cuboid::new(size.x, size.y, size.z));
        commands.spawn((
            Mesh3d(m), MeshMaterial3d(grass.clone()),
            Transform::from_translation(pos),
            RigidBody::Static, Collider::cuboid(size.x, size.y, size.z),
            Friction::new(0.6), rl.clone(),
        ));
    }

    // Western ridge
    for (pos, size) in [
        (Vec3::new(-20.0, 0.3, -10.0), Vec3::new(10.0, 0.6, 24.0)),
        (Vec3::new(-26.0, 0.8, -10.0), Vec3::new(6.0, 1.6, 20.0)),
    ] {
        let m = meshes.add(Cuboid::new(size.x, size.y, size.z));
        commands.spawn((
            Mesh3d(m), MeshMaterial3d(grass.clone()),
            Transform::from_translation(pos),
            RigidBody::Static, Collider::cuboid(size.x, size.y, size.z),
            Friction::new(0.5), rl.clone(),
        ));
    }

    // Northern rocky slope
    for (pos, size) in [
        (Vec3::new(0.0, 0.4, -28.0), Vec3::new(30.0, 0.8, 10.0)),
        (Vec3::new(0.0, 1.2, -35.0), Vec3::new(25.0, 2.4, 8.0)),
    ] {
        let m = meshes.add(Cuboid::new(size.x, size.y, size.z));
        commands.spawn((
            Mesh3d(m), MeshMaterial3d(rock_red.clone()),
            Transform::from_translation(pos),
            RigidBody::Static, Collider::cuboid(size.x, size.y, size.z),
            Friction::new(0.6), rl.clone(),
        ));
    }

    // Southern approach (worn trail)
    let trail = meshes.add(Cuboid::new(4.0, 0.04, 12.0));
    commands.spawn((
        Mesh3d(trail), MeshMaterial3d(dirt_light.clone()),
        Transform::from_xyz(0.0, 0.02, 14.0),
        RigidBody::Static, Collider::cuboid(4.0, 0.04, 12.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Trail"),
    ));

    // ========================================
    // MAIN CABIN — weathered log cabin with porch
    // ========================================

    // Cabin floor (raised wooden platform)
    let cabin_floor = meshes.add(Cuboid::new(8.0, 0.2, 6.0));
    commands.spawn((
        Mesh3d(cabin_floor), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(0.0, 0.3, 0.0),
        RigidBody::Static, Collider::cuboid(8.0, 0.2, 6.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Cabin Floor"),
    ));

    // Cabin walls — log construction
    let cabin_wall_long = meshes.add(Cuboid::new(0.4, 2.8, 6.0));
    let cabin_wall_short = meshes.add(Cuboid::new(0.4, 2.8, 8.0));
    let cabin_half_wall = meshes.add(Cuboid::new(0.4, 2.8, 2.5));

    // West wall
    commands.spawn((
        Mesh3d(cabin_wall_long.clone()), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(-4.0, 1.7, 0.0),
        RigidBody::Static, Collider::cuboid(0.4, 2.8, 6.0),
        Friction::new(0.2), rl.clone(),
        Name::new("Cabin West Wall"),
    ));
    // East wall
    commands.spawn((
        Mesh3d(cabin_wall_long.clone()), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(4.0, 1.7, 0.0),
        RigidBody::Static, Collider::cuboid(0.4, 2.8, 6.0),
        Friction::new(0.2), rl.clone(),
        Name::new("Cabin East Wall"),
    ));
    // North wall (solid back)
    commands.spawn((
        Mesh3d(cabin_wall_short.clone()), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(0.0, 1.7, -3.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static, Collider::cuboid(0.4, 2.8, 8.0),
        Friction::new(0.2), rl.clone(),
        Name::new("Cabin North Wall"),
    ));
    // South wall — left of doorway
    commands.spawn((
        Mesh3d(cabin_half_wall.clone()), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(-2.75, 1.7, 3.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static, Collider::cuboid(0.4, 2.8, 2.5),
        Friction::new(0.2), rl.clone(),
        Name::new("Cabin South Wall L"),
    ));
    // South wall — right of doorway
    commands.spawn((
        Mesh3d(cabin_half_wall.clone()), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(2.75, 1.7, 3.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static, Collider::cuboid(0.4, 2.8, 2.5),
        Friction::new(0.2), rl.clone(),
        Name::new("Cabin South Wall R"),
    ));

    // Cabin roof
    let roof = meshes.add(Cuboid::new(9.0, 0.2, 7.0));
    commands.spawn((
        Mesh3d(roof), MeshMaterial3d(aged_wood.clone()),
        Transform::from_xyz(0.0, 3.3, 0.0),
        RigidBody::Static, Collider::cuboid(9.0, 0.2, 7.0),
        Friction::new(0.2), rl.clone(),
        Name::new("Cabin Roof"),
    ));

    // Front porch
    let porch_floor = meshes.add(Cuboid::new(8.0, 0.15, 3.0));
    commands.spawn((
        Mesh3d(porch_floor), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(0.0, 0.2, 5.5),
        RigidBody::Static, Collider::cuboid(8.0, 0.15, 3.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Porch"),
    ));

    // Porch railings
    let railing_side = meshes.add(Cuboid::new(0.2, 0.8, 3.0));
    let railing_front = meshes.add(Cuboid::new(0.2, 0.8, 8.0));
    commands.spawn((
        Mesh3d(railing_side.clone()), MeshMaterial3d(aged_wood.clone()),
        Transform::from_xyz(-3.9, 0.7, 5.5),
        RigidBody::Static, Collider::cuboid(0.2, 0.8, 3.0),
        Friction::new(0.2), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(railing_side), MeshMaterial3d(aged_wood.clone()),
        Transform::from_xyz(3.9, 0.7, 5.5),
        RigidBody::Static, Collider::cuboid(0.2, 0.8, 3.0),
        Friction::new(0.2), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(railing_front), MeshMaterial3d(aged_wood.clone()),
        Transform::from_xyz(0.0, 0.7, 7.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static, Collider::cuboid(0.2, 0.8, 8.0),
        Friction::new(0.2), rl.clone(),
    ));

    // Porch steps
    let step1 = meshes.add(Cuboid::new(2.0, 0.12, 0.6));
    let step2 = meshes.add(Cuboid::new(2.0, 0.06, 0.6));
    commands.spawn((
        Mesh3d(step1), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(0.0, 0.12, 7.5),
        RigidBody::Static, Collider::cuboid(2.0, 0.12, 0.6),
        Friction::new(0.3), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(step2), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(0.0, 0.06, 8.0),
        RigidBody::Static, Collider::cuboid(2.0, 0.06, 0.6),
        Friction::new(0.3), rl.clone(),
    ));

    // Table inside cabin (worn wood)
    let table = meshes.add(Cuboid::new(2.0, 0.8, 1.2));
    commands.spawn((
        Mesh3d(table), MeshMaterial3d(aged_wood.clone()),
        Transform::from_xyz(0.0, 0.4, -1.0),
        RigidBody::Static, Collider::cuboid(2.0, 0.8, 1.2),
        Friction::new(0.2), rl.clone(),
        Name::new("Cabin Table"),
    ));

    // Fireplace / hearth (stone)
    let hearth = meshes.add(Cuboid::new(2.0, 1.0, 1.0));
    commands.spawn((
        Mesh3d(hearth), MeshMaterial3d(fireplace_stone.clone()),
        Transform::from_xyz(0.0, 0.5, -2.5),
        RigidBody::Static, Collider::cuboid(2.0, 1.0, 1.0),
        Friction::new(0.4), rl.clone(),
        Name::new("Fireplace"),
    ));

    // Embers in the fireplace (faint glow)
    let ember_mesh = meshes.add(Cuboid::new(1.0, 0.15, 0.6));
    commands.spawn((
        Mesh3d(ember_mesh), MeshMaterial3d(embers.clone()),
        Transform::from_xyz(0.0, 0.15, -2.3),
        rl.clone(),
        Name::new("Embers"),
    ));

    // Chimney
    let chimney = meshes.add(Cuboid::new(1.2, 3.0, 1.2));
    commands.spawn((
        Mesh3d(chimney), MeshMaterial3d(chimney_stone.clone()),
        Transform::from_xyz(0.0, 3.0, -2.8),
        RigidBody::Static, Collider::cuboid(1.2, 3.0, 1.2),
        Friction::new(0.4), rl.clone(),
        Name::new("Chimney"),
    ));

    // ========================================
    // EQUIPMENT SHED — west of cabin
    // ========================================

    // Shed floor
    let shed_floor = meshes.add(Cuboid::new(5.0, 0.15, 4.0));
    commands.spawn((
        Mesh3d(shed_floor), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(-14.0, 0.15, 2.0),
        RigidBody::Static, Collider::cuboid(5.0, 0.15, 4.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Shed Floor"),
    ));

    // Shed walls
    let shed_wall_w = meshes.add(Cuboid::new(0.3, 2.4, 4.0));
    let shed_post = meshes.add(Cuboid::new(0.3, 2.4, 0.3));
    let shed_wall_n = meshes.add(Cuboid::new(0.3, 2.4, 5.0));
    let shed_wall_s = meshes.add(Cuboid::new(0.3, 2.4, 2.0));

    commands.spawn((
        Mesh3d(shed_wall_w), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(-16.5, 1.2, 2.0),
        RigidBody::Static, Collider::cuboid(0.3, 2.4, 4.0),
        Friction::new(0.2), rl.clone(),
        Name::new("Shed West Wall"),
    ));
    // East side — open with posts
    commands.spawn((
        Mesh3d(shed_post.clone()), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(-11.5, 1.2, 4.0),
        RigidBody::Static, Collider::cuboid(0.3, 2.4, 0.3),
        Friction::new(0.2), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(shed_post), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(-11.5, 1.2, 0.0),
        RigidBody::Static, Collider::cuboid(0.3, 2.4, 0.3),
        Friction::new(0.2), rl.clone(),
    ));
    // North wall
    commands.spawn((
        Mesh3d(shed_wall_n), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(-14.0, 1.2, 0.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static, Collider::cuboid(0.3, 2.4, 5.0),
        Friction::new(0.2), rl.clone(),
        Name::new("Shed North Wall"),
    ));
    // South wall (partial)
    commands.spawn((
        Mesh3d(shed_wall_s), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(-15.0, 1.2, 4.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static, Collider::cuboid(0.3, 2.4, 2.0),
        Friction::new(0.2), rl.clone(),
    ));

    // Shed roof (corrugated metal)
    let shed_roof = meshes.add(Cuboid::new(6.0, 0.1, 5.0));
    commands.spawn((
        Mesh3d(shed_roof), MeshMaterial3d(corrugated_metal.clone()),
        Transform::from_xyz(-14.0, 2.5, 2.0),
        RigidBody::Static, Collider::cuboid(6.0, 0.1, 5.0),
        Friction::new(0.2), rl.clone(),
        Name::new("Shed Roof"),
    ));

    // Workbench inside shed
    let workbench = meshes.add(Cuboid::new(2.5, 0.8, 0.8));
    commands.spawn((
        Mesh3d(workbench), MeshMaterial3d(aged_wood.clone()),
        Transform::from_xyz(-15.0, 0.4, 1.5),
        RigidBody::Static, Collider::cuboid(2.5, 0.8, 0.8),
        Friction::new(0.2), rl.clone(),
        Name::new("Workbench"),
    ));

    // ========================================
    // MINE ENTRANCE — carved into eastern hillside
    // ========================================

    // Mine tunnel floor
    let mine_floor = meshes.add(Cuboid::new(3.0, 0.1, 8.0));
    commands.spawn((
        Mesh3d(mine_floor), MeshMaterial3d(dirt.clone()),
        Transform::from_xyz(22.0, 0.8, -6.0),
        RigidBody::Static, Collider::cuboid(3.0, 0.1, 8.0),
        Friction::new(0.4), rl.clone(),
        Name::new("Mine Floor"),
    ));

    // Mine tunnel walls
    let mine_wall = meshes.add(Cuboid::new(0.4, 2.4, 8.0));
    commands.spawn((
        Mesh3d(mine_wall.clone()), MeshMaterial3d(mine_rock.clone()),
        Transform::from_xyz(20.5, 2.0, -6.0),
        RigidBody::Static, Collider::cuboid(0.4, 2.4, 8.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Mine Left Wall"),
    ));
    commands.spawn((
        Mesh3d(mine_wall), MeshMaterial3d(mine_rock.clone()),
        Transform::from_xyz(23.5, 2.0, -6.0),
        RigidBody::Static, Collider::cuboid(0.4, 2.4, 8.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Mine Right Wall"),
    ));

    // Mine ceiling
    let mine_ceiling = meshes.add(Cuboid::new(3.0, 0.3, 8.0));
    commands.spawn((
        Mesh3d(mine_ceiling), MeshMaterial3d(mine_rock.clone()),
        Transform::from_xyz(22.0, 3.2, -6.0),
        RigidBody::Static, Collider::cuboid(3.0, 0.3, 8.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Mine Ceiling"),
    ));

    // Mine support beams (timber frames)
    let beam_post = meshes.add(Cuboid::new(0.25, 2.0, 0.25));
    let beam_cross = meshes.add(Cuboid::new(2.8, 0.25, 0.25));
    for z_off in [-3.0_f32, -6.0, -9.0] {
        commands.spawn((
            Mesh3d(beam_post.clone()), MeshMaterial3d(mine_timber.clone()),
            Transform::from_xyz(20.8, 1.8, z_off),
            RigidBody::Static, Collider::cuboid(0.25, 2.0, 0.25),
            Friction::new(0.2), rl.clone(),
        ));
        commands.spawn((
            Mesh3d(beam_post.clone()), MeshMaterial3d(mine_timber.clone()),
            Transform::from_xyz(23.2, 1.8, z_off),
            RigidBody::Static, Collider::cuboid(0.25, 2.0, 0.25),
            Friction::new(0.2), rl.clone(),
        ));
        commands.spawn((
            Mesh3d(beam_cross.clone()), MeshMaterial3d(mine_timber.clone()),
            Transform::from_xyz(22.0, 3.0, z_off),
            RigidBody::Static, Collider::cuboid(2.8, 0.25, 0.25),
            Friction::new(0.2), rl.clone(),
        ));
    }

    // Mine entrance overhang
    let overhang = meshes.add(Cuboid::new(5.0, 1.0, 2.0));
    commands.spawn((
        Mesh3d(overhang), MeshMaterial3d(stone_dark.clone()),
        Transform::from_xyz(22.0, 3.5, -2.0),
        RigidBody::Static, Collider::cuboid(5.0, 1.0, 2.0),
        Friction::new(0.5), rl.clone(),
        Name::new("Mine Overhang"),
    ));

    // ========================================
    // SUPPLY CRATES & BARRELS
    // ========================================

    let crate_mesh_large = meshes.add(Cuboid::new(1.0, 0.8, 1.0));
    let crate_mesh_med = meshes.add(Cuboid::new(0.8, 0.4, 0.8));
    let crate_mesh_sm = meshes.add(Cuboid::new(0.6, 0.6, 0.6));
    let crate_wide = meshes.add(Cuboid::new(1.2, 0.7, 0.8));
    let crate_tiny = meshes.add(Cuboid::new(0.5, 0.5, 0.5));
    let barrel_mesh = meshes.add(Cuboid::new(0.7, 1.0, 0.7));

    // Crate stack near shed
    commands.spawn((
        Mesh3d(crate_mesh_large.clone()), MeshMaterial3d(crate_wood.clone()),
        Transform::from_xyz(-12.0, 0.4, 4.0),
        RigidBody::Static, Collider::cuboid(1.0, 0.8, 1.0),
        Friction::new(0.3), rl.clone(), Name::new("Crate Stack 1"),
    ));
    commands.spawn((
        Mesh3d(crate_mesh_med.clone()), MeshMaterial3d(crate_wood.clone()),
        Transform::from_xyz(-12.0, 1.0, 4.0),
        RigidBody::Static, Collider::cuboid(0.8, 0.4, 0.8),
        Friction::new(0.3), rl.clone(), Name::new("Crate Stack 2"),
    ));
    commands.spawn((
        Mesh3d(crate_mesh_sm.clone()), MeshMaterial3d(crate_wood.clone()),
        Transform::from_xyz(-11.0, 0.3, 3.5),
        RigidBody::Static, Collider::cuboid(0.6, 0.6, 0.6),
        Friction::new(0.3), rl.clone(),
    ));

    // Crates near cabin porch
    commands.spawn((
        Mesh3d(crate_wide.clone()), MeshMaterial3d(crate_wood.clone()),
        Transform::from_xyz(5.5, 0.35, 6.0),
        RigidBody::Static, Collider::cuboid(1.2, 0.7, 0.8),
        Friction::new(0.3), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(crate_tiny.clone()), MeshMaterial3d(crate_wood.clone()),
        Transform::from_xyz(6.5, 0.25, 5.5),
        RigidBody::Static, Collider::cuboid(0.5, 0.5, 0.5),
        Friction::new(0.3), rl.clone(),
    ));

    // Barrel cluster south of cabin
    for (x, z) in [(-3.0, 8.0), (-2.0, 8.5), (-2.5, 9.2)] {
        commands.spawn((
            Mesh3d(barrel_mesh.clone()), MeshMaterial3d(barrel_metal.clone()),
            Transform::from_xyz(x, 0.5, z),
            RigidBody::Static, Collider::cuboid(0.7, 1.0, 0.7),
            Friction::new(0.3), rl.clone(),
        ));
    }

    // Crate near mine entrance
    commands.spawn((
        Mesh3d(crate_mesh_large.clone()), MeshMaterial3d(crate_wood.clone()),
        Transform::from_xyz(19.5, 1.2, -3.0),
        RigidBody::Static, Collider::cuboid(1.0, 0.8, 1.0),
        Friction::new(0.3), rl.clone(),
    ));

    // ========================================
    // ROCKY OUTCROPS & BOULDERS
    // ========================================

    // NW boulder cluster
    let boulder_lg = meshes.add(Cuboid::new(3.0, 1.4, 2.5));
    commands.spawn((
        Mesh3d(boulder_lg), MeshMaterial3d(stone_gray.clone()),
        Transform::from_xyz(-10.0, 0.7, -15.0),
        RigidBody::Static, Collider::cuboid(3.0, 1.4, 2.5),
        Friction::new(0.7), rl.clone(), Name::new("Boulder NW 1"),
    ));
    let boulder_md = meshes.add(Cuboid::new(1.8, 0.8, 1.5));
    commands.spawn((
        Mesh3d(boulder_md.clone()), MeshMaterial3d(stone_gray.clone()),
        Transform::from_xyz(-8.5, 0.4, -13.5),
        RigidBody::Static, Collider::cuboid(1.8, 0.8, 1.5),
        Friction::new(0.7), rl.clone(),
    ));
    let boulder_md2 = meshes.add(Cuboid::new(2.0, 1.0, 1.5));
    commands.spawn((
        Mesh3d(boulder_md2), MeshMaterial3d(rock_red.clone()),
        Transform::from_xyz(-11.5, 0.5, -14.0).with_rotation(Quat::from_rotation_y(0.4)),
        RigidBody::Static, Collider::cuboid(2.0, 1.0, 1.5),
        Friction::new(0.7), rl.clone(),
    ));

    // NE rocky ridge (cover near mine approach)
    let ridge1 = meshes.add(Cuboid::new(4.0, 1.2, 1.5));
    commands.spawn((
        Mesh3d(ridge1), MeshMaterial3d(stone_dark.clone()),
        Transform::from_xyz(12.0, 0.6, -16.0).with_rotation(Quat::from_rotation_y(0.3)),
        RigidBody::Static, Collider::cuboid(4.0, 1.2, 1.5),
        Friction::new(0.7), rl.clone(), Name::new("Ridge NE 1"),
    ));
    let ridge2 = meshes.add(Cuboid::new(2.0, 0.8, 2.0));
    commands.spawn((
        Mesh3d(ridge2), MeshMaterial3d(stone_gray.clone()),
        Transform::from_xyz(14.0, 0.4, -14.0),
        RigidBody::Static, Collider::cuboid(2.0, 0.8, 2.0),
        Friction::new(0.7), rl.clone(),
    ));
    let ridge3 = meshes.add(Cuboid::new(2.5, 0.6, 1.5));
    commands.spawn((
        Mesh3d(ridge3), MeshMaterial3d(stone_dark.clone()),
        Transform::from_xyz(10.0, 0.3, -18.0).with_rotation(Quat::from_rotation_y(-0.2)),
        RigidBody::Static, Collider::cuboid(2.5, 0.6, 1.5),
        Friction::new(0.7), rl.clone(),
    ));

    // Mid-field boulders (combat cover)
    let cover1 = meshes.add(Cuboid::new(1.8, 0.9, 1.2));
    commands.spawn((
        Mesh3d(cover1), MeshMaterial3d(stone_gray.clone()),
        Transform::from_xyz(7.0, 0.45, -5.0).with_rotation(Quat::from_rotation_y(0.7)),
        RigidBody::Static, Collider::cuboid(1.8, 0.9, 1.2),
        Friction::new(0.7), rl.clone(),
    ));
    let cover2 = meshes.add(Cuboid::new(1.5, 0.7, 1.5));
    commands.spawn((
        Mesh3d(cover2), MeshMaterial3d(rock_red.clone()),
        Transform::from_xyz(-6.0, 0.35, -8.0),
        RigidBody::Static, Collider::cuboid(1.5, 0.7, 1.5),
        Friction::new(0.7), rl.clone(),
    ));
    let cover3 = meshes.add(Cuboid::new(2.0, 0.6, 1.0));
    commands.spawn((
        Mesh3d(cover3), MeshMaterial3d(stone_dark.clone()),
        Transform::from_xyz(3.0, 0.3, -20.0).with_rotation(Quat::from_rotation_y(1.1)),
        RigidBody::Static, Collider::cuboid(2.0, 0.6, 1.0),
        Friction::new(0.7), rl.clone(),
    ));

    // ========================================
    // FALLEN LOGS
    // ========================================

    let fallen_log_sizes = [
        (Vec3::new(-5.0, 0.25, -12.0), 0.6_f32, Vec3::new(0.4, 0.4, 5.0)),
        (Vec3::new(8.0, 0.3, -22.0), -0.8, Vec3::new(0.5, 0.5, 6.0)),
        (Vec3::new(-8.0, 0.2, 5.0), 1.2, Vec3::new(0.35, 0.35, 4.0)),
    ];
    for (pos, rot_y, size) in fallen_log_sizes {
        let m = meshes.add(Cuboid::new(size.x, size.y, size.z));
        commands.spawn((
            Mesh3d(m), MeshMaterial3d(log_wood.clone()),
            Transform::from_translation(pos).with_rotation(Quat::from_rotation_y(rot_y)),
            RigidBody::Static, Collider::cuboid(size.x, size.y, size.z),
            Friction::new(0.4), rl.clone(),
        ));
    }

    // ========================================
    // PINE TREES — trunk (bark cuboid) + canopy (green cuboids)
    // ========================================

    let trunk_mesh = meshes.add(Cuboid::new(0.6, 4.0, 0.6));
    // Three sizes of canopy for variety
    let canopy_large = meshes.add(Cuboid::new(3.5, 5.0, 3.5));
    let canopy_med = meshes.add(Cuboid::new(2.8, 4.0, 2.8));
    let canopy_small = meshes.add(Cuboid::new(2.2, 3.5, 2.2));

    let tree_data: &[(Vec3, u8)] = &[
        // (position of trunk base center, size: 0=large, 1=med, 2=small)
        (Vec3::new(-18.0, 0.0, -18.0), 0), (Vec3::new(-15.0, 0.0, -22.0), 1),
        (Vec3::new(-20.0, 0.0, -5.0), 0),  (Vec3::new(-22.0, 0.0, 5.0), 2),
        (Vec3::new(-17.0, 0.0, 10.0), 1),  (Vec3::new(-12.0, 0.0, -20.0), 0),
        (Vec3::new(15.0, 0.0, -24.0), 1),  (Vec3::new(18.0, 0.0, -20.0), 0),
        (Vec3::new(10.0, 0.0, 8.0), 2),    (Vec3::new(14.0, 0.0, 5.0), 1),
        (Vec3::new(-5.0, 0.0, -25.0), 0),  (Vec3::new(5.0, 0.0, -28.0), 2),
        (Vec3::new(-25.0, 0.0, -15.0), 0), (Vec3::new(20.0, 0.0, 3.0), 1),
        (Vec3::new(-8.0, 0.0, 12.0), 2),   (Vec3::new(8.0, 0.0, 14.0), 0),
        (Vec3::new(0.0, 0.0, -32.0), 1),   (Vec3::new(-14.0, 0.0, -28.0), 0),
    ];

    for (base_pos, size) in tree_data {
        let trunk_y = base_pos.y + 2.0;
        // Trunk
        commands.spawn((
            Mesh3d(trunk_mesh.clone()), MeshMaterial3d(pine_bark.clone()),
            Transform::from_xyz(base_pos.x, trunk_y, base_pos.z),
            RigidBody::Static, Collider::cuboid(0.6, 4.0, 0.6),
            Friction::new(0.3), rl.clone(),
        ));
        // Canopy
        let (canopy, canopy_h, green_mat) = match size {
            0 => (canopy_large.clone(), 6.5, pine_green.clone()),
            1 => (canopy_med.clone(), 6.0, pine_green_dark.clone()),
            _ => (canopy_small.clone(), 5.5, pine_green.clone()),
        };
        commands.spawn((
            Mesh3d(canopy), MeshMaterial3d(green_mat),
            Transform::from_xyz(base_pos.x, base_pos.y + canopy_h, base_pos.z),
            rl.clone(),
        ));
    }

    // ========================================
    // WATCHTOWER — elevated lookout NW of cabin
    // ========================================

    let post_mesh = meshes.add(Cuboid::new(0.3, 4.0, 0.3));
    let tower_platform = meshes.add(Cuboid::new(4.0, 0.2, 4.0));
    let tower_wall = meshes.add(Cuboid::new(0.15, 1.0, 4.0));
    let tower_wall_short = meshes.add(Cuboid::new(0.15, 1.0, 4.0));
    let ladder_mesh = meshes.add(Cuboid::new(0.5, 0.15, 1.0));

    // Four posts
    for (x, z) in [(-9.0, -6.0), (-9.0, -9.0), (-6.0, -6.0), (-6.0, -9.0)] {
        commands.spawn((
            Mesh3d(post_mesh.clone()), MeshMaterial3d(log_wood.clone()),
            Transform::from_xyz(x, 2.0, z),
            RigidBody::Static, Collider::cuboid(0.3, 4.0, 0.3),
            Friction::new(0.3), rl.clone(),
        ));
    }
    // Platform
    commands.spawn((
        Mesh3d(tower_platform), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(-7.5, 3.8, -7.5),
        RigidBody::Static, Collider::cuboid(4.0, 0.2, 4.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Watchtower Platform"),
    ));
    // Ladder
    commands.spawn((
        Mesh3d(ladder_mesh), MeshMaterial3d(aged_wood.clone()),
        Transform::from_xyz(-5.5, 1.9, -7.5).with_rotation(Quat::from_rotation_z(0.5)),
        RigidBody::Static, Collider::cuboid(0.5, 0.15, 1.0),
        Friction::new(0.4), rl.clone(),
        Name::new("Ladder"),
    ));

    // Half-walls (cover on watchtower)
    commands.spawn((
        Mesh3d(tower_wall.clone()), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(-9.2, 4.4, -7.5),
        RigidBody::Static, Collider::cuboid(0.15, 1.0, 4.0),
        Friction::new(0.2), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(tower_wall.clone()), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(-5.8, 4.4, -7.5),
        RigidBody::Static, Collider::cuboid(0.15, 1.0, 4.0),
        Friction::new(0.2), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(tower_wall_short), MeshMaterial3d(plank_wood.clone()),
        Transform::from_xyz(-7.5, 4.4, -9.2)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static, Collider::cuboid(0.15, 1.0, 4.0),
        Friction::new(0.2), rl.clone(),
    ));

    // ========================================
    // OLD PICKUP TRUCK — rusted, SE of cabin
    // ========================================

    let truck_body = meshes.add(Cuboid::new(2.5, 1.2, 5.0));
    let truck_cab = meshes.add(Cuboid::new(2.3, 1.0, 2.5));
    let rot_truck = Quat::from_rotation_y(0.3);

    commands.spawn((
        Mesh3d(truck_body), MeshMaterial3d(rusted_metal.clone()),
        Transform::from_xyz(10.0, 0.6, 3.0).with_rotation(rot_truck),
        RigidBody::Static, Collider::cuboid(2.5, 1.2, 5.0),
        Friction::new(0.3), rl.clone(),
        Name::new("Truck Body"),
    ));
    commands.spawn((
        Mesh3d(truck_cab), MeshMaterial3d(rusted_metal.clone()),
        Transform::from_xyz(10.0, 1.5, 1.5).with_rotation(rot_truck),
        RigidBody::Static, Collider::cuboid(2.3, 1.0, 2.5),
        Friction::new(0.3), rl.clone(),
        Name::new("Truck Cab"),
    ));

    // ========================================
    // CAMPFIRE RING — south of cabin
    // ========================================

    let fire_stone = meshes.add(Cuboid::new(0.4, 0.3, 0.4));
    let ring_center = Vec3::new(3.0, 0.0, 10.0);
    for i in 0..8 {
        let angle = i as f32 * std::f32::consts::TAU / 8.0;
        let r = 1.2;
        let x = ring_center.x + angle.cos() * r;
        let z = ring_center.z + angle.sin() * r;
        commands.spawn((
            Mesh3d(fire_stone.clone()), MeshMaterial3d(stone_dark.clone()),
            Transform::from_xyz(x, 0.15, z),
            RigidBody::Static, Collider::cuboid(0.4, 0.3, 0.4),
            Friction::new(0.7), rl.clone(),
        ));
    }

    // Campfire embers (glow)
    let campfire_embers = meshes.add(Cuboid::new(0.6, 0.2, 0.6));
    commands.spawn((
        Mesh3d(campfire_embers), MeshMaterial3d(embers.clone()),
        Transform::from_xyz(ring_center.x, 0.1, ring_center.z),
        rl.clone(),
        Name::new("Campfire Embers"),
    ));

    // Log seats
    let log_seat = meshes.add(Cuboid::new(0.3, 0.3, 1.8));
    commands.spawn((
        Mesh3d(log_seat.clone()), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(1.0, 0.2, 10.0),
        RigidBody::Static, Collider::cuboid(0.3, 0.3, 1.8),
        Friction::new(0.4), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(log_seat.clone()), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(5.0, 0.2, 10.0),
        RigidBody::Static, Collider::cuboid(0.3, 0.3, 1.8),
        Friction::new(0.4), rl.clone(),
    ));
    commands.spawn((
        Mesh3d(log_seat), MeshMaterial3d(log_wood.clone()),
        Transform::from_xyz(3.0, 0.2, 12.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static, Collider::cuboid(0.3, 0.3, 1.8),
        Friction::new(0.4), rl.clone(),
    ));

    // Campfire point light (warm flicker simulated with static warm light)
    commands.spawn((
        PointLight {
            color: Color::srgb(1.0, 0.6, 0.2),
            intensity: 15000.0,
            range: 12.0,
            shadows_enabled: false,
            ..default()
        },
        Transform::from_xyz(ring_center.x, 0.5, ring_center.z),
        rl.clone(),
    ));

    info!("Spawned world: Colorado wilderness compound (cabin, shed, mine, watchtower)");
}

/// Server-only: spawns interactive world objects as replicated entities.
/// Clients receive these via lightyear replication and add rendering in observers.
///
/// Layout matches the Colorado wilderness compound:
///   - Cabin door in south wall doorway
///   - Pickaxe on the workbench in the shed
///   - AK47 on the cabin table
///   - Ore vein inside the mine tunnel
pub fn spawn_server_interactive_objects(mut commands: Commands) {
    // Cabin door — south wall doorway (2.5m gap centered at x=0, z=3)
    commands.spawn((
        Position(Vec3::new(0.0, 1.7, 3.0)),
        Rotation::default(),
        RigidBody::Static,
        Collider::cuboid(2.5, 2.8, 0.3),
        Friction::new(0.0),
        DoorState { open: false },
        Name::new("Cabin Door"),
        Replicate::to_clients(NetworkTarget::All),
    ));

    // Pickaxe on the workbench inside the shed
    commands.spawn((
        Position(Vec3::new(-15.0, 0.9, 1.5)),
        Rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
        RigidBody::Kinematic,
        Collider::cuboid(0.6, 0.2, 0.6),
        Sensor,
        Equippable {
            name: "Pickaxe".to_string(),
            model_path: "dirty-pickaxe.glb".to_string(),
            interaction_distance: 2.0,
            scale: 1.8,
            model_rotation: [0.0, 0.0, 0.0],
            muzzle_offset: None,
        },
        Name::new("Pickaxe"),
        Replicate::to_clients(NetworkTarget::All),
    ));

    // AK47 on the cabin table
    commands.spawn((
        Position(Vec3::new(0.0, 0.9, -1.0)),
        Rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
        RigidBody::Kinematic,
        Collider::cuboid(0.6, 0.2, 0.6),
        Sensor,
        Equippable {
            name: "AK47".to_string(),
            model_path: "ak47.glb".to_string(),
            interaction_distance: 2.0,
            scale: 1.8,
            model_rotation: [std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2, 0.0],
            muzzle_offset: Some([0.2, -0.1, -0.9]),
        },
        Name::new("AK47"),
        Replicate::to_clients(NetworkTarget::All),
    ));

    // Ore vein inside the mine tunnel (deep end)
    commands.spawn((
        Position(Vec3::new(22.0, 1.2, -9.0)),
        Rotation::default(),
        RigidBody::Static,
        Collider::cuboid(0.5, 0.5, 0.5),
        Interactable {
            required_tool: Some("Pickaxe".to_string()),
            interaction_distance: 2.0,
            interaction_time: 3.0,
            model_path: "ore_chunk.glb".to_string(),
            scale: 1.0,
            mine_start_secs: None,
            last_mine_secs: None,
        },
        Name::new("Ore Vein"),
        Replicate::to_clients(NetworkTarget::All),
    ));

    info!("Server spawned interactive objects (cabin door, pickaxe in shed, AK47 on table, ore in mine)");
}

/// Lighting for the Colorado wilderness — late afternoon golden hour,
/// sun low in the west casting long shadows through the pines.
pub fn spawn_lights(mut commands: Commands) {
    // Ambient: cool blue-gray from overcast Colorado sky
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.65, 0.70, 0.80),
        brightness: 0.15,
        ..default()
    });

    // Main sun — low angle, warm golden (late afternoon, west)
    commands.spawn((
        DirectionalLight {
            illuminance: 12000.0,
            shadows_enabled: true,
            color: Color::srgb(1.0, 0.85, 0.55),
            ..default()
        },
        Transform::from_xyz(-20.0, 12.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER, VIEW_MODEL_RENDER_LAYER]),
    ));

    // Fill light — cool blue bounce from sky (opposite side)
    commands.spawn((
        DirectionalLight {
            illuminance: 2000.0,
            shadows_enabled: false,
            color: Color::srgb(0.6, 0.7, 0.9),
            ..default()
        },
        Transform::from_xyz(15.0, 8.0, -15.0).looking_at(Vec3::ZERO, Vec3::Y),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER, VIEW_MODEL_RENDER_LAYER]),
    ));
}

// ========================================
// Client init systems — add rendering to replicated interactive entities
// Uses Added<T> in Update schedule. Works because these entities use Replicate
// without InterpolationTarget, so components arrive as C (not Confirmed<C>).
// ========================================

/// Client-only system: adds rendering to replicated door entities.
pub fn init_replicated_doors(
    door_query: Query<(Entity, &DoorState, &Position, &Rotation), Added<DoorState>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, door_state, pos, rot) in door_query.iter() {
        info!("init_replicated_doors: {:?} at {:?}", entity, pos.0);

        let door_mesh = meshes.add(Cuboid::new(2.5, 2.8, 0.3));
        let wood = materials.add(Color::srgb(0.35, 0.22, 0.12));

        commands.entity(entity).insert((
            Mesh3d(door_mesh),
            MeshMaterial3d(wood),
            Transform::from_translation(pos.0).with_rotation(rot.0),
            Visibility::default(),
            Collider::cuboid(2.5, 2.8, 0.3),
            Friction::new(0.0),
            RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        ));

        if door_state.open {
            commands
                .entity(entity)
                .remove::<Collider>()
                .insert(Visibility::Hidden);
        }
    }
}

/// Client-only system: adds rendering to replicated equippable entities.
pub fn init_replicated_equippables(
    query: Query<(Entity, &Equippable, &Position, &Rotation), Added<Equippable>>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    for (entity, equippable, pos, rot) in query.iter() {
        info!("init_replicated_equippables: {} at {:?} entity={:?}", equippable.name, pos.0, entity);

        let model = asset_server
            .load(GltfAssetLabel::Scene(0).from_asset(equippable.model_path.clone()));

        commands.entity(entity).insert((
            SceneRoot(model),
            Transform::from_translation(pos.0)
                .with_rotation(rot.0)
                .with_scale(Vec3::splat(equippable.scale)),
            Visibility::default(),
            RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        ));
    }
}

/// Client-only system: adds rendering to replicated interactable entities.
pub fn init_replicated_interactables(
    query: Query<(Entity, &Interactable, &Position, &Rotation), Added<Interactable>>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    for (entity, interactable, pos, rot) in query.iter() {
        info!("init_replicated_interactables: {} at {:?}", interactable.model_path, pos.0);

        let model = asset_server
            .load(GltfAssetLabel::Scene(0).from_asset(interactable.model_path.clone()));

        commands.entity(entity).insert((
            SceneRoot(model),
            Transform::from_translation(pos.0)
                .with_rotation(rot.0)
                .with_scale(Vec3::splat(interactable.scale)),
            Visibility::default(),
            RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        ));
    }
}

// ========================================
// Client sync systems — derive visual state from replicated components
// ========================================

/// Client-only: syncs equippable visibility when any player's equipped state changes.
/// Hides world entities for items that any player is currently holding.
/// Gated by `not(is_in_rollback)` in client.rs to avoid flicker during prediction rollback.
pub fn sync_equippable_visibility(
    equipped_query: Query<&PlayerEquipped, Changed<PlayerEquipped>>,
    all_equipped: Query<&PlayerEquipped>,
    mut equippable_query: Query<(&Equippable, &mut Visibility)>,
) {
    // Only recalculate when someone's equipped state actually changed
    if equipped_query.is_empty() {
        return;
    }
    for (equippable, mut visibility) in equippable_query.iter_mut() {
        let held = all_equipped
            .iter()
            .any(|pe| pe.0.as_deref() == Some(equippable.name.as_str()));
        *visibility = if held {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }
}

/// Client-only: syncs equippable Transform with replicated Position.
/// Lightyear doesn't sync Position→Transform for non-predicted world objects.
pub fn sync_equippable_position(
    mut query: Query<(&Position, &Rotation, &mut Transform, &Equippable), Changed<Position>>,
) {
    for (pos, rot, mut transform, equippable) in query.iter_mut() {
        transform.translation = pos.0;
        transform.rotation = rot.0;
        transform.scale = Vec3::splat(equippable.scale);
    }
}

/// Client-only: syncs door visual state when DoorState changes via replication.
pub fn sync_door_state(
    mut door_query: Query<(Entity, &DoorState), Changed<DoorState>>,
    mut commands: Commands,
) {
    for (entity, door_state) in door_query.iter_mut() {
        if door_state.open {
            commands
                .entity(entity)
                .remove::<Collider>()
                .insert(Visibility::Hidden);
        }
    }
}

/// Marker for a third-person equipped item attached to a remote player.
#[derive(Component)]
pub struct RemoteEquippedItem;

/// Client-only: attaches/detaches a visible GLTF model on remote players when their
/// PlayerEquipped state changes. Only runs on non-local players.
pub fn sync_remote_equipped(
    changed_query: Query<
        (Entity, &PlayerEquipped),
        (Changed<PlayerEquipped>, Without<lightyear::prelude::Controlled>),
    >,
    children_query: Query<&Children>,
    remote_item_query: Query<Entity, With<RemoteEquippedItem>>,
    equippable_query: Query<&Equippable>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    for (player_entity, equipped) in changed_query.iter() {
        // Remove existing equipped model from this player
        if let Ok(children) = children_query.get(player_entity) {
            for child in children.iter() {
                if remote_item_query.get(child).is_ok() {
                    commands.entity(child).despawn();
                }
            }
        }

        // Attach new model if something is equipped
        let Some(ref tool_name) = equipped.0 else { continue; };

        let Some(equippable) = equippable_query.iter().find(|e| e.name == *tool_name) else {
            continue;
        };
        let model_path = equippable.model_path.clone();
        let scale = equippable.scale;
        let [rx, ry, rz] = equippable.model_rotation;
        let model_rot = Quat::from_euler(EulerRot::YXZ, ry, rx, rz);

        let asset_path = GltfAssetLabel::Scene(0).from_asset(model_path);
        let model = commands
            .spawn((
                SceneRoot(asset_server.load(asset_path)),
                Transform::from_xyz(0.3, 0.4, -0.3)
                    .with_scale(Vec3::splat(scale))
                    .with_rotation(model_rot),
                RemoteEquippedItem,
                RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
            ))
            .id();

        commands.entity(player_entity).add_child(model);
    }
}


// ========================================
// Shared observers — run on both client + server via BEI input replay
// ========================================

/// Shared observer: opens door when player presses E within range.
pub fn shared_door_interact(
    trigger: On<Fire<InteractAction>>,
    player_query: Query<(&Position, Has<Predicted>, Has<Interpolated>)>,
    mut door_query: Query<(Entity, &Position, &mut DoorState)>,
    mut commands: Commands,
) {
    let Ok((player_pos, is_predicted, is_interpolated)) = player_query.get(trigger.context) else {
        return;
    };
    if is_interpolated { return; }

    for (entity, door_pos, mut door) in door_query.iter_mut() {
        if door.open {
            continue;
        }
        if player_pos.0.distance(door_pos.0) <= DOOR_INTERACT_DISTANCE {
            door.open = true;
            // Server: remove Collider so players can walk through.
            // Client: sync_door_state handles rendering changes via Changed<DoorState>.
            if !is_predicted {
                commands.entity(entity).remove::<Collider>();
            }
            info!("Door opened!");
            break;
        }
    }
}

/// Shared observer: equip items when player presses E within range.
pub fn shared_equip_interact(
    trigger: On<Fire<InteractAction>>,
    mut player_query: Query<(&Position, &mut PlayerEquipped, Has<Interpolated>)>,
    equippable_query: Query<(Entity, &Position, &Equippable), Without<PlayerEquipped>>,
) {
    let Ok((player_pos, mut equipped, is_interpolated)) = player_query.get_mut(trigger.context) else {
        return;
    };
    if is_interpolated { return; }

    let mut closest: Option<(Entity, f32, String)> = None;
    for (entity, eq_pos, equippable) in equippable_query.iter() {
        let dist = player_pos.0.distance(eq_pos.0);
        if dist <= equippable.interaction_distance {
            if closest.as_ref().is_none_or(|(_, d, _)| dist < *d) {
                closest = Some((entity, dist, equippable.name.clone()));
            }
        }
    }

    if let Some((_, _, name)) = closest {
        if equipped.0.as_ref() == Some(&name) {
            return;
        }
        info!("Equipped {}", name);
        equipped.0 = Some(name);
    }
}

/// Shared observer: drop equipped item when player presses G.
pub fn shared_drop(
    trigger: On<Fire<DropAction>>,
    mut player_query: Query<(&Position, &mut PlayerEquipped, Has<Interpolated>)>,
    mut equippable_query: Query<(Entity, &mut Position, &Equippable), Without<PlayerEquipped>>,
) {
    let Ok((player_pos, mut equipped, is_interpolated)) = player_query.get_mut(trigger.context) else {
        return;
    };
    if is_interpolated { return; }

    let Some(dropped_name) = equipped.0.take() else {
        return;
    };
    info!("Dropped {}", dropped_name);

    for (_, mut eq_pos, equippable) in equippable_query.iter_mut() {
        if equippable.name == dropped_name {
            eq_pos.0 = player_pos.0 + Vec3::new(0.0, -0.5, 0.0);
            break;
        }
    }
}

/// Shared observer: primary action — routes to mine or shoot based on equipped item.
/// Rollback-safe: stores `mine_start_secs` (absolute time) and computes progress
/// as `current_time - start_time`. Idempotent — replaying the same tick
/// during rollback produces the same result without double-counting.
///
/// Only the server handles despawn + ore chunk spawn (replicates to all clients).
pub fn shared_primary_action(
    trigger: On<Fire<PrimaryAction>>,
    player_query: Query<(&Position, &PlayerYaw, &PlayerPitch, &PlayerEquipped, &crate::protocol::PlayerId, Has<Predicted>, Has<Interpolated>)>,
    mut interactables_query: Query<(Entity, &Position, &mut Interactable)>,
    mut health_query: Query<(Entity, &mut PlayerHealth, &Position, Option<&mut crate::protocol::LastDamagedBy>)>,
    equippable_query: Query<&Equippable>,
    spatial_query: SpatialQuery,
    mut commands: Commands,
    mut last_shot: Local<f32>,
    mut shot_counter: Local<u32>,
    time: Res<Time>,
) {
    let Ok((player_pos, yaw, pitch, equipped, attacker_id, is_predicted, is_interpolated)) = player_query.get(trigger.context) else {
        return;
    };
    if is_interpolated { return; }

    let tool_name = equipped.0.as_deref();

    match tool_name {
        // Gun equipped → hitscan shoot
        Some(name) if name.contains("AK") || name.contains("ak") || name.contains("gun") => {
            let current = time.elapsed_secs();
            if current - *last_shot < SHOOT_COOLDOWN {
                return;
            }
            *last_shot = current;

            let eye_pos = player_pos.0 + Vec3::Y * 0.8;
            let ray_dir = Quat::from_euler(EulerRot::YXZ, yaw.0, pitch.0, 0.0) * Vec3::NEG_Z;
            let filter = SpatialQueryFilter::from_excluded_entities([trigger.context]);

            info!(
                "[SHOOT] Fire! pos={:?} yaw={:.2} pitch={:.2} dir={:?} predicted={}",
                eye_pos, yaw.0, pitch.0, ray_dir, is_predicted
            );

            // Log all players with colliders for debugging hit detection
            if !is_predicted {
                for (e, hp, pos, _) in health_query.iter() {
                    info!(
                        "[SHOOT] Potential target: {:?} pos={:?} hp={} dist={:.1}",
                        e, pos.0, hp.0, eye_pos.distance(pos.0)
                    );
                }
            }

            let tracer_dist;

            if let Some(hit) = spatial_query.cast_ray(
                eye_pos,
                Dir3::new(ray_dir).unwrap_or(Dir3::NEG_Z),
                SHOOT_RANGE,
                true,
                &filter,
            ) {
                tracer_dist = hit.distance;
                info!(
                    "[SHOOT] Ray hit entity {:?} at distance {:.1}",
                    hit.entity, hit.distance
                );
                if !is_predicted {
                    if let Ok((_entity, mut health, _pos, last_damaged)) = health_query.get_mut(hit.entity) {
                        health.0 -= SHOOT_DAMAGE;
                        if let Some(mut last) = last_damaged {
                            last.0 = attacker_id.0;
                        }
                        info!(
                            "[SHOOT] Player hit! {} damage applied, health now: {}",
                            SHOOT_DAMAGE, health.0
                        );
                    } else {
                        info!("[SHOOT] Hit entity {:?} but it has no PlayerHealth", hit.entity);
                    }
                }
            } else {
                tracer_dist = SHOOT_RANGE;
                info!("[SHOOT] Miss — no ray hit within {} range", SHOOT_RANGE);
            }

            // Look up muzzle offset from the Equippable component
            let muzzle_local = equippable_query
                .iter()
                .find(|e| e.name == *name)
                .and_then(|e| e.muzzle_offset)
                .map(|o| Vec3::from_array(o))
                .unwrap_or(Vec3::new(0.2, -0.1, -0.9));

            let cam_rot = Quat::from_euler(EulerRot::YXZ, yaw.0, pitch.0, 0.0);
            let muzzle_world = eye_pos + cam_rot * muzzle_local;
            let hit_point = eye_pos + ray_dir * tracer_dist;

            commands.trigger(ShotFired {
                muzzle: muzzle_world,
                hit_point,
            });

            // Set LastShot on the player entity so remote clients can see the tracer
            *shot_counter += 1;
            commands.entity(trigger.context).insert(crate::protocol::LastShot {
                muzzle: muzzle_world,
                hit_point,
                tick: *shot_counter,
            });
        }

        // Tool equipped → mine nearby interactable
        Some(_tool) => {
            let current_secs = time.elapsed_secs();

            let mut closest: Option<Entity> = None;
            let mut closest_dist = f32::MAX;

            for (entity, pos, interactable) in interactables_query.iter() {
                let dist = player_pos.0.distance(pos.0);
                if dist <= interactable.interaction_distance && dist < closest_dist {
                    let tool_matches = interactable.required_tool.is_none()
                        || interactable.required_tool.as_deref() == tool_name;
                    if tool_matches {
                        closest_dist = dist;
                        closest = Some(entity);
                    }
                }
            }

            let Some(target) = closest else { return; };
            let Ok((_, pos, mut interactable)) = interactables_query.get_mut(target) else { return; };

            if let Some(last) = interactable.last_mine_secs {
                if current_secs - last > 0.05 {
                    interactable.mine_start_secs = None;
                }
            }
            interactable.last_mine_secs = Some(current_secs);

            if interactable.mine_start_secs.is_none() {
                interactable.mine_start_secs = Some(current_secs);
                info!("Started mining");
            }

            let progress = interactable.progress(current_secs);
            if progress >= interactable.interaction_time {
                info!("Mining complete!");
                if !is_predicted {
                    let spawn_pos = pos.0;
                    commands.entity(target).despawn();
                    commands.spawn((
                        Position(spawn_pos + Vec3::new(0.0, 0.3, 0.0)),
                        Rotation::default(),
                        RigidBody::Dynamic,
                        Collider::cuboid(0.2, 0.2, 0.2),
                        Equippable {
                            name: "Ore Chunk".to_string(),
                            model_path: "ore_chunk.glb".to_string(),
                            interaction_distance: 2.0,
                            scale: 0.5,
                            model_rotation: [0.0, 0.0, 0.0],
                            muzzle_offset: None,
                        },
                        Name::new("Ore Chunk"),
                        Replicate::to_clients(NetworkTarget::All),
                    ));
                }
            }
        }

        // Nothing equipped → no action
        None => {}
    }
}

const SHOOT_DAMAGE: i32 = 25;
const SHOOT_RANGE: f32 = 500.0;
const SHOOT_COOLDOWN: f32 = 0.15;

/// Shared system: resets mining state on interactables that haven't been mined recently.
/// Runs every FixedUpdate. If `last_mine_secs` is stale (>0.05s ago), clears mining state.
pub fn reset_stale_mining(
    mut interactables: Query<&mut Interactable>,
    time: Res<Time>,
) {
    let current_secs = time.elapsed_secs();
    for mut interactable in interactables.iter_mut() {
        if let Some(last) = interactable.last_mine_secs {
            if current_secs - last > 0.05 {
                interactable.mine_start_secs = None;
                interactable.last_mine_secs = None;
            }
        }
    }
}

// ========================================
// Client-only systems
// ========================================

/// Client-only: spawns/despawns the FPS view model when PlayerEquipped changes.
pub fn update_view_model(
    player_query: Query<(&PlayerEquipped, &Children), With<lightyear::prelude::Controlled>>,
    camera_query: Query<Entity, With<WorldModelCamera>>,
    view_model_query: Query<Entity, With<EquippedItem>>,
    equippable_query: Query<&Equippable>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut last_equipped: Local<Option<String>>,
) {
    let Ok((equipped, children)) = player_query.single() else {
        return;
    };

    // Only react to changes
    if *last_equipped == equipped.0 {
        return;
    }
    *last_equipped = equipped.0.clone();

    // Despawn any existing view model
    for vm_entity in view_model_query.iter() {
        commands.entity(vm_entity).despawn();
    }

    // If nothing equipped, we're done
    let Some(ref tool_name) = equipped.0 else {
        return;
    };

    // Find the Equippable to get its model path and rotation
    let equippable = equippable_query
        .iter()
        .find(|e| e.name == *tool_name);

    let Some(equippable) = equippable else {
        return;
    };

    let asset_path = GltfAssetLabel::Scene(0).from_asset(equippable.model_path.clone());
    let model_handle = asset_server.load(asset_path);
    let [rx, ry, rz] = equippable.model_rotation;
    let model_rot = Quat::from_euler(EulerRot::YXZ, ry, rx, rz);

    let view_model = commands
        .spawn((
            SceneRoot(model_handle),
            Transform::from_xyz(0.2, -0.15, -0.4)
                .with_scale(Vec3::splat(1.0))
                .with_rotation(model_rot),
            RenderLayers::layer(VIEW_MODEL_RENDER_LAYER),
            EquippedItem {
                name: tool_name.clone(),
            },
        ))
        .id();

    // Attach to camera
    let cam_entity = children
        .iter()
        .find(|c| camera_query.get(*c).is_ok());
    if let Some(parent) = cam_entity {
        commands.entity(parent).add_child(view_model);
    }
}

/// Client-only: shows mining progress bar when any Interactable is being mined.
pub fn interaction_ui_system(
    mut contexts: bevy_egui::EguiContexts,
    interactables_query: Query<&Interactable>,
    time: Res<Time>,
) {
    let current_secs = time.elapsed_secs();

    // Find any interactable with active mining
    let active = interactables_query
        .iter()
        .find(|i| i.mine_start_secs.is_some());

    let Some(interactable) = active else {
        return;
    };

    let progress = interactable.progress(current_secs);
    let max_time = interactable.interaction_time;

    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let screen_rect = ctx.content_rect();

    bevy_egui::egui::Window::new("Mining Progress")
        .title_bar(false)
        .resizable(false)
        .collapsible(false)
        .fixed_pos(bevy_egui::egui::pos2(
            screen_rect.width() / 2.0 - 100.0,
            screen_rect.height() - 70.0,
        ))
        .fixed_size(bevy_egui::egui::vec2(200.0, 50.0))
        .show(ctx, |ui| {
            let percent = (progress / max_time * 100.0) as i32;
            ui.label(format!("Mining... {}%", percent));
            ui.add(
                bevy_egui::egui::ProgressBar::new(progress / max_time)
                    .desired_width(200.0),
            );
        });
}
