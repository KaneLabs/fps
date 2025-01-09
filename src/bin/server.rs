use bevy::{
    diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin},
    input::mouse::MouseMotion,
    prelude::*,
    render::view::RenderLayers,
};
use bevy_egui::EguiPlugin;
use bevy_rapier3d::{
    pipeline::CollisionEvent,
    plugin::{NoUserData, RapierPhysicsPlugin},
    render::RapierDebugRenderPlugin,
};
use bevy_renet::{
    renet::{RenetServer, ServerEvent},
    RenetServerPlugin,
};
use multiplayer::{
    bot::{spawn_fireball, BotId, Projectile, Velocity},
    entities::PlayerBundle,
    network::{
        ClientChannel, ClientInput, NetworkedEntities, ServerChannel, ServerLobby, ServerMessages,
    },
    player::{Player, PlayerCommand, PlayerInput, PLAYER_MOVE_SPEED},
    world::{spawn_lights, spawn_world_model, DEFAULT_RENDER_LAYER},
};
use renet_visualizer::RenetServerVisualizer;

pub const DEBUG_FLYCAM_LAYER: usize = 2;

#[cfg(feature = "netcode")]
fn add_netcode_network(app: &mut App) {
    use bevy_renet::netcode::{
        NetcodeServerPlugin, NetcodeServerTransport, ServerAuthentication, ServerConfig,
    };
    use multiplayer::{network::connection_config, PROTOCOL_ID};
    use std::{net::UdpSocket, time::SystemTime};

    app.add_plugins(NetcodeServerPlugin);

    let server = RenetServer::new(connection_config());

    let public_addr = "127.0.0.1:5000".parse().unwrap();
    let socket = UdpSocket::bind(public_addr).unwrap();
    let current_time: std::time::Duration = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let server_config = ServerConfig {
        current_time,
        max_clients: 64,
        protocol_id: PROTOCOL_ID,
        public_addresses: vec![public_addr],
        authentication: ServerAuthentication::Unsecure,
    };

    let transport = NetcodeServerTransport::new(server_config, socket).unwrap();
    app.insert_resource(server);
    app.insert_resource(transport);
}

#[cfg(feature = "steam")]
fn add_steam_network(app: &mut App) {
    use bevy_renet::steam::{
        AccessPermission, SteamServerConfig, SteamServerPlugin, SteamServerTransport,
    };
    use multiplayer::connection_config;
    use steamworks::SingleClient;

    let (steam_client, single) = steamworks::Client::init_app(480).unwrap();

    let server: RenetServer = RenetServer::new(connection_config());

    let steam_transport_config = SteamServerConfig {
        max_clients: 10,
        access_permission: AccessPermission::Public,
    };
    let transport = SteamServerTransport::new(&steam_client, steam_transport_config).unwrap();

    app.add_plugins(SteamServerPlugin);
    app.insert_resource(server);
    app.insert_non_send_resource(transport);
    app.insert_non_send_resource(single);

    fn steam_callbacks(client: NonSend<SingleClient>) {
        client.run_callbacks();
    }

    app.add_systems(PreUpdate, steam_callbacks);
}

#[derive(Resource)]
struct DebugCameraState {
    cursor_grabbed: bool,
}

#[derive(Resource)]
struct NetworkSyncTimer(Timer);

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);

    app.add_plugins(RenetServerPlugin);
    app.add_plugins(FrameTimeDiagnosticsPlugin);
    app.add_plugins(LogDiagnosticsPlugin::default());
    app.add_plugins(EguiPlugin);

    app.insert_resource(ServerLobby::default());
    app.insert_resource(BotId(0));

    app.insert_resource(RenetServerVisualizer::<200>::default());

    #[cfg(feature = "netcode")]
    add_netcode_network(&mut app);

    #[cfg(feature = "steam")]
    add_steam_network(&mut app);

    app.add_plugins(RapierPhysicsPlugin::<NoUserData>::default());
    app.add_plugins(RapierDebugRenderPlugin {
        mode: bevy_rapier3d::render::DebugRenderMode::COLLIDER_SHAPES,
        ..default()
    });

    app.add_systems(
        Update,
        (
            server_update_system,
            server_network_sync,
            move_players_system,
            update_projectiles_system,
        ),
    );

    app.add_systems(PostUpdate, projectile_on_removal_system);

    app.add_systems(
        Startup,
        (spawn_world_model, setup_debug_camera, spawn_lights).chain(),
    );

    app.add_event::<CollisionEvent>();

    app.add_systems(Update, log_physics_events);

    app.add_systems(Startup, setup_debug_controls);

    app.add_systems(Update, handle_debug_controls);

    app.add_systems(Update, debug_camera_look);

    app.insert_resource(DebugCameraState {
        cursor_grabbed: true,
    });

    app.insert_resource(NetworkSyncTimer(Timer::from_seconds(
        0.1,
        TimerMode::Repeating,
    )));

    app.run();
}

