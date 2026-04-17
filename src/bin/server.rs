use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use bevy::prelude::*;
use leafwing_input_manager::prelude::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::interpolation::plugin::InterpolationDelay;
use lightyear_avian3d::prelude::{LagCompensationHistory, LagCompensationPlugin, LagCompensationSpatialQuery};
use avian3d::prelude::SpatialQueryFilter;

use multiplayer::auth::{self, VerifiedWallets};
use multiplayer::player::{player_physics_bundle, player_replicated_bundle, select_spawn_point};
use multiplayer::protocol::{KillFeedEntry, LastDamagedBy, PlayerActions, PlayerId, PlayerDead, PlayerEquipped, PlayerHealth, PlayerDisplayId, PlayerInventory, PlayerYaw, PlayerPitch, WalletAuthMessage};
use multiplayer::solana::{self, RespawnAuth, RespawnConfig, WalletAddress};
use multiplayer::world::{spawn_server_interactive_objects, spawn_world_physics, Equippable};
use multiplayer::{SharedPlugin, FIXED_TIMESTEP_HZ, PROTOCOL_ID, SERVER_PORT};

use avian3d::prelude::Position;

/// Respawn delay in seconds before a dead player can respawn.
const RESPAWN_DELAY: f32 = 20.0;

fn main() {
    eprintln!(
        "Anima Server {} (commit {} built {})",
        env!("ANIMA_VERSION"),
        env!("ANIMA_BUILD_SHA"),
        env!("ANIMA_BUILD_DATE"),
    );

    let mut app = App::new();

    // Headless server: no window
    app.add_plugins(
        DefaultPlugins
            .build()
            .disable::<bevy::winit::WinitPlugin>()
            .disable::<bevy::render::RenderPlugin>()
            .disable::<bevy::core_pipeline::CorePipelinePlugin>()
            .disable::<bevy::pbr::PbrPlugin>()
            .disable::<bevy::gltf::GltfPlugin>()
            .disable::<bevy::sprite::SpritePlugin>()
            .disable::<bevy::ui::UiPlugin>()
            .disable::<bevy::text::TextPlugin>()
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

    // Lag compensation — maintains collider history so hits can be rewound
    // to where targets were when the client saw them
    app.add_plugins(LagCompensationPlugin);

    // World — physics only, no rendering on headless server
    app.add_systems(Startup, spawn_world_physics);
    app.add_systems(Startup, spawn_server);
    app.add_systems(Startup, spawn_server_interactive_objects);

    // Player ID counter
    app.init_resource::<PlayerCounter>();

    // Solana: verified wallets + respawn config
    app.init_resource::<VerifiedWallets>();
    app.insert_resource(solana::parse_respawn_config());

    // Death and respawn
    app.init_resource::<PendingRespawns>();
    app.add_systems(FixedUpdate, (kill_plane, check_player_death, process_respawns).chain());

    // Wallet auth: process incoming auth messages from clients
    app.add_systems(Update, process_wallet_auth);

    // Client handling
    app.add_observer(handle_new_client);
    app.add_observer(handle_connected);
    app.add_observer(handle_disconnected);

    // Lag-compensated hitscan damage — FixedUpdate system querying ActionState.
    // The shared world::shared_primary_action_system handles tracer prediction
    // on the client. This system runs on the server and rewinds targets to
    // where the shooter saw them (using the shooter's replicated InterpolationDelay).
    app.add_systems(FixedUpdate, server_shoot_with_lag_comp);

    app.run();
}

fn spawn_server(mut commands: Commands) {
    let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), SERVER_PORT);

    let server_entity = commands
        .spawn((
            NetcodeServer::new(NetcodeConfig {
                protocol_id: PROTOCOL_ID,
                private_key: [0; 32],
                // Short timeout — stale client IDs clear quickly so reconnects work
                client_timeout_secs: 10,
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
            Duration::from_secs_f64(1.0 / FIXED_TIMESTEP_HZ),
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
    living_query: Query<&Position, (With<PlayerId>, Without<PlayerDead>)>,
    mut commands: Commands,
    mut counter: ResMut<PlayerCounter>,
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

    // Pick spawn point furthest from living players
    let living_positions: Vec<Vec3> = living_query.iter().map(|p| p.0).collect();
    let spawn_pos = select_spawn_point(&living_positions);

    // CS/Valorant-style replication:
    // - Owning client gets prediction (instant local movement, rollback on mismatch)
    // - All other clients get interpolation (smooth, slightly delayed, no rubberbanding)
    counter.0 += 1;
    let display_id = counter.0;

    commands.spawn((
        player_replicated_bundle(client_id_bits),
        player_physics_bundle(),
        PlayerDisplayId(display_id),
        // WalletAddress starts empty — populated after auth verification
        WalletAddress::default(),
        Replicate::to_clients(NetworkTarget::All),
        PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
        ControlledBy {
            owner: entity,
            lifetime: Default::default(),
        },
        // Lag compensation: server keeps a history of this collider's position/rotation
        // so hitscan from remote shooters can be rewound to where the client saw them
        LagCompensationHistory::default(),
    ))
    // Set spawn position after spawn — player_replicated_bundle already includes Position
    .insert(Position(spawn_pos));

    info!("[SPAWN] Player {} spawning at {:?}", display_id, spawn_pos);
}

/// When a client disconnects, clean up server state.
/// Lightyear auto-despawns SessionBased controlled entities (the player),
/// but we need to clean up VerifiedWallets and log the event.
fn handle_disconnected(
    trigger: On<Add, Disconnected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut verified_wallets: ResMut<VerifiedWallets>,
) {
    let entity = trigger.entity;
    let Ok(remote_id) = query.get(entity) else {
        return;
    };

    let client_id = remote_id.0.to_bits();
    info!("[DISCONNECT] Client {} (entity={:?}) disconnected", client_id, entity);

    // Remove from verified wallets
    if verified_wallets.remove(client_id) {
        info!("[DISCONNECT] Removed wallet verification for client {}", client_id);
    }
}

/// Server-only FixedUpdate system: handles hitscan damage with lag compensation.
/// The shared world::shared_primary_action_system handles tracer prediction on the
/// client. This system runs on the server and uses the shooter's InterpolationDelay
/// to rewind targets to where they were when the client saw them.
///
/// Queries ActionState each tick and fires on `just_pressed(Primary)`.
fn server_shoot_with_lag_comp(
    player_query: Query<(
        Entity,
        &ActionState<PlayerActions>,
        &Position,
        &PlayerYaw,
        &PlayerPitch,
        &PlayerEquipped,
        &PlayerId,
        Option<&ControlledBy>,
    )>,
    client_query: Query<&InterpolationDelay, With<ClientOf>>,
    mut health_query: Query<(&mut PlayerHealth, Option<&mut LastDamagedBy>)>,
    lag_query: LagCompensationSpatialQuery,
    mut last_shot: Local<std::collections::HashMap<Entity, f32>>,
    time: Res<Time>,
) {
    for (shooter, action, pos, yaw, pitch, equipped, attacker_id, controlled_by) in player_query.iter() {
        if !action.just_pressed(&PlayerActions::Primary) {
            continue;
        }

        // Only run for gun shots
        let Some(ref name) = equipped.0 else { continue; };
        if !(name.contains("AK") || name.contains("ak") || name.contains("gun")) {
            continue;
        }

        // Cooldown per shooter
        let current = time.elapsed_secs();
        let last = last_shot.get(&shooter).copied().unwrap_or(-10.0);
        if current - last < multiplayer::world::SHOOT_COOLDOWN {
            continue;
        }
        last_shot.insert(shooter, current);

        // Get the shooter's InterpolationDelay so we know how far back to rewind
        let Some(controlled) = controlled_by else {
            warn!("[SHOOT-SERVER] Shooter {:?} has no ControlledBy", shooter);
            continue;
        };
        let Ok(delay) = client_query.get(controlled.owner) else {
            warn!("[SHOOT-SERVER] No InterpolationDelay for client {:?}", controlled.owner);
            continue;
        };

        let eye_pos = pos.0 + Vec3::Y * 0.8;
        let ray_dir = Quat::from_euler(EulerRot::YXZ, yaw.0, pitch.0, 0.0) * Vec3::NEG_Z;
        let mut filter = SpatialQueryFilter::from_excluded_entities([shooter]);

        if let Some(hit) = lag_query.cast_ray(
            *delay,
            eye_pos,
            Dir3::new(ray_dir).unwrap_or(Dir3::NEG_Z),
            multiplayer::world::SHOOT_RANGE,
            true,
            &mut filter,
        ) {
            info!(
                "[SHOOT-SERVER] Lag-comp hit entity {:?} at distance {:.1}",
                hit.entity, hit.distance
            );
            if let Ok((mut health, last_damaged)) = health_query.get_mut(hit.entity) {
                health.0 -= multiplayer::world::SHOOT_DAMAGE;
                if let Some(mut last) = last_damaged {
                    last.0 = attacker_id.0;
                }
                info!(
                    "[SHOOT-SERVER] Player hit! {} damage applied, health now: {}",
                    multiplayer::world::SHOOT_DAMAGE, health.0
                );
            }
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

/// Server-only: kill plane — any player below this Y is instantly killed.
/// Prevents players from falling forever if they clip through geometry.
const KILL_PLANE_Y: f32 = -60.0;

fn kill_plane(
    mut query: Query<(&Position, &mut PlayerHealth, &PlayerId), Without<PlayerDead>>,
) {
    for (pos, mut health, id) in query.iter_mut() {
        if pos.0.y < KILL_PLANE_Y && health.0 > 0 {
            info!("[KILL-PLANE] Player {} fell below y={} (pos={:?})", id.0, KILL_PLANE_Y, pos.0);
            health.0 = 0;
        }
    }
}

/// Server-only: when health drops to 0, mark the player as dead and drop all items.
/// Equipped item + inventory items are dropped as world Equippable entities at
/// the death position. This is the core loot loop — die, lose your stuff.
fn check_player_death(
    mut death_query: Query<
        (Entity, &PlayerHealth, &PlayerId, &PlayerDisplayId, &LastDamagedBy,
         &Position, &mut PlayerEquipped, &mut PlayerInventory),
        (Changed<PlayerHealth>, Without<PlayerDead>),
    >,
    all_players: Query<(&PlayerId, &PlayerDisplayId)>,
    mut equippable_query: Query<(&Equippable, &mut Position), Without<PlayerHealth>>,
    mut commands: Commands,
    mut pending: ResMut<PendingRespawns>,
    time: Res<Time>,
) {
    for (entity, health, player_id, victim_display, last_damaged_by,
         death_pos, mut equipped, mut inventory) in death_query.iter_mut()
    {
        if health.0 > 0 {
            continue;
        }

        let killer_display = all_players.iter()
            .find(|(pid, _)| pid.0 == last_damaged_by.0)
            .map(|(_, d)| d.0)
            .unwrap_or(0);

        // --- Drop all items at death position ---
        // Collect all item names to drop (equipped + inventory)
        let mut items_to_drop: Vec<String> = Vec::new();
        if let Some(equipped_name) = equipped.0.take() {
            items_to_drop.push(equipped_name);
        }
        items_to_drop.append(&mut inventory.items);

        // Move matching world Equippable entities to the death position.
        // Spread items slightly so they don't stack on the exact same spot.
        let drop_pos = death_pos.0;
        let mut drop_index = 0u32;
        for item_name in &items_to_drop {
            // Small offset so items fan out in a circle around the death spot
            let angle = drop_index as f32 * std::f32::consts::TAU / items_to_drop.len().max(1) as f32;
            let offset = if items_to_drop.len() > 1 {
                Vec3::new(angle.cos() * 0.5, 0.0, angle.sin() * 0.5)
            } else {
                Vec3::ZERO
            };

            let mut found = false;
            for (equippable, mut eq_pos) in equippable_query.iter_mut() {
                if equippable.name == *item_name {
                    eq_pos.0 = drop_pos + offset;
                    found = true;
                    info!("[DEATH DROP] Moved {} to {:?}", item_name, eq_pos.0);
                    break;
                }
            }

            if !found {
                info!("[DEATH DROP] No world entity found for '{}' — skipping", item_name);
            }
            drop_index += 1;
        }

        if !items_to_drop.is_empty() {
            info!(
                "[DEATH] Player {} dropped {} item(s): {:?}",
                victim_display.0, items_to_drop.len(), items_to_drop
            );
        }

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
/// Picks the spawn point furthest from living players to avoid spawn-camping.
///
/// This is the pay-to-respawn gate. Uses `solana::check_respawn_authorization()`
/// which checks the RespawnConfig:
/// - Dev mode (default): always authorized (--require-respawn-payment not set)
/// - Production mode: checks wallet verification, and in the future checks
///   ANIMA_RESPAWN token balance or SOL balance via Solana RPC.
fn process_respawns(
    mut pending: ResMut<PendingRespawns>,
    mut query: Query<(&mut PlayerHealth, &mut Position, &mut avian3d::prelude::Rotation, &PlayerId, &mut PlayerEquipped, &mut PlayerInventory), With<PlayerDead>>,
    living_query: Query<&Position, (With<PlayerId>, Without<PlayerDead>)>,
    mut commands: Commands,
    time: Res<Time>,
    respawn_config: Res<RespawnConfig>,
    verified_wallets: Res<VerifiedWallets>,
) {
    let now = time.elapsed_secs();
    let mut i = 0;
    while i < pending.timers.len() {
        if now >= pending.timers[i].1 {
            let (entity, _) = pending.timers.remove(i);

            let Ok((mut health, mut position, mut rotation, player_id, mut equipped, mut inventory)) = query.get_mut(entity) else {
                continue;
            };

            match solana::check_respawn_authorization(&respawn_config, player_id.0, &verified_wallets) {
                RespawnAuth::Authorized => {
                    let living_positions: Vec<Vec3> = living_query
                        .iter()
                        .map(|p| p.0)
                        .collect();
                    let spawn_pos = select_spawn_point(&living_positions);

                    info!("[RESPAWN] Player {:?} (id={}) respawning at {:?}", entity, player_id.0, spawn_pos);
                    health.0 = 100;
                    position.0 = spawn_pos;
                    rotation.0 = Quat::IDENTITY;
                    // Ensure inventory is clean on respawn (should already be empty from death drop)
                    equipped.0 = None;
                    inventory.items.clear();
                    commands.entity(entity).remove::<PlayerDead>();
                }
                RespawnAuth::InsufficientFunds { required_lamports, available_lamports } => {
                    warn!(
                        "[RESPAWN] Player {} denied — insufficient funds ({} available, {} required lamports)",
                        player_id.0, available_lamports, required_lamports
                    );
                    // Re-queue with a retry delay — player may fund wallet
                    pending.timers.push((entity, now + 5.0));
                }
                RespawnAuth::WalletNotVerified => {
                    warn!(
                        "[RESPAWN] Player {} denied — wallet not verified yet",
                        player_id.0
                    );
                    // Re-queue — wallet auth may still be in flight
                    pending.timers.push((entity, now + 5.0));
                }
            }
        } else {
            i += 1;
        }
    }
}

// ========================================
// Wallet Auth Verification
// ========================================

/// Process incoming wallet auth messages from clients.
/// Reads WalletAuthMessage from each client's MessageReceiver, verifies the
/// Ed25519 signature, and maps the pubkey -> Solana wallet address on the player entity.
fn process_wallet_auth(
    mut client_query: Query<(&RemoteId, &mut MessageReceiver<WalletAuthMessage>), With<ClientOf>>,
    mut player_query: Query<(&PlayerId, &mut WalletAddress)>,
    mut verified_wallets: ResMut<VerifiedWallets>,
) {
    for (remote_id, mut receiver) in client_query.iter_mut() {
        let client_id_bits = remote_id.0.to_bits();

        // Skip if already verified
        if verified_wallets.is_verified(client_id_bits) {
            // Drain any remaining messages
            for _ in receiver.receive() {}
            continue;
        }

        for auth_msg in receiver.receive() {
            info!(
                "[AUTH] Received wallet auth from client {} (pubkey: {})",
                client_id_bits,
                auth::pubkey_address(&auth_msg.pubkey)
            );

            match auth::verify_auth_signature(
                &auth_msg.pubkey,
                &auth_msg.signature,
                client_id_bits,
            ) {
                Ok(wallet_address) => {
                    info!(
                        "[AUTH] Wallet VERIFIED for client {}: {}",
                        client_id_bits, wallet_address
                    );

                    // Store in verified wallets resource
                    verified_wallets.wallets.insert(client_id_bits, wallet_address.clone());

                    // Update the player entity's WalletAddress component (replicated to all)
                    for (player_id, mut wallet) in player_query.iter_mut() {
                        if player_id.0 == client_id_bits {
                            wallet.0 = wallet_address.clone();
                            info!(
                                "[AUTH] WalletAddress set on player entity for client {}",
                                client_id_bits
                            );
                            break;
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        "[AUTH] Wallet auth FAILED for client {}: {}",
                        client_id_bits, e
                    );
                }
            }
        }
    }
}
