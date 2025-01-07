use bevy::log::info;
use bevy::{color::palettes::tailwind, prelude::*, render::view::RenderLayers};
use bevy_rapier3d::prelude::*;

use crate::player::VIEW_MODEL_RENDER_LAYER;

#[derive(Debug, Component)]
pub struct WorldModelCamera;

/// Used implicitly by all entities without a `RenderLayers` component.
/// Our world model camera and all objects other than the player are on this layer.
/// The light source belongs to both layers.
pub const DEFAULT_RENDER_LAYER: usize = 0;

pub fn spawn_world_model(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let floor = meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(10.0)));
    let cube = meshes.add(Cuboid::new(2.0, 0.5, 1.0));
    let wall = meshes.add(Cuboid::new(0.5, 4.0, 10.0));
    let material = materials.add(Color::WHITE);

    commands.spawn((
        Mesh3d(floor),
        MeshMaterial3d(material.clone()),
        Collider::cuboid(5.0, 0.1, 5.0),
        RigidBody::Fixed,
    ));

    commands.spawn((
        Mesh3d(cube.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(0.0, 0.25, -3.0),
    ));

    commands.spawn((
        Mesh3d(cube),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(0.75, 1.75, 0.0),
    ));

    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(-5.0, 2.0, 0.0),
        Collider::cuboid(0.25, 2.0, 5.0),
        RigidBody::Fixed,
    ));

    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(5.0, 2.0, 0.0),
        Collider::cuboid(0.25, 2.0, 5.0),
        RigidBody::Fixed,
    ));

    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(0.0, 2.0, 5.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        Collider::cuboid(0.25, 2.0, 5.0),
        RigidBody::Fixed,
    ));

    commands.spawn((
        Mesh3d(wall),
        MeshMaterial3d(material),
        Transform::from_xyz(0.0, 2.0, -5.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        Collider::cuboid(0.25, 2.0, 5.0),
        RigidBody::Fixed,
    ));
}

pub fn spawn_lights(mut commands: Commands) {
    commands.spawn((
        PointLight {
            color: Color::from(tailwind::ROSE_300),
            shadows_enabled: true,
            ..default()
        },
        Transform::from_xyz(-2.0, 4.0, -0.75),
        // The light source illuminates both the world model and the view model.
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER, VIEW_MODEL_RENDER_LAYER]),
    ));
}

// pub fn spawn_text(mut commands: Commands) {
//     commands
//         .spawn(Node {
//             position_type: PositionType::Absolute,
//             bottom: Val::Px(12.0),
//             left: Val::Px(12.0),
//             ..default()
//         })
//         .with_child(Text::new(concat!(
//             "Move the camera with your mouse.\n",
//             "Press arrow up to decrease the FOV of the world model.\n",
//             "Press arrow down to increase the FOV of the world model."
//         )));
// }

pub fn setup_physics_and_debug(app: &mut App) {
    app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
        // Uncomment the next line to see collision boxes (helpful for debugging)
        // .add_plugins(RapierDebugRenderPlugin::default())
        .add_systems(Update, log_collisions);
}

pub fn log_collisions(mut collision_events: EventReader<CollisionEvent>) {
    for collision_event in collision_events.read() {
        match collision_event {
            CollisionEvent::Started(entity1, entity2, _) => {
                info!("Collision started between entities: {entity1:?} and {entity2:?}");
            }
            CollisionEvent::Stopped(entity1, entity2, _) => {
                info!("Collision stopped between entities: {entity1:?} and {entity2:?}");
            }
        }
    }
}