#[allow(clippy::too_many_arguments)]
fn server_update_system(
    mut server_events: EventReader<ServerEvent>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut lobby: ResMut<ServerLobby>,
    mut server: ResMut<RenetServer>,
    players: Query<(Entity, &Player, &Transform)>,
) {
    for event in server_events.read() {
        match event {
            ServerEvent::ClientConnected { client_id } => {
                info!("Server: New client connected with ID: {}", client_id);

                // Log existing players
                for (entity, player, transform) in players.iter() {
                    info!(
                        "Server: Existing player - ID: {}, Entity: {:?}, Position: {:?}",
                        player.id, entity, transform.translation
                    );
                }

                // Initialize other players for this new client
                for (entity, player, transform) in players.iter() {
                    let translation: [f32; 3] = transform.translation.into();
                    info!(
                        "Server: Sending existing player {} to new client {}",
                        player.id, client_id
                    );
                    let message = bincode::serialize(&ServerMessages::PlayerCreate {
                        id: player.id,
                        entity,
                        translation,
                    })
                    .unwrap();
                    server.send_message(*client_id, ServerChannel::ServerMessages, message);
                }

                // Spawn new player
                let transform = Transform::from_xyz(0.0, 2.0, 0.0);
                let player_entity = commands
                    .spawn(PlayerBundle::new(
                        *client_id,
                        transform,
                        &mut meshes,
                        &mut materials,
                    ))
                    .id();

                info!(
                    "Server: Spawned new player - ID: {}, Entity: {:?}, Position: {:?}",
                    client_id, player_entity, transform.translation
                );

                lobby.players.insert(*client_id, player_entity);

                // Broadcast new player to all clients
                let translation: [f32; 3] = transform.translation.into();
                let message = bincode::serialize(&ServerMessages::PlayerCreate {
                    id: *client_id,
                    entity: player_entity,
                    translation,
                })
                .unwrap();
                server.broadcast_message(ServerChannel::ServerMessages, message);

                info!(
                    "Server Mappings - Client ID: {}, Entity: {:?}, Lobby Size: {}",
                    client_id,
                    player_entity,
                    lobby.players.len()
                );
            }
            ServerEvent::ClientDisconnected { client_id, reason } => {
                println!("Player {} disconnected: {}", client_id, reason);
                if let Some(player_entity) = lobby.players.remove(client_id) {
                    commands.entity(player_entity).despawn();
                }

                let message =
                    bincode::serialize(&ServerMessages::PlayerRemove { id: *client_id }).unwrap();
                server.broadcast_message(ServerChannel::ServerMessages, message);
            }
        }
    }

    for client_id in server.clients_id() {
        while let Some(message) = server.receive_message(client_id, ClientChannel::Command) {
            let command: PlayerCommand = bincode::deserialize(&message).unwrap();
            match command {
                PlayerCommand::BasicAttack { mut cast_at } => {
                    println!(
                        "Received basic attack from client {}: {:?}",
                        client_id, cast_at
                    );

                    if let Some(player_entity) = lobby.players.get(&client_id) {
                        if let Ok((_, _, player_transform)) = players.get(*player_entity) {
                            cast_at[1] = player_transform.translation[1];

                            let direction =
                                (cast_at - player_transform.translation).normalize_or_zero();
                            let mut translation = player_transform.translation + (direction * 0.7);
                            translation[1] = 1.0;

                            let fireball_entity = spawn_fireball(
                                &mut commands,
                                &mut meshes,
                                &mut materials,
                                translation,
                                direction,
                            );
                            let message = ServerMessages::SpawnProjectile {
                                entity: fireball_entity,
                                translation: translation.into(),
                            };
                            let message = bincode::serialize(&message).unwrap();
                            server.broadcast_message(ServerChannel::ServerMessages, message);
                        }
                    }
                }
            }
        }
        while let Some(message) = server.receive_message(client_id, ClientChannel::Input) {
            if let Some(player_entity) = lobby.players.get(&client_id) {
                match bincode::deserialize(&message).unwrap() {
                    ClientInput::Movement(input) => {
                        commands.entity(*player_entity).insert(input);
                    }
                    ClientInput::Rotation(rotation) => {
                        if let Ok((entity, _, transform)) = players.get(*player_entity) {
                            commands.entity(entity).insert(Transform {
                                rotation,
                                ..transform.clone()
                            });
                        }
                    }
                    ClientInput::Position(position) => {
                        // Optionally validate position before applying
                        if let Ok((entity, _, transform)) = players.get(*player_entity) {
                            commands.entity(entity).insert(Transform {
                                translation: position,
                                ..transform.clone()
                            });
                        }
                    }
                    ClientInput::Interact => {
                        info!("Player {} tried to interact", client_id);
                        // Handle interaction (we can add specific interaction logic later)
                    }
                }
            }
        }
    }
}

