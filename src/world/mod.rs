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

/// Component for objects that can be interacted with using tools
#[derive(Component)]
pub struct Interactable {
    pub required_tool: Option<String>, // "Pickaxe", etc.
    pub interaction_distance: f32,
    pub interaction_time: f32, // For actions that take time
    pub interaction_progress: f32, // Current progress (0.0 to interaction_time)
}

impl Default for Interactable {
    fn default() -> Self {
        Self {
            required_tool: None,
            interaction_distance: 2.0,
            interaction_time: 1.0,
            interaction_progress: 0.0,
        }
    }
}

/// Resource to track interaction progress
#[derive(Resource, Default)]
pub struct InteractionState {
    pub current_target: Option<Entity>,
    pub progress: f32,
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
    
    // Add an ore block that can be mined with the pickaxe
    let ore_model_handle = asset_server.load(GltfAssetLabel::Scene(0).from_asset("ore_chunk.glb"));

    commands.spawn((
        SceneRoot(ore_model_handle),
        Transform::from_xyz(2.0, 0.5, -3.0)
            .with_scale(Vec3::splat(1.0)),
        RigidBody::Fixed,
        Collider::cuboid(0.25, 0.25, 0.25),
        Name::new("Ore Block"),
        Interactable {
            required_tool: Some("Pickaxe".to_string()),
            interaction_distance: 2.0,
            interaction_time: 3.0, // Takes 3 seconds to mine
            interaction_progress: 0.0,
        },
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
    visibility_query: Query<&Visibility>,
) {
    // Only process if the player pressed E and enough time has passed since last interaction
    if player_input.interact && time.elapsed_secs() - *last_interact > 0.5 {
        *last_interact = time.elapsed_secs();
        
        if let Ok((player_entity, player_transform)) = player_query.get_single() {
            // Check if player is already holding something
            if let Some(equipped_entity) = equipment.equipped_item {
                // Find the item entity that was equipped
                let mut item_entity = None;
                
                // Find the world model of the equipped item
                for (entity, _, equippable) in equippable_query.iter() {
                    if let Ok(visibility) = visibility_query.get(entity) {
                        if *visibility == Visibility::Hidden {
                            item_entity = Some((entity, equippable.name.clone()));
                            break;
                        }
                    }
                }
                
                // Unequip the current item
                commands.entity(equipped_entity).despawn_recursive();
                equipment.equipped_item = None;
                
                // Drop the item to the floor
                if let Some((entity, name)) = item_entity {
                    info!("Unequipped and dropped {} to the floor", name);
                    
                    // Get player position to drop the item nearby
                    let drop_position = player_transform.translation + player_transform.forward() * 1.0;
                    
                    // Make the item visible again and update its position
                    commands.entity(entity).insert((
                        Visibility::Visible,
                        Transform::from_translation(drop_position)
                            .with_scale(Vec3::splat(1.8))
                            .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)),
                    ));
                    
                    // Send unequip message to server
                    if client.is_connected() {
                        let input = crate::network::ClientInput::UnequipItem;
                        let message = bincode::serialize(&input).unwrap();
                        client.send_message(crate::network::ClientChannel::Input, message);
                    }
                } else {
                    info!("Unequipped item");
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
                    client.send_message(crate::network::ClientChannel::Input, message);
                }
            }
        }
    }
}

