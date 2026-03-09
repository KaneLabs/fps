use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use bevy::prelude::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use multiplayer::player::{player_physics_bundle, player_replicated_bundle};
use multiplayer::world::{spawn_server_interactive_objects, spawn_world_physics};
use multiplayer::{SharedPlugin, FIXED_TIMESTEP_HZ, PROTOCOL_ID, SERVER_PORT};

fn main() {
    let mut app = App::new();

    // Headless server: no window
    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<bevy::winit::WinitPlugin>()
            .set(bevy::window::WindowPlugin {
                primary_window: None,
                primary_cursor_options: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                close_when_requested: false,
            }),
    );
    app.add_plugins(bevy::app::ScheduleRunnerPlugin::run_loop(
        Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    ));

    // Lightyear server
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
    });

    // Shared: protocol, physics, frame interpolation, movement observer
    app.add_plugins(SharedPlugin);

    // World — physics only, no rendering on headless server
    app.add_systems(Startup, spawn_world_physics);
    app.add_systems(Startup, spawn_server);
    app.add_systems(Startup, spawn_server_interactive_objects);

    // Client handling
    app.add_observer(handle_new_client);
    app.add_observer(handle_connected);

    app.run();
}

fn spawn_server(mut commands: Commands) {
    let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), SERVER_PORT);

    let server_entity = commands
        .spawn((
            NetcodeServer::new(NetcodeConfig {
                protocol_id: PROTOCOL_ID,
                private_key: [0; 32],
                client_timeout_secs: 120,
                ..Default::default()
            }),
            LocalAddr(server_addr),
            ServerUdpIo::default(),
        ))
        .id();

    commands.trigger(Start {
        entity: server_entity,
    });

    info!("Server listening on {}", server_addr);
}

/// When a new link is created, add ReplicationSender + ReplicationReceiver.
/// ReplicationSender: enables the server to replicate entities to this client.
/// ReplicationReceiver: enables receiving BEI Action entities from this client.
fn handle_new_client(trigger: On<Add, LinkOf>, mut commands: Commands) {
    let entity = trigger.entity;
    info!("New client link: {:?}", entity);
    commands.entity(entity).insert((
        ReplicationSender::new(
            Duration::from_millis(100),
            SendUpdatesMode::SinceLastAck,
            false,
        ),
        ReplicationReceiver::default(),
    ));
}

/// When a client connection is confirmed, spawn their player entity.
fn handle_connected(
    trigger: On<Add, Connected>,
    query: Query<(&RemoteId, Has<ReplicationSender>), With<ClientOf>>,
    mut commands: Commands,
) {
    let entity = trigger.entity;
    let Ok((remote_id, has_sender)) = query.get(entity) else {
        return;
    };

    let client_id = remote_id.0;
    let client_id_bits = client_id.to_bits();
    info!(
        "Client connected: {} (entity={:?}, has_replication_sender={})",
        client_id_bits, entity, has_sender
    );

    // Ensure ReplicationSender is present (should be from handle_new_client,
    // but if command flush ordering caused it to be missing, add it now)
    if !has_sender {
        warn!("ReplicationSender missing on client entity {:?}, adding now", entity);
        commands.entity(entity).insert(
            ReplicationSender::new(
                Duration::from_millis(100),
                SendUpdatesMode::SinceLastAck,
                false,
            ),
        );
    }

    // CS/Valorant-style replication:
    // - Owning client gets prediction (instant local movement, rollback on mismatch)
    // - All other clients get interpolation (smooth, slightly delayed, no rubberbanding)
    commands.spawn((
        player_replicated_bundle(client_id_bits),
        player_physics_bundle(),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ControlledBy {
            owner: entity,
            lifetime: Default::default(),
        },
    ));
}
