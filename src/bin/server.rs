use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use bevy::prelude::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::prelude::Lifetime;

use multiplayer::mesh::{GameMeshPlugin, MeshActive, MeshConfig, MeshIdGen, NeighborConfig};
use multiplayer::player::{player_physics_bundle, player_replicated_bundle, PLAYER_SPAWN_POS};
use multiplayer::protocol::{KillFeedEntry, LastDamagedBy, PlayerId, PlayerContext, PlayerDead, PlayerHealth, PlayerDisplayId};
use multiplayer::world::{spawn_server_interactive_objects, spawn_world_physics};
use multiplayer::{SharedPlugin, FIXED_TIMESTEP_HZ, PROTOCOL_ID, SERVER_PORT};
use lightyear_mesh::prelude::*;
use lightyear_mesh::dual_sim::BoundaryAxis;

use avian3d::prelude::Position;

/// Respawn delay in seconds before a dead player can respawn.
const RESPAWN_DELAY: f32 = 20.0;

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
            })
            .set(bevy::log::LogPlugin {
                filter: "bevy_enhanced_input::action::fns=error".into(),
                ..default()
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

    // Mesh networking (activates only if MESH_ZONE env var is set)
    app.add_plugins(GameMeshPlugin);

    // World — physics only, no rendering on headless server
    app.add_systems(Startup, spawn_world_physics);
    app.add_systems(Startup, spawn_server);
    app.add_systems(Startup, spawn_server_interactive_objects);

    // Player ID counter
    app.init_resource::<PlayerCounter>();

    // Death and respawn
    app.init_resource::<PendingRespawns>();
    app.add_systems(FixedUpdate, (check_player_death, process_respawns));

    // Client handling
    app.add_observer(handle_new_client);
    app.add_observer(handle_connected);

    app.run();
}

fn spawn_server(mut commands: Commands) {
    let port = std::env::var("SERVER_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SERVER_PORT);
    let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);

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

/// Sequential player number counter.
#[derive(Resource, Default)]
struct PlayerCounter(u32);

/// When a client connection is confirmed, spawn their player entity.
fn handle_connected(
    trigger: On<Add, Connected>,
    query: Query<(&RemoteId, Has<ReplicationSender>), With<ClientOf>>,
    existing_players: Query<(Entity, &PlayerId)>,
    mut commands: Commands,
    mut counter: ResMut<PlayerCounter>,
    mesh_active: Option<Res<MeshActive>>,
    mesh_id_gen: Option<Res<MeshIdGen>>,
    mesh_config: Option<Res<MeshConfig>>,
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

    // If mesh is active, check if this client's player entity already exists
    // (from mesh ghost sync → authority claim). If so, re-associate instead of spawning new.
    if mesh_active.is_some() {
        // Look for an existing entity with this PlayerId (arrived via mesh)
        let existing = existing_players.iter().find(|(_, pid)| pid.0 == client_id_bits);
        if let Some((existing_entity, _)) = existing {
            info!(
                "[MESH] Client {} reconnected — re-associating existing entity {:?}",
                client_id_bits, existing_entity
            );
            // Update MeshControlled with THIS server's perspective
            let config = mesh_config.as_ref().expect("MeshConfig must exist when mesh is active");
            let updated_controlled = MeshControlled {
                authority_server: config.server_id,
                zone_bounds: ZoneBounds {
                    min_x: config.zone_min_x,
                    min_z: -500.0,
                    max_x: config.zone_max_x,
                    max_z: 500.0,
                },
                neighbors: config.neighbors.iter().map(|n| MeshNeighborInfo {
                    id: n.id,
                    address: n.game_addr.to_string(),
                    boundary: BoundaryKind::Seamless,
                    zone_bounds: ZoneBounds {
                        min_x: n.zone_min_x,
                        min_z: -500.0,
                        max_x: n.zone_max_x,
                        max_z: 500.0,
                    },
                }).collect(),
                active_connections: vec![config.server_id],
            };

            commands.entity(existing_entity).insert((
                // Lightyear replication
                ControlledBy {
                    owner: entity,
                    lifetime: Lifetime::Persistent,
                },
                Replicate::to_clients(NetworkTarget::All),
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
                // Gameplay components the ghost doesn't have
                PlayerContext,
                player_physics_bundle(),
                // Updated mesh context with this server's neighbors
                updated_controlled,
            ));
            commands.queue(ForcePositionUpdate);
            return;
        }

        // If spawn point is outside our zone, skip (will arrive via mesh)
        if let Some(ref config) = mesh_config {
            let spawn_x = PLAYER_SPAWN_POS.x;
            if spawn_x < config.zone_min_x || spawn_x >= config.zone_max_x {
                info!(
                    "Client {} connected but spawn ({}) is outside our zone [{}, {}] — skipping player spawn",
                    client_id_bits, spawn_x, config.zone_min_x, config.zone_max_x
                );
                return;
            }
        }
    }

    // CS/Valorant-style replication:
    // - Owning client gets prediction (instant local movement, rollback on mismatch)
    // - All other clients get interpolation (smooth, slightly delayed, no rubberbanding)
    counter.0 += 1;
    let display_id = counter.0;

    let mut player = commands.spawn((
        player_replicated_bundle(client_id_bits),
        player_physics_bundle(),
        PlayerDisplayId(display_id),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ControlledBy {
            owner: entity,
            lifetime: if mesh_active.is_some() { Lifetime::Persistent } else { Default::default() },
        },
    ));

    // If mesh is active, add mesh components so this entity syncs to neighbor servers
    if mesh_active.is_some() {
        if let Some(id_gen) = mesh_id_gen {
            let mesh_id = id_gen.0.next();

            // Determine which side of the boundary this server owns
            // Boundary is at zone_max_x (east edge) or zone_min_x (west edge)
            // For Server 1 (zone -500..0): boundary at x=0, my side is negative
            // For Server 2 (zone 0..500): boundary at x=0, my side is positive
            let dual_sim = if let Some(ref config) = mesh_config {
                let boundary_x = if config.zone_max_x < 500.0 {
                    config.zone_max_x // boundary at east edge of my zone
                } else {
                    config.zone_min_x // boundary at west edge of my zone
                };
                let my_side_positive = config.zone_min_x >= boundary_x;
                MeshDualSim::single(BoundaryAxis::X, boundary_x, my_side_positive)
            } else {
                MeshDualSim::default()
            };

            let config = mesh_config.as_ref().expect("MeshConfig must exist when mesh is active");
            let controlled = MeshControlled {
                authority_server: config.server_id,
                zone_bounds: ZoneBounds {
                    min_x: config.zone_min_x,
                    min_z: -500.0,
                    max_x: config.zone_max_x,
                    max_z: 500.0,
                },
                neighbors: config.neighbors.iter().map(|n| MeshNeighborInfo {
                    id: n.id,
                    address: n.game_addr.to_string(),
                    boundary: BoundaryKind::Seamless,
                    zone_bounds: ZoneBounds {
                        min_x: n.zone_min_x,
                        min_z: -500.0,
                        max_x: n.zone_max_x,
                        max_z: 500.0,
                    },
                }).collect(),
                active_connections: vec![config.server_id],
            };

            player.insert((
                mesh_id,
                MeshSyncSource,
                MeshAuthority { version: 0 },
                dual_sim,
                controlled,
            ));
            info!("Player {} spawned with MeshEntityId {:?} + dual-sim + MeshControlled", client_id_bits, mesh_id);
        }
    }

    // Force-touch all existing players' Positions so lightyear sends fresh snapshots
    // to the new client. Without this, interpolated entities need 2 snapshots to render
    // and a stationary player never generates a second one.
    commands.queue(ForcePositionUpdate);
}

/// Command that force-touches all player Positions so lightyear marks them as changed.
struct ForcePositionUpdate;

impl Command for ForcePositionUpdate {
    fn apply(self, world: &mut World) {
        let mut query = world.query_filtered::<&mut Position, With<PlayerId>>();
        for mut pos in query.iter_mut(world) {
            pos.set_changed();
        }
    }
}

// ========================================
// Death & Respawn
// ========================================

/// Tracks when each dead player becomes eligible for respawn.
#[derive(Resource, Default)]
struct PendingRespawns {
    /// Maps player entity -> time when respawn is allowed.
    timers: Vec<(Entity, f32)>,
}

/// Server-only: when health drops to 0, mark the player as dead.
/// Only runs when PlayerHealth changes (event-driven via Changed<>).
fn check_player_death(
    query: Query<(Entity, &PlayerHealth, &PlayerId, &PlayerDisplayId, &LastDamagedBy), (Changed<PlayerHealth>, Without<PlayerDead>)>,
    all_players: Query<(&PlayerId, &PlayerDisplayId)>,
    mut commands: Commands,
    mut pending: ResMut<PendingRespawns>,
    time: Res<Time>,
) {
    for (entity, health, player_id, victim_display, last_damaged_by) in query.iter() {
        if health.0 > 0 {
            continue;
        }

        // Look up killer's display ID
        let killer_display = all_players.iter()
            .find(|(pid, _)| pid.0 == last_damaged_by.0)
            .map(|(_, d)| d.0)
            .unwrap_or(0);

        info!(
            "[DEATH] Player {} killed by Player {}! Respawn in {}s",
            victim_display.0, killer_display, RESPAWN_DELAY
        );

        commands.entity(entity).insert(PlayerDead);
        commands.entity(entity).insert(avian3d::prelude::Rotation(
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        ));
        pending.timers.push((entity, time.elapsed_secs() + RESPAWN_DELAY));

        // Spawn kill feed entry — replicated to all clients
        let now = time.elapsed_secs();
        commands.spawn((
            KillFeedEntry {
                killer_name: multiplayer::auth::client_id_to_base58(last_damaged_by.0),
                victim_name: multiplayer::auth::client_id_to_base58(player_id.0),
                timestamp: now,
            },
            Replicate::to_clients(NetworkTarget::All),
        ));
    }
}

/// Server-only: processes respawn timers. Revives players after delay.
/// This is the hook point for pay-to-respawn (Solana transaction check).
fn process_respawns(
    mut pending: ResMut<PendingRespawns>,
    mut query: Query<(&mut PlayerHealth, &mut Position, &mut avian3d::prelude::Rotation, &PlayerId), With<PlayerDead>>,
    mut commands: Commands,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    let mut i = 0;
    while i < pending.timers.len() {
        if now >= pending.timers[i].1 {
            let (entity, _) = pending.timers.remove(i);

            let Ok((mut health, mut position, mut rotation, player_id)) = query.get_mut(entity) else {
                continue;
            };

            if authorize_respawn(player_id.0) {
                info!("[RESPAWN] Player {:?} (id={}) respawning", entity, player_id.0);
                health.0 = 100;
                position.0 = PLAYER_SPAWN_POS;
                rotation.0 = Quat::IDENTITY;
                commands.entity(entity).remove::<PlayerDead>();
            }
        } else {
            i += 1;
        }
    }
}

/// Respawn authorization gate. Currently always approves.
/// Future: verify Solana payment, check wallet balance, deduct SOL.
fn authorize_respawn(_client_id: u64) -> bool {
    true
}
