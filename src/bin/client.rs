use bevy::color::palettes::tailwind;
use bevy::pbr::NotShadowCaster;
use bevy::render::view::RenderLayers;
use bevy::window::PrimaryWindow;
use bevy::{
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    prelude::Vec3,
    prelude::*,
    gltf::GltfAssetLabel,
};
use bevy_egui::EguiPlugin;
use bevy_egui::{egui, EguiContexts};
use bevy_rapier3d::plugin::{NoUserData, RapierPhysicsPlugin};
use bevy_rapier3d::prelude::{
    Ccd, CoefficientCombineRule, Collider, Damping, Friction, GravityScale,
    KinematicCharacterController, LockedAxes, Restitution, RigidBody, Sensor,
};
use bevy_renet::renet::ClientId;
use bevy_renet::{client_connected, renet::RenetClient, RenetClientPlugin};
use multiplayer::bot::Velocity;
use multiplayer::entities::PlayerBundle;
use multiplayer::network::{
    ClientChannel, ClientLobby, ControlledPlayer, CurrentClientId, NetworkMapping, PlayerInfo,
    ServerChannel,
};
use multiplayer::player::{
    change_fov, grab_mouse, handle_interaction, move_player, move_player_body, player_input,
    spawn_view_model, CameraSensitivity, CursorState, Player, VIEW_MODEL_RENDER_LAYER,
};
use multiplayer::world::{
    equip_item_system, interaction_ui_system, spawn_lights, spawn_world_model, InteractionState, 
    PlayerEquipment, WorldModelCamera, DEFAULT_RENDER_LAYER, tool_interaction_system,
};
use multiplayer::{
    network::{connection_config, NetworkedEntities, ServerMessages},
    player::{PlayerCommand, PlayerInput},
};

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Connected;

#[cfg(feature = "netcode")]
fn add_netcode_network(app: &mut App) {
    use bevy_renet::netcode::{
        ClientAuthentication, NetcodeClientPlugin, NetcodeClientTransport, NetcodeTransportError,
    };
    use multiplayer::PROTOCOL_ID;
    use std::{net::UdpSocket, time::SystemTime};

    app.add_plugins(NetcodeClientPlugin);

    app.configure_sets(Update, Connected.run_if(client_connected));

    let client = RenetClient::new(connection_config());

    let server_addr = "127.0.0.1:5000".parse().unwrap();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let client_id = current_time.as_millis() as ClientId;
    let authentication = ClientAuthentication::Unsecure {
        client_id,
        protocol_id: PROTOCOL_ID,
        server_addr,
        user_data: None,
    };

    let transport = NetcodeClientTransport::new(current_time, authentication, socket).unwrap();

    app.insert_resource(client);
    app.insert_resource(transport);
    app.insert_resource(CurrentClientId(client_id));

    // If any error is found we just panic
    #[allow(clippy::never_loop)]
    fn panic_on_error_system(mut renet_error: EventReader<NetcodeTransportError>) {
        for e in renet_error.read() {
            panic!("{}", e);
        }
    }

    app.add_systems(Update, panic_on_error_system);
}