fn update_projectiles_system(
    mut commands: Commands,
    mut projectiles: Query<(Entity, &mut Projectile)>,
    time: Res<Time>,
) {
    for (entity, mut projectile) in projectiles.iter_mut() {
        projectile.duration.tick(time.delta());
        if projectile.duration.finished() {
            commands.entity(entity).despawn();
        }
    }
}

#[allow(clippy::type_complexity)]
fn server_network_sync(
    mut server: ResMut<RenetServer>,
    query: Query<(Entity, &Transform), Or<(With<Player>, With<Projectile>)>>,
    time: Res<Time>,
    mut sync_timer: ResMut<NetworkSyncTimer>,
) {
    if !sync_timer.0.tick(time.delta()).just_finished() {
        return;
    }

    let mut networked_entities = NetworkedEntities::default();
    for (entity, transform) in query.iter() {
        networked_entities.entities.push(entity);
        networked_entities
            .translations
            .push(transform.translation.into());
        networked_entities.rotations.push(transform.rotation.into());
    }

    if !networked_entities.entities.is_empty() {
        let sync_message = bincode::serialize(&networked_entities).unwrap();
        server.broadcast_message(ServerChannel::NetworkedEntities, sync_message);
    }
}

fn move_players_system(
    mut query: Query<
        (
            &mut Transform,
            &PlayerInput,
            &mut Velocity,
            &mut bevy_rapier3d::prelude::Velocity,
        ),
        With<Player>,
    >,
    time: Res<Time>,
) {
    for (mut transform, input, mut game_vel, mut physics_vel) in query.iter_mut() {
        let x = (input.right as i8 - input.left as i8) as f32;
        let z = (input.down as i8 - input.up as i8) as f32;

        if x != 0.0 || z != 0.0 {
            let forward = transform.forward();
            let right = transform.right();

            let movement = (forward * -z + right * x).normalize() * PLAYER_MOVE_SPEED;

            // Update velocities
            game_vel.0.x = movement.x;
            game_vel.0.z = movement.z;
            physics_vel.linvel.x = movement.x;
            physics_vel.linvel.z = movement.z;
        } else {
            game_vel.0.x = 0.0;
            game_vel.0.z = 0.0;
            physics_vel.linvel.x = 0.0;
            physics_vel.linvel.z = 0.0;
        }
    }
}

#[derive(Component)]
struct DebugCamera;

pub fn setup_debug_camera(mut commands: Commands) {
    commands.spawn((
        Camera3dBundle {
            transform: Transform::from_xyz(-20.5, 30.0, 20.5).looking_at(Vec3::ZERO, Vec3::Y),
            camera: Camera {
                order: 1,
                ..default()
            },
            ..default()
        },
        DebugCamera,
        RenderLayers::from_layers(&[DEBUG_FLYCAM_LAYER, DEFAULT_RENDER_LAYER]),
    ));
}

