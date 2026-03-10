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
    player_query: Query<(&Position, &PlayerYaw, &PlayerPitch, Has<Predicted>, Has<Interpolated>)>,
    mut health_query: Query<(Entity, &mut PlayerHealth, &Position)>,
    spatial_query: SpatialQuery,
    mut commands: Commands,
    mut last_jab: Local<f32>,
    time: Res<Time>,
) {
    let Ok((player_pos, yaw, pitch, is_predicted, is_interpolated)) = player_query.get(trigger.context) else {
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
            if let Ok((_entity, mut health, _pos)) = health_query.get_mut(hit.entity) {
                health.0 -= JAB_DAMAGE;
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
pub fn spawn_world_physics(mut commands: Commands) {
    // Floor
    commands.spawn((
        Transform::from_xyz(0.0, 0.0, -20.0),
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Friction::new(0.0),
    ));

    // West wall
    commands.spawn((Transform::from_xyz(-5.0, 2.0, 0.0), RigidBody::Static, Collider::cuboid(0.5, 4.0, 10.0), Friction::new(0.0)));
    // East wall
    commands.spawn((Transform::from_xyz(5.0, 2.0, 0.0), RigidBody::Static, Collider::cuboid(0.5, 4.0, 10.0), Friction::new(0.0)));
    // South wall
    commands.spawn((Transform::from_xyz(0.0, 2.0, 5.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)), RigidBody::Static, Collider::cuboid(0.5, 4.0, 10.0), Friction::new(0.0)));
    // North wall left
    commands.spawn((Transform::from_xyz(-3.25, 2.0, -5.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)), RigidBody::Static, Collider::cuboid(0.5, 4.0, 3.5), Friction::new(0.0)));
    // North wall right
    commands.spawn((Transform::from_xyz(3.25, 2.0, -5.0).with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)), RigidBody::Static, Collider::cuboid(0.5, 4.0, 3.5), Friction::new(0.0)));

    // Table
    commands.spawn((Transform::from_xyz(0.0, 0.0, -3.0), RigidBody::Static, Collider::cuboid(2.0, 1.0, 1.0), Friction::new(0.0)));

    // Staircase
    for i in 0..6 {
        let h = 0.5 * (i as f32 + 1.0);
        commands.spawn((Transform::from_xyz(-6.0, h / 2.0, -8.0 - i as f32 * 1.5), RigidBody::Static, Collider::cuboid(2.0, h, 1.5), Friction::new(0.0)));
    }

    // Ramp
    commands.spawn((Transform::from_xyz(6.0, 1.5, -12.0).with_rotation(Quat::from_rotation_x(-0.25)), RigidBody::Static, Collider::cuboid(3.0, 0.2, 8.0), Friction::new(0.8)));

    // Platforms
    commands.spawn((Transform::from_xyz(0.0, 0.5, -9.0), RigidBody::Static, Collider::cuboid(3.0, 1.0, 3.0), Friction::new(0.0)));
    commands.spawn((Transform::from_xyz(0.0, 0.5, -15.0), RigidBody::Static, Collider::cuboid(3.0, 1.0, 3.0), Friction::new(0.0)));

    // Stepping stones
    for (pos, _) in [
        (Vec3::new(0.0, 1.0, -20.0), 0), (Vec3::new(2.0, 1.5, -22.0), 0),
        (Vec3::new(-1.0, 2.0, -24.0), 0), (Vec3::new(1.5, 2.5, -26.0), 0),
        (Vec3::new(-0.5, 3.0, -28.0), 0),
    ] {
        commands.spawn((Transform::from_translation(pos), RigidBody::Static, Collider::cuboid(1.5, 0.3, 1.5), Friction::new(0.0)));
    }

    // Pillars
    commands.spawn((Transform::from_xyz(-6.0, 3.0, -20.0), RigidBody::Static, Collider::cuboid(1.5, 6.0, 1.5), Friction::new(0.0)));
    commands.spawn((Transform::from_xyz(-6.0, 3.0, -24.0), RigidBody::Static, Collider::cuboid(1.5, 6.0, 1.5), Friction::new(0.0)));

    // Low wall
    commands.spawn((Transform::from_xyz(6.0, 0.6, -18.0), RigidBody::Static, Collider::cuboid(6.0, 1.2, 0.5), Friction::new(0.0)));

    // Elevated walkway
    commands.spawn((Transform::from_xyz(10.0, 2.0, -16.0), RigidBody::Static, Collider::cuboid(2.0, 0.3, 12.0), Friction::new(0.0)));

    info!("Server: spawned world physics colliders");
}