#[cfg(feature = "steam")]
fn add_steam_network(app: &mut App) {
    use bevy_renet::steam::{SteamClientPlugin, SteamClientTransport, SteamTransportError};
    use steamworks::{SingleClient, SteamId};

    let (steam_client, single) = steamworks::Client::init_app(480).unwrap();

    steam_client.networking_utils().init_relay_network_access();

    let args: Vec<String> = std::env::args().collect();
    let server_steam_id: u64 = args[1].parse().unwrap();
    let server_steam_id = SteamId::from_raw(server_steam_id);

    let client = RenetClient::new(connection_config());
    let transport = SteamClientTransport::new(&steam_client, &server_steam_id).unwrap();

    app.add_plugins(SteamClientPlugin);
    app.insert_resource(client);
    app.insert_resource(transport);
    app.insert_resource(CurrentClientId(
        steam_client.user().steam_id().raw() as ClientId
    ));

    app.configure_sets(Update, Connected.run_if(client_connected));

    app.insert_non_send_resource(single);
    fn steam_callbacks(client: NonSend<SingleClient>) {
        client.run_callbacks();
    }

    app.add_systems(PreUpdate, steam_callbacks);

    // If any error is found we just panic
    #[allow(clippy::never_loop)]
    fn panic_on_error_system(mut renet_error: EventReader<SteamTransportError>) {
        for e in renet_error.read() {
            panic!("{}", e);
        }
    }

    app.add_systems(Update, panic_on_error_system);
}

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    app.add_plugins(RenetClientPlugin);
    app.add_plugins(FrameTimeDiagnosticsPlugin);
    app.add_plugins(LogDiagnosticsPlugin::default());
    app.add_plugins(EguiPlugin);

    #[cfg(feature = "netcode")]
    add_netcode_network(&mut app);

    #[cfg(feature = "steam")]
    add_steam_network(&mut app);

    app.add_event::<PlayerCommand>();

    app.insert_resource(ClientLobby::default());
    app.insert_resource(PlayerInput::default());
    app.insert_resource(NetworkMapping::default());

    app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default());

    app.add_systems(Startup, spawn_world_model);
    app.add_systems(Startup, spawn_lights);

    app.insert_resource(CursorState::default());
    app.insert_resource(PlayerEquipment::default());
    app.insert_resource(InteractionState::default());

    app.configure_sets(Update, Connected.run_if(client_connected));

    app.add_systems(
        Update,
        (
            player_input,
            move_player,
            move_player_body,
            grab_mouse,
            change_fov,
            handle_interaction,
            equip_item_system,
            tool_interaction_system,
            interaction_ui_system,
        ),
    );

    app.add_systems(Update, (client_sync_players).in_set(Connected));

    app.run();
}

