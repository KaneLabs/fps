use bevy::log::info;
use bevy::render::render_graph::Node;
use bevy::{color::palettes::tailwind, prelude::*, render::view::RenderLayers};
use bevy_egui::{egui, EguiContexts};
use bevy_rapier3d::prelude::*;
use bevy::gltf::GltfAssetLabel;

use crate::entities::PictureFrameBundle;
use crate::entities::WorldObjectBundle;
use crate::network::ControlledPlayer;
use crate::player::{PlayerInput, VIEW_MODEL_RENDER_LAYER};

#[derive(Debug, Component)]
pub struct WorldModelCamera;

/// Used implicitly by all entities without a `RenderLayers` component.
/// Our world model camera and all objects other than the player are on this layer.
/// The light source belongs to both layers.
pub const DEFAULT_RENDER_LAYER: usize = 0;

#[derive(Component)]
pub struct ControlsUI; // Marker to track if UI exists

/// Component for items that can be equipped by the player
#[derive(Component)]
pub struct Equippable {
    pub name: String,
    pub model_path: String,
    pub interaction_distance: f32,
}

/// Component for the currently equipped item
#[derive(Component)]
pub struct EquippedItem {
    pub name: String,
}

/// Resource to track the player's equipped item
#[derive(Resource, Default)]
pub struct PlayerEquipment {
    pub equipped_item: Option<Entity>,
}