/// Client-only: spawns static world geometry with rendering + physics.
/// Interactive objects (door, pickaxe, ore) are server-spawned replicated entities.
pub fn spawn_world_model(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let white = materials.add(Color::WHITE);
    let gray = materials.add(Color::srgb(0.5, 0.5, 0.5));
    let dark_gray = materials.add(Color::srgb(0.3, 0.3, 0.3));
    let wood = materials.add(Color::from(tailwind::AMBER_700));
    let green = materials.add(Color::from(tailwind::GREEN_600));
    let blue = materials.add(Color::from(tailwind::BLUE_400));
    let red = materials.add(Color::from(tailwind::RED_500));
    let yellow = materials.add(Color::from(tailwind::YELLOW_400));

    // ========================================
    // FLOOR — large ground plane covering room + gym
    // ========================================
    let floor = meshes.add(Plane3d::new(Vec3::Y, Vec2::new(50.0, 50.0)));
    commands.spawn((
        Mesh3d(floor),
        MeshMaterial3d(white.clone()),
        Transform::from_xyz(0.0, 0.0, -20.0),
        RigidBody::Static,
        Collider::cuboid(100.0, 0.1, 100.0),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    // ========================================
    // STARTING ROOM — 10x10, walls with door gap in north wall
    // (Door is a server-spawned replicated entity)
    // ========================================
    let wall = meshes.add(Cuboid::new(0.5, 4.0, 10.0));
    let half_wall = meshes.add(Cuboid::new(0.5, 4.0, 3.5));

    // West wall (x = -5)
    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(white.clone()),
        Transform::from_xyz(-5.0, 2.0, 0.0),
        RigidBody::Static,
        Collider::cuboid(0.5, 4.0, 10.0),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    // East wall (x = 5)
    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(white.clone()),
        Transform::from_xyz(5.0, 2.0, 0.0),
        RigidBody::Static,
        Collider::cuboid(0.5, 4.0, 10.0),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    // South wall (z = 5)
    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(white.clone()),
        Transform::from_xyz(0.0, 2.0, 5.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static,
        Collider::cuboid(0.5, 4.0, 10.0),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    // North wall — split into two halves with a 3-unit door gap in the center
    // Left half (x = -5 to x = -1.5)
    commands.spawn((
        Mesh3d(half_wall.clone()),
        MeshMaterial3d(white.clone()),
        Transform::from_xyz(-3.25, 2.0, -5.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static,
        Collider::cuboid(0.5, 4.0, 3.5),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));
    // Right half (x = 1.5 to x = 5)
    commands.spawn((
        Mesh3d(half_wall.clone()),
        MeshMaterial3d(white.clone()),
        Transform::from_xyz(3.25, 2.0, -5.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        RigidBody::Static,
        Collider::cuboid(0.5, 4.0, 3.5),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    // Table in room
    let table_size = Vec3::new(2.0, 1.0, 1.0);
    let table = meshes.add(Cuboid::new(table_size.x, table_size.y, table_size.z));
    commands.spawn((
        Mesh3d(table),
        MeshMaterial3d(wood.clone()),
        Transform::from_xyz(0.0, 0.0, -3.0),
        RigidBody::Static,
        Collider::cuboid(table_size.x, table_size.y, table_size.z),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    // ========================================
    // PHYSICS GYM — north of the room (z < -6)
    // ========================================

    // --- Staircase (ascending blocks, left side) ---
    for i in 0..6 {
        let step_height = 0.5 * (i as f32 + 1.0);
        let step = meshes.add(Cuboid::new(2.0, step_height, 1.5));
        commands.spawn((
            Mesh3d(step),
            MeshMaterial3d(gray.clone()),
            Transform::from_xyz(-6.0, step_height / 2.0, -8.0 - i as f32 * 1.5),
            RigidBody::Static,
            Collider::cuboid(2.0, step_height, 1.5),
            Friction::new(0.0),
            RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
            Name::new(format!("Stair {}", i + 1)),
        ));
    }

    // --- Ramp (right side) ---
    let ramp = meshes.add(Cuboid::new(3.0, 0.2, 8.0));
    commands.spawn((
        Mesh3d(ramp),
        MeshMaterial3d(green.clone()),
        Transform::from_xyz(6.0, 1.5, -12.0)
            .with_rotation(Quat::from_rotation_x(-0.25)), // ~14 degree incline
        RigidBody::Static,
        Collider::cuboid(3.0, 0.2, 8.0),
        Friction::new(0.8),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        Name::new("Ramp"),
    ));

    // --- Jump gap (center) — two platforms with a gap ---
    let platform = meshes.add(Cuboid::new(3.0, 1.0, 3.0));

    // Near platform
    commands.spawn((
        Mesh3d(platform.clone()),
        MeshMaterial3d(blue.clone()),
        Transform::from_xyz(0.0, 0.5, -9.0),
        RigidBody::Static,
        Collider::cuboid(3.0, 1.0, 3.0),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        Name::new("Jump Platform Near"),
    ));

    // Far platform (3-unit gap)
    commands.spawn((
        Mesh3d(platform.clone()),
        MeshMaterial3d(blue.clone()),
        Transform::from_xyz(0.0, 0.5, -15.0),
        RigidBody::Static,
        Collider::cuboid(3.0, 1.0, 3.0),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        Name::new("Jump Platform Far"),
    ));

    // --- Stepping stones (various heights) ---
    let stone = meshes.add(Cuboid::new(1.5, 0.3, 1.5));
    let stone_positions = [
        (Vec3::new(0.0, 1.0, -20.0), yellow.clone()),
        (Vec3::new(2.0, 1.5, -22.0), red.clone()),
        (Vec3::new(-1.0, 2.0, -24.0), green.clone()),
        (Vec3::new(1.5, 2.5, -26.0), blue.clone()),
        (Vec3::new(-0.5, 3.0, -28.0), yellow.clone()),
    ];

    for (i, (pos, mat)) in stone_positions.into_iter().enumerate() {
        commands.spawn((
            Mesh3d(stone.clone()),
            MeshMaterial3d(mat),
            Transform::from_translation(pos),
            RigidBody::Static,
            Collider::cuboid(1.5, 0.3, 1.5),
            Friction::new(0.0),
            RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
            Name::new(format!("Stepping Stone {}", i + 1)),
        ));
    }

    // --- Tall pillars to jump between ---
    let pillar = meshes.add(Cuboid::new(1.5, 6.0, 1.5));
    commands.spawn((
        Mesh3d(pillar.clone()),
        MeshMaterial3d(dark_gray.clone()),
        Transform::from_xyz(-6.0, 3.0, -20.0),
        RigidBody::Static,
        Collider::cuboid(1.5, 6.0, 1.5),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        Name::new("Pillar 1"),
    ));
    commands.spawn((
        Mesh3d(pillar),
        MeshMaterial3d(dark_gray.clone()),
        Transform::from_xyz(-6.0, 3.0, -24.0),
        RigidBody::Static,
        Collider::cuboid(1.5, 6.0, 1.5),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        Name::new("Pillar 2"),
    ));

    // --- Wide low wall to jump over ---
    let low_wall = meshes.add(Cuboid::new(6.0, 1.2, 0.5));
    commands.spawn((
        Mesh3d(low_wall),
        MeshMaterial3d(red.clone()),
        Transform::from_xyz(6.0, 0.6, -18.0),
        RigidBody::Static,
        Collider::cuboid(6.0, 1.2, 0.5),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        Name::new("Low Wall"),
    ));

    // --- Elevated walkway ---
    let walkway = meshes.add(Cuboid::new(2.0, 0.3, 12.0));
    commands.spawn((
        Mesh3d(walkway),
        MeshMaterial3d(gray.clone()),
        Transform::from_xyz(10.0, 2.0, -16.0),
        RigidBody::Static,
        Collider::cuboid(2.0, 0.3, 12.0),
        Friction::new(0.0),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        Name::new("Elevated Walkway"),
    ));

    info!("Spawned world: room with door gap + physics gym");
}

/// Server-only: spawns interactive world objects as replicated entities.
/// Clients receive these via lightyear replication and add rendering in observers.
pub fn spawn_server_interactive_objects(mut commands: Commands) {
    // Door in the north wall gap
    commands.spawn((
        Position(Vec3::new(0.0, 2.0, -5.0)),
        Rotation::default(),
        RigidBody::Static,
        Collider::cuboid(3.0, 4.0, 0.3),
        Friction::new(0.0),
        DoorState { open: false },
        Name::new("Door"),
        Replicate::to_clients(NetworkTarget::All),
    ));

    // Pickaxe on the table
    commands.spawn((
        Position(Vec3::new(0.0, 0.5, -3.0)),
        Rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
        RigidBody::Kinematic,
        Collider::cuboid(0.6, 0.2, 0.6),
        Sensor,
        Equippable {
            name: "Pickaxe".to_string(),
            model_path: "dirty-pickaxe.glb".to_string(),
            interaction_distance: 2.0,
            scale: 1.8,
            model_rotation: [std::f32::consts::FRAC_PI_2, 0.0, 0.0],
            muzzle_offset: None,
        },
        Name::new("Pickaxe"),
        Replicate::to_clients(NetworkTarget::All),
    ));

    // AK47 on the table
    commands.spawn((
        Position(Vec3::new(-1.0, 0.5, -3.0)),
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
            // Muzzle tip in camera-local space: gun at (0.2, -0.15, -0.4),
            // barrel extends ~0.5 along -Z at scale 1.0
            muzzle_offset: Some([0.2, -0.1, -0.9]),
        },
        Name::new("AK47"),
        Replicate::to_clients(NetworkTarget::All),
    ));

    // Ore block in room
    commands.spawn((
        Position(Vec3::new(2.0, 0.5, -3.0)),
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
        Name::new("Ore Block"),
        Replicate::to_clients(NetworkTarget::All),
    ));

    info!("Server spawned interactive objects (door, pickaxe, AK47, ore block)");
}

pub fn spawn_lights(mut commands: Commands) {
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(1.0, 0.95, 0.9),
        brightness: 0.3,
        ..default()
    });

    commands.spawn((
        DirectionalLight {
            illuminance: 10000.0,
            shadows_enabled: true,
            color: Color::from(tailwind::ROSE_300),
            ..default()
        },
        Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
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

        let door_mesh = meshes.add(Cuboid::new(3.0, 4.0, 0.3));
        let wood = materials.add(Color::from(tailwind::AMBER_700));

        commands.entity(entity).insert((
            Mesh3d(door_mesh),
            MeshMaterial3d(wood),
            Transform::from_translation(pos.0).with_rotation(rot.0),
            Visibility::default(),
            Collider::cuboid(3.0, 4.0, 0.3),
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
        info!("init_replicated_equippables: {} at {:?}", equippable.name, pos.0);

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
            Collider::cuboid(0.5, 0.5, 0.5),
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
        let model_rot = Quat::from_euler(EulerRot::XYZ, rx, ry, rz);

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
    player_query: Query<(&Position, &PlayerYaw, &PlayerPitch, &PlayerEquipped, Has<Predicted>, Has<Interpolated>)>,
    mut interactables_query: Query<(Entity, &Position, &mut Interactable)>,
    mut health_query: Query<(Entity, &mut PlayerHealth, &Position)>,
    equippable_query: Query<&Equippable>,
    spatial_query: SpatialQuery,
    mut commands: Commands,
    mut last_shot: Local<f32>,
    time: Res<Time>,
) {
    let Ok((player_pos, yaw, pitch, equipped, is_predicted, is_interpolated)) = player_query.get(trigger.context) else {
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
                for (e, hp, pos) in health_query.iter() {
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
                    if let Ok((_entity, mut health, _pos)) = health_query.get_mut(hit.entity) {
                        health.0 -= SHOOT_DAMAGE;
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
    let model_rot = Quat::from_euler(EulerRot::XYZ, rx, ry, rz);

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