fn client_sync_players(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut client: ResMut<RenetClient>,
    client_id: Res<CurrentClientId>,
    mut lobby: ResMut<ClientLobby>,
    mut network_mapping: ResMut<NetworkMapping>,
    controlled_players: Query<Entity, With<ControlledPlayer>>,
    children_query: Query<&Children>,
    names: Query<&Name>,
    asset_server: Res<AssetServer>,
) {
    if !client.is_connected() {
        return;
    }

    let client_id = client_id.0;
    while let Some(message) = client.receive_message(ServerChannel::ServerMessages) {
        let server_message = bincode::deserialize(&message).unwrap();
        match server_message {
            ServerMessages::PlayerCreate {
                id,
                translation,
                entity,
            } => {
                info!(
                    "Player Connected: {}, Entity: {:?}, Is Local: {}, Current Client ID: {}",
                    id,
                    entity,
                    id == client_id,
                    client_id
                );
                println!("Player {} connected.", id);

                // If this is our player, we spawn the FPS view model
                if client_id == id {
                    let arm = meshes.add(Cuboid::new(0.1, 0.1, 0.5));
                    let arm_material = materials.add(Color::from(tailwind::TEAL_200));

                    let player_entity = commands
                        .spawn(PlayerBundle::new(
                            client_id,
                            Transform::from_xyz(0.0, 2.0, 0.0),
                            &mut meshes,
                            &mut materials,
                        ))
                        .insert(ControlledPlayer)
                        .with_children(|parent| {
                            // World model camera (sees layer 0)
                            parent.spawn((
                                WorldModelCamera,
                                Camera3d::default(),
                                Projection::from(PerspectiveProjection {
                                    fov: 90.0_f32.to_radians(),
                                    ..default()
                                }),
                            ));

                            // Spawn view model camera.
                            parent.spawn((
                                Camera3d::default(),
                                Camera {
                                    // Bump the order to render on top of the world model.
                                    order: 1,
                                    ..default()
                                },
                                Projection::from(PerspectiveProjection {
                                    fov: 70.0_f32.to_radians(),
                                    ..default()
                                }),
                                // Only render objects belonging to the view model.
                                RenderLayers::layer(VIEW_MODEL_RENDER_LAYER),
                            ));

                            // Player's arm
                            parent.spawn((
                                Mesh3d(arm),
                                MeshMaterial3d(arm_material),
                                Transform::from_xyz(0.2, -0.1, -0.25),
                                RenderLayers::layer(VIEW_MODEL_RENDER_LAYER),
                                NotShadowCaster,
                            ));
                        })
                        .id();

                    // Add to lobby/mapping
                    let player_info = PlayerInfo {
                        server_entity: entity,
                        client_entity: player_entity,
                    };
                    lobby.players.insert(id, player_info);
                    network_mapping.0.insert(entity, player_entity);
                } else {
                    // For other players, spawn with matching collider but as sensor
                    let client_entity = commands
                        .spawn((
                            Mesh3d(meshes.add(Mesh::from(Capsule3d::default()))),
                            MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
                            Transform::from_translation(Vec3::from(translation)),
                            Collider::capsule(
                                Vec3::new(0.0, 0.5, 0.0),
                                Vec3::new(0.0, 1.5, 0.0),
                                0.5,
                            ),
                            Sensor,
                            RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
                        ))
                        .id();

                    info!(
                        "Client Mappings - ID: {}, Server Entity: {:?}, Client Entity: {:?}, Network Map Size: {}",
                        id,
                        entity,
                        client_entity,
                        network_mapping.0.len()
                    );

                    let player_info = PlayerInfo {
                        server_entity: entity,
                        client_entity,
                    };
                    lobby.players.insert(id, player_info);
                    network_mapping.0.insert(entity, client_entity);
                }

                info!(
                    "Player spawn - ID: {}, Is Local: {}, Position: {:?}, Layer: {}",
                    id,
                    client_id == id,
                    translation,
                    if client_id == id { "Local" } else { "Remote" }
                );
            }
            ServerMessages::PlayerRemove { id } => {
                println!("Player {} disconnected.", id);
                if let Some(PlayerInfo {
                    server_entity,
                    client_entity,
                }) = lobby.players.remove(&id)
                {
                    commands.entity(client_entity).despawn();
                    network_mapping.0.remove(&server_entity);
                }
            }
            ServerMessages::SpawnProjectile {
                entity,
                translation,
            } => {
                let projectile_entity = commands.spawn((
                    Mesh3d(meshes.add(Mesh::from(Sphere::new(0.1)))),
                    MeshMaterial3d(materials.add(Color::srgb(1.0, 0.0, 0.0))),
                    Transform::from_translation(translation.into()),
                ));
                network_mapping.0.insert(entity, projectile_entity.id());
            }
            ServerMessages::DespawnProjectile { entity } => {
                if let Some(entity) = network_mapping.0.remove(&entity) {
                    commands.entity(entity).despawn();
                }
            }
            ServerMessages::EquipItem { player_id, item_entity, item_name, item_model } => {
                info!("Player {} equipped item: {}", player_id, item_name);
                
                // If this is another player, update their visual state
                if player_id != client_id {
                    if let Some(player_info) = lobby.players.get(&player_id) {
                        // Hide the world model of the item
                        if let Some(world_item) = network_mapping.0.get(&item_entity) {
                            commands.entity(*world_item).insert(Visibility::Hidden);
                        }
                        
                        // Add a view model to the other player (simplified version)
                        let model_handle = asset_server.load(GltfAssetLabel::Scene(0).from_asset(item_model));
                        commands.entity(player_info.client_entity).with_children(|parent| {
                            parent.spawn((
                                SceneRoot(model_handle),
                                Transform::from_xyz(0.4, -0.3, -0.5)
                                    .with_scale(Vec3::splat(0.5)),
                                RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
                                Name::new(format!("Equipped_{}", item_name)),
                            ));
                        });
                    }
                }
            }
            ServerMessages::UnequipItem { player_id } => {
                info!("Player {} unequipped item", player_id);
                
                // If this is another player, update their visual state
                if player_id != client_id {
                    if let Some(player_info) = lobby.players.get(&player_id) {
                        // Find and remove any equipped items from this player
                        if let Ok(children) = children_query.get(player_info.client_entity) {
                            for child in children.iter() {
                                if let Ok(name) = names.get(*child) {
                                    if name.as_str().starts_with("Equipped_") {
                                        commands.entity(*child).despawn_recursive();
                                    }
                                }
                            }
                        }
                        
                        // The server will handle making the world item visible again and updating its position
                    }
                }
            }
        }
    }

    while let Some(message) = client.receive_message(ServerChannel::NetworkedEntities) {
        let networked_entities: NetworkedEntities = bincode::deserialize(&message).unwrap();

        for i in 0..networked_entities.entities.len() {
            if let Some(entity) = network_mapping.0.get(&networked_entities.entities[i]) {
                // Skip updates for our controlled player
                if let Some(player_info) = lobby.players.get(&client_id) {
                    if player_info.client_entity == *entity {
                        continue;
                    }
                }

                if let Some(mut cmd_entity) = commands.get_entity(*entity) {
                    let translation = networked_entities.translations[i].into();
                    let rotation = Quat::from_array(networked_entities.rotations[i]);

                    cmd_entity.insert(Transform {
                        translation,
                        rotation,
                        ..Default::default()
                    });
                }
            }
        }
    }
}