pub fn spawn_world_model(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
) {
    let floor = meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(10.0)));
    let wall = meshes.add(Cuboid::new(0.5, 4.0, 10.0));
    let material = materials.add(Color::WHITE);

    commands.spawn((
        Mesh3d(floor),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(0.0, 0.0, 0.0),
        WorldObjectBundle::floor(),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    info!("Spawned floor with collider at y=0");

    let table_size = Vec3::new(2.0, 1.0, 1.0);
    let cube = meshes.add(Cuboid::new(table_size.x, table_size.y, table_size.z));

    commands.spawn((
        Mesh3d(cube),
        MeshMaterial3d(materials.add(Color::from(tailwind::AMBER_700))),
        Transform::from_xyz(0.0, 0.0, -3.0),
        RigidBody::Fixed,
        Collider::cuboid(table_size.x, 2.0, table_size.z),
        Friction {
            coefficient: 0.0,
            combine_rule: CoefficientCombineRule::Min,
        },
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    // Load and spawn the GLTF model on the table
    // Using the approach from the Bevy documentation
    let model_handle = asset_server.load(GltfAssetLabel::Scene(0).from_asset("dirty-pickaxe.glb"));

    commands.spawn((
        SceneRoot(model_handle),
        Transform::from_xyz(0.0, 0.5, -3.0)
            .with_scale(Vec3::splat(1.8))
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
        Equippable {
            name: "Pickaxe".to_string(),
            model_path: "dirty-pickaxe.glb".to_string(),
            interaction_distance: 2.0,
        },
        // Add a collider for interaction detection
        Collider::cuboid(0.3, 0.1, 0.3),
        Sensor,
        Name::new("Pickaxe"),
    ));

    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(-5.0, 2.0, 0.0),
        WorldObjectBundle::wall(),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(5.0, 2.0, 0.0),
        WorldObjectBundle::wall(),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    commands.spawn((
        Mesh3d(wall.clone()),
        MeshMaterial3d(material.clone()),
        Transform::from_xyz(0.0, 2.0, 5.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        WorldObjectBundle::wall(),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    commands.spawn((
        Mesh3d(wall),
        MeshMaterial3d(material),
        Transform::from_xyz(0.0, 2.0, -5.0)
            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
        WorldObjectBundle::wall(),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));

    // Add some picture frames
    commands.spawn((
        PictureFrameBundle::new(
            &mut meshes,
            &mut materials,
            &asset_server,
            Vec3::new(0.0, 2.5, -4.9),
            Vec2::new(1.0, 1.0),
        ),
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
    ));
}

pub fn spawn_lights(mut commands: Commands) {
    // Ambient light with warmer color
    commands.insert_resource(AmbientLight {
        color: Color::rgb(1.0, 0.95, 0.9), // Slightly warm white
        brightness: 0.3,                   // Lower ambient for more contrast
    });

    // Main directional light with rose tint
    commands.spawn((
        DirectionalLightBundle {
            directional_light: DirectionalLight {
                illuminance: 10000.0,
                shadows_enabled: true,
                color: Color::from(tailwind::ROSE_300), // Warm rose color
                ..default()
            },
            transform: Transform::from_xyz(4.0, 8.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y),
            ..default()
        },
        // Light affects both world and view model
        RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER, VIEW_MODEL_RENDER_LAYER]),
    ));
}

pub fn setup_physics_and_debug(app: &mut App) {
    app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default())
        .add_plugins(RapierDebugRenderPlugin::default())
        .add_systems(Update, log_collisions);
}

pub fn log_collisions(mut collision_events: EventReader<CollisionEvent>) {
    for collision_event in collision_events.read() {
        match collision_event {
            CollisionEvent::Started(entity1, entity2, _) => {
                info!(
                    "Collision started between entities: {:?} and {:?}",
                    entity1, entity2
                );
            }
            CollisionEvent::Stopped(entity1, entity2, _) => {
                info!(
                    "Collision stopped between entities: {:?} and {:?}",
                    entity1, entity2
                );
            }
        }
    }
}

pub fn equip_item_system(
    mut commands: Commands,
    player_query: Query<(Entity, &Transform), With<ControlledPlayer>>,
    equippable_query: Query<(Entity, &Transform, &Equippable)>,
    player_input: Res<PlayerInput>,
    mut equipment: ResMut<PlayerEquipment>,
    asset_server: Res<AssetServer>,
    mut last_interact: Local<f32>,
    time: Res<Time>,
    mut client: ResMut<bevy_renet::renet::RenetClient>,
) {
    // Only process if the player pressed E and enough time has passed since last interaction
    if player_input.interact && time.elapsed_secs() - *last_interact > 0.5 {
        *last_interact = time.elapsed_secs();
        
        if let Ok((player_entity, player_transform)) = player_query.get_single() {
            // Check if player is already holding something
            if let Some(equipped_entity) = equipment.equipped_item {
                // Unequip the current item
                commands.entity(equipped_entity).despawn_recursive();
                equipment.equipped_item = None;
                info!("Unequipped item");
                
                // Send unequip message to server
                if client.is_connected() {
                    let input = crate::network::ClientInput::UnequipItem;
                    let message = bincode::serialize(&input).unwrap();
                    client.send_message(crate::network::ClientChannel::Command, message);
                }
                
                return;
            }
            
            // Find the closest equippable item within interaction distance
            let mut closest_item = None;
            let mut closest_distance = f32::MAX;
            
            for (entity, transform, equippable) in equippable_query.iter() {
                let distance = player_transform.translation.distance(transform.translation);
                if distance <= equippable.interaction_distance && distance < closest_distance {
                    closest_distance = distance;
                    closest_item = Some((entity, equippable.name.clone(), equippable.model_path.clone()));
                }
            }
            
            // Process the closest item if found
            if let Some((equippable_entity, name, model_path)) = closest_item {
                info!("Equipping {}", name);
                
                // Load the model for the view model
                let model_handle = asset_server.load(GltfAssetLabel::Scene(0).from_asset(model_path.clone()));
                
                // Spawn the view model as a child of the player
                let view_model_entity = commands.spawn((
                    SceneRoot(model_handle),
                    Transform::from_xyz(0.4, -0.3, -0.5)
                        .with_scale(Vec3::splat(0.5))
                        .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
                    RenderLayers::layer(VIEW_MODEL_RENDER_LAYER),
                    EquippedItem {
                        name: name.clone(),
                    },
                )).id();
                
                // Add the view model as a child of the player
                commands.entity(player_entity).add_child(view_model_entity);
                
                // Update the equipment resource
                equipment.equipped_item = Some(view_model_entity);
                
                // Hide the world model of the item
                commands.entity(equippable_entity).insert(Visibility::Hidden);
                
                // Send equip message to server
                if client.is_connected() {
                    let input = crate::network::ClientInput::EquipItem {
                        item_entity: equippable_entity,
                    };
                    let message = bincode::serialize(&input).unwrap();
                    client.send_message(crate::network::ClientChannel::Command, message);
                }
            }
        }
    }
}