/// System to handle tool-based interactions (like mining ore with pickaxe)
pub fn tool_interaction_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    player_query: Query<(Entity, &Transform), With<ControlledPlayer>>,
    equipped_items_query: Query<&EquippedItem>,
    mut interactables_query: Query<(Entity, &Transform, &mut Interactable)>,
    mouse_input: Res<ButtonInput<MouseButton>>,
    equipment: Res<PlayerEquipment>,
    mut interaction_state: ResMut<InteractionState>,
    time: Res<Time>,
    asset_server: Res<AssetServer>,
) {
    // Only process if the player is holding the left mouse button
    if mouse_input.pressed(MouseButton::Left) {
        if let Ok((player_entity, player_transform)) = player_query.get_single() {
            // Check if player has an equipped item
            let equipped_tool = if let Some(equipped_entity) = equipment.equipped_item {
                equipped_items_query.get(equipped_entity).ok().map(|item| item.name.clone())
            } else {
                None
            };
            
            // Find the closest interactable within range
            let mut closest_interactable = None;
            let mut closest_distance = f32::MAX;
            
            for (entity, transform, interactable) in interactables_query.iter_mut() {
                let distance = player_transform.translation.distance(transform.translation);
                
                // Check if within interaction distance and requires the equipped tool (or no tool)
                if distance <= interactable.interaction_distance && 
                   (interactable.required_tool.is_none() || 
                    interactable.required_tool.as_ref() == equipped_tool.as_ref()) {
                    if distance < closest_distance {
                        closest_distance = distance;
                        closest_interactable = Some(entity);
                    }
                }
            }
            
            // Process interaction with the closest interactable
            if let Some(interactable_entity) = closest_interactable {
                // If we're already interacting with this entity, continue the interaction
                if interaction_state.current_target == Some(interactable_entity) {
                    if let Ok((_, transform, mut interactable)) = interactables_query.get_mut(interactable_entity) {
                        // Increase progress
                        interactable.interaction_progress += time.delta_secs();
                        interaction_state.progress = interactable.interaction_progress;
                        
                        // Check if interaction is complete
                        if interactable.interaction_progress >= interactable.interaction_time {
                            info!("Interaction complete with entity {:?}", interactable_entity);
                            
                            // Get the position before dropping the mutable borrow
                            let spawn_position = transform.translation;
                            
                            // Reset progress
                            interactable.interaction_progress = 0.0;
                            interaction_state.current_target = None;
                            interaction_state.progress = 0.0;
                            
                            // Load the ore chunk model
                            let model_handle = asset_server.load(GltfAssetLabel::Scene(0).from_asset("ore_chunk.glb"));
                            
                            // Spawn the ore chunk with the model
                            commands.spawn((
                                SceneRoot(model_handle),
                                Transform::from_translation(spawn_position + Vec3::new(0.0, 0.3, 0.0))
                                    .with_scale(Vec3::splat(0.5)),
                                RigidBody::Dynamic,
                                Collider::cuboid(0.1, 0.1, 0.1),
                                Name::new("Ore Chunk"),
                                Equippable {
                                    name: "Ore Chunk".to_string(),
                                    model_path: "ore_chunk.glb".to_string(),
                                    interaction_distance: 2.0,
                                },
                                RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
                            ));
                        }
                    }
                } else {
                    // Start new interaction
                    interaction_state.current_target = Some(interactable_entity);
                    interaction_state.progress = 0.0;
                    
                    if let Ok((_, _, mut interactable)) = interactables_query.get_mut(interactable_entity) {
                        interactable.interaction_progress = 0.0;
                    }
                    
                    info!("Started interaction with entity {:?}", interactable_entity);
                }
            }
        }
    } else {
        // Reset interaction state when not interacting
        if interaction_state.current_target.is_some() {
            if let Some(entity) = interaction_state.current_target {
                if let Ok((_, _, mut interactable)) = interactables_query.get_mut(entity) {
                    interactable.interaction_progress = 0.0;
                }
            }
            
            interaction_state.current_target = None;
            interaction_state.progress = 0.0;
        }
    }
}

pub fn interaction_ui_system(
    mut contexts: EguiContexts,
    interaction_state: Res<InteractionState>,
    interactables_query: Query<&Interactable>,
) {
    if let Some(target) = interaction_state.current_target {
        if interaction_state.progress > 0.0 {
            // Get the interactable to determine the max time
            let max_time = if let Ok(interactable) = interactables_query.get(target) {
                interactable.interaction_time
            } else {
                3.0 // Default fallback
            };
            
            // Get screen dimensions
            let screen_rect = contexts.ctx_mut().screen_rect();
            
            // Create a small panel at the bottom of the screen
            egui::Window::new("Mining Progress")
                .title_bar(false)
                .resizable(false)
                .collapsible(false)
                .fixed_pos(egui::pos2(
                    screen_rect.width() / 2.0 - 100.0, 
                    screen_rect.height() - 70.0
                ))
                .fixed_size(egui::vec2(200.0, 50.0))
                .show(contexts.ctx_mut(), |ui| {
                    // Show percentage
                    let percent = (interaction_state.progress / max_time * 100.0) as i32;
                    ui.label(format!("Mining... {}%", percent));
                    
                    // Add progress bar
                    ui.add(egui::ProgressBar::new(interaction_state.progress / max_time)
                        .desired_width(200.0));
                });
        }
    }
}