fn projectile_on_removal_system(
    mut server: ResMut<RenetServer>,
    mut removed_projectiles: RemovedComponents<Projectile>,
) {
    for entity in removed_projectiles.read() {
        let message = ServerMessages::DespawnProjectile { entity };
        let message = bincode::serialize(&message).unwrap();

        server.broadcast_message(ServerChannel::ServerMessages, message);
    }
}

fn log_physics_events(
    mut collision_events: EventReader<CollisionEvent>,
    names: Query<&Name>,
    transforms: Query<&Transform>,
) {
    for collision_event in collision_events.read() {
        match collision_event {
            CollisionEvent::Started(e1, e2, _) => {
                let name1 = names.get(*e1).map(|n| n.as_str()).unwrap_or("Unknown");
                let name2 = names.get(*e2).map(|n| n.as_str()).unwrap_or("Unknown");
                let pos1 = transforms
                    .get(*e1)
                    .map(|t| t.translation)
                    .unwrap_or_default();
                let pos2 = transforms
                    .get(*e2)
                    .map(|t| t.translation)
                    .unwrap_or_default();
                info!(
                    "Collision started: {} at {:?} <-> {} at {:?}",
                    name1, pos1, name2, pos2
                );
            }
            CollisionEvent::Stopped(e1, e2, _) => {
                let name1 = names.get(*e1).map(|n| n.as_str()).unwrap_or("Unknown");
                let name2 = names.get(*e2).map(|n| n.as_str()).unwrap_or("Unknown");
                info!("Collision stopped: {} <-> {}", name1, name2);
            }
        }
    }
}

fn setup_debug_controls(mut windows: Query<&mut Window>) {
    let mut window = windows.single_mut();
    window.cursor_options.grab_mode = bevy::window::CursorGrabMode::Locked;
    window.cursor_options.visible = false;

    info!("Debug Camera Controls:");
    info!("  WASD - Move");
    info!("  Space/LShift - Up/Down");
    info!("  Mouse - Look around");
    info!("  Mouse Wheel - Adjust speed");
    info!("  Esc - Release/grab mouse");
}

fn handle_debug_controls(
    mut windows: Query<&mut Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    key: Res<ButtonInput<KeyCode>>,
    mut camera_state: ResMut<DebugCameraState>,
) {
    let mut window = windows.single_mut();

    if key.just_pressed(KeyCode::Escape) {
        camera_state.cursor_grabbed = false;
        window.cursor_options.grab_mode = bevy::window::CursorGrabMode::None;
        window.cursor_options.visible = true;
    }

    if mouse.just_pressed(MouseButton::Left) && !camera_state.cursor_grabbed {
        camera_state.cursor_grabbed = true;
        window.cursor_options.grab_mode = bevy::window::CursorGrabMode::Locked;
        window.cursor_options.visible = false;
    }
}

fn debug_camera_look(
    mut query: Query<&mut Transform, With<DebugCamera>>,
    mut mouse_motion: EventReader<MouseMotion>,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    camera_state: Res<DebugCameraState>,
) {
    let mut transform = query.single_mut();

    if camera_state.cursor_grabbed {
        // Mouse look
        for MouseMotion { delta } in mouse_motion.read() {
            let sensitivity = 0.001;
            let (mut yaw, mut pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);

            yaw -= delta.x * sensitivity;
            pitch -= delta.y * sensitivity;
            pitch = pitch.clamp(-1.5, 1.5);

            transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
        }
    }

    // Movement (always active)
    let speed = 10.0;
    let forward = transform.forward();
    let right = transform.right();
    let up = Vec3::Y;

    if keyboard.pressed(KeyCode::KeyW) {
        transform.translation -= forward * speed * time.delta_secs();
    }
    if keyboard.pressed(KeyCode::KeyS) {
        transform.translation += forward * speed * time.delta_secs();
    }
    if keyboard.pressed(KeyCode::KeyA) {
        transform.translation -= right * speed * time.delta_secs();
    }
    if keyboard.pressed(KeyCode::KeyD) {
        transform.translation += right * speed * time.delta_secs();
    }
    if keyboard.pressed(KeyCode::Space) {
        transform.translation += up * speed * time.delta_secs();
    }
    if keyboard.pressed(KeyCode::ShiftLeft) {
        transform.translation -= up * speed * time.delta_secs();
    }
}
