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
use multiplayer::solana::{self, ChainCheckResult, ChainVerifier, RespawnAuth, RespawnConfig, WalletAddress};
use multiplayer::world::{spawn_server_interactive_objects, spawn_world_physics, Equippable};
use multiplayer::{SharedPlugin, FIXED_TIMESTEP_HZ, PROTOCOL_ID, SERVER_PORT};

use lightyear::prelude::input::InputBuffer;
use lightyear::prelude::input::leafwing::LeafwingSnapshot;

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

    // Asset root — works for cargo run, raw binary, systemd service.
    let asset_path = multiplayer::asset_root_path();
    eprintln!("Assets: {asset_path}");

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
            .set(AssetPlugin {
                file_path: asset_path,
                ..default()
            })
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

    // Solana: verified wallets + respawn config + on-chain verifier
    app.init_resource::<VerifiedWallets>();
    let respawn_config = solana::parse_respawn_config();
    if respawn_config.require_payment {
        info!(
            "[SOLANA] Pay-to-spawn ENABLED — rpc: {}, cost: {} lamports, treasury: {}",
            respawn_config.rpc_url, respawn_config.respawn_cost_lamports, respawn_config.treasury_address
        );
    }
    app.insert_resource(ChainVerifier::json_rpc(&respawn_config.rpc_url));
    app.insert_resource(respawn_config);

    // Death and respawn
    app.init_resource::<PendingRespawns>();
    app.add_systems(
        FixedUpdate,
        (kill_plane, check_player_death, publish_kill_feed, process_respawns, poll_chain_checks).chain(),
    );

    // Wallet auth: process incoming auth messages from clients
    app.init_resource::<SupersededSessions>();
    // Chained: a session queued for a kick this frame is disconnected this frame.
    app.add_systems(Update, (process_wallet_auth, kick_superseded_sessions).chain());

    // Client handling
    app.add_observer(handle_new_client);
    app.add_observer(handle_connected);
    app.add_observer(handle_disconnected);

    // Lag-compensated hitscan damage — FixedUpdate system querying ActionState.
    // The shared world::shared_primary_action_system handles tracer prediction
    // on the client. This system runs on the server and rewinds targets to
    // where the shooter saw them (using the shooter's replicated InterpolationDelay).
    app.add_systems(FixedUpdate, server_shoot_with_lag_comp);

    // Diagnostics: per-player input arrival lateness, logged 1/s
    app.add_systems(FixedUpdate, log_input_lag);

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
                // Applied via the registry, not the bare constant: damage that
                // does not flow through DamageSource is damage the on-chain
                // contributor guard cannot see.
                health.0 -= multiplayer::world::DamageSource::Hitscan.per_hit_damage();
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
        (Entity, &PlayerHealth, &PlayerDisplayId, &LastDamagedBy,
         &Position, &mut PlayerEquipped, &mut PlayerInventory),
        (Changed<PlayerHealth>, Without<PlayerDead>),
    >,
    all_players: Query<(&PlayerId, &PlayerDisplayId)>,
    mut equippable_query: Query<(&Equippable, &mut Position), Without<PlayerHealth>>,
    mut commands: Commands,
    mut pending: ResMut<PendingRespawns>,
    time: Res<Time>,
) {
    for (entity, health, victim_display, last_damaged_by,
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
        for (drop_index, item_name) in items_to_drop.iter().enumerate() {
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
    }
}

/// Sentinel `LastDamagedBy` value meaning "nobody has damaged this player".
const NO_KILLER: u64 = 0;

/// Killer label for deaths with no responsible player (kill plane, or a killer
/// who disconnected before the victim died).
const ENVIRONMENT_KILLER: &str = "the void";

/// How many leading characters of the wallet address to show as a name.
/// Matches the existing convention (`&identity.address[..8]` in the client
/// window title).
const WALLET_NAME_CHARS: usize = 8;

/// Player-facing display name.
///
/// Prefers the verified wallet address, truncated — it is the player's DURABLE
/// identity and is stable across reconnects. Falls back to the sequential
/// "Player N" while the wallet is unverified.
///
/// Deliberately never derived from the netcode client id: since #19 that id is
/// random per connection, so naming from it gave a player a different name every
/// session. `PlayerDisplayId` is also per-session, but it is at least legible
/// and already what the server logs use.
fn player_display_name(wallet: &WalletAddress, display: &PlayerDisplayId) -> String {
    let address = wallet.0.trim();
    if address.is_empty() {
        return format!("Player {}", display.0);
    }
    // char-wise, not byte-wise: never panic on a non-ASCII boundary even though
    // base58 addresses are always ASCII.
    address.chars().take(WALLET_NAME_CHARS).collect()
}

/// Server-only: publishes a replicated kill feed entry for each newly dead
/// player. Split from `check_player_death` so death/respawn game logic stays
/// free of replication concerns (and headless-testable) — `Replicate`'s
/// component hooks require the full lightyear server stack.
///
/// Runs chained directly after `check_player_death`, so `Added<PlayerDead>`
/// fires the same tick the death is processed.
fn publish_kill_feed(
    newly_dead: Query<
        (&LastDamagedBy, &WalletAddress, &PlayerDisplayId),
        Added<PlayerDead>,
    >,
    all_players: Query<(&PlayerId, &WalletAddress, &PlayerDisplayId)>,
    mut commands: Commands,
    time: Res<Time>,
) {
    for (last_damaged_by, victim_wallet, victim_display) in newly_dead.iter() {
        // A killer of 0 is the default LastDamagedBy — nobody ever damaged this
        // player, so the death was environmental (kill plane). Previously this
        // base58-encoded the literal id 0 and rendered "11111111 killed X".
        let killer_name = if last_damaged_by.0 == NO_KILLER {
            ENVIRONMENT_KILLER.to_string()
        } else {
            all_players
                .iter()
                .find(|(pid, _, _)| pid.0 == last_damaged_by.0)
                .map(|(_, wallet, display)| player_display_name(wallet, display))
                // Killer already disconnected — their entity is gone, so there
                // is no wallet left to name them by.
                .unwrap_or_else(|| ENVIRONMENT_KILLER.to_string())
        };

        commands.spawn((
            KillFeedEntry {
                killer_name,
                victim_name: player_display_name(victim_wallet, victim_display),
                timestamp: time.elapsed_secs(),
            },
            Replicate::to_clients(NetworkTarget::All),
        ));
    }
}

/// Retry delay (seconds) after a denied respawn (unverified wallet,
/// insufficient funds, or RPC failure).
const RESPAWN_RETRY_DELAY: f32 = 5.0;

/// Server-only marker: an async on-chain balance check is in flight for this
/// dead player. Holds the receiver for the worker thread's verdict; polled by
/// `poll_chain_checks` with `try_recv` — the game loop never blocks on RPC.
/// (`Mutex` only because components must be `Sync`; there is no contention.)
#[derive(Component)]
struct PendingChainCheck(std::sync::Mutex<std::sync::mpsc::Receiver<ChainCheckResult>>);

/// Revive a dead player at the spawn point furthest from living players.
#[allow(clippy::too_many_arguments)]
fn revive_player(
    entity: Entity,
    player_id: &PlayerId,
    health: &mut PlayerHealth,
    position: &mut Position,
    rotation: &mut avian3d::prelude::Rotation,
    equipped: &mut PlayerEquipped,
    inventory: &mut PlayerInventory,
    living_positions: &[Vec3],
    commands: &mut Commands,
) {
    let spawn_pos = select_spawn_point(living_positions);
    info!("[RESPAWN] Player {:?} (id={}) respawning at {:?}", entity, player_id.0, spawn_pos);
    health.0 = 100;
    position.0 = spawn_pos;
    rotation.0 = Quat::IDENTITY;
    // Ensure inventory is clean on respawn (should already be empty from death drop)
    equipped.0 = None;
    inventory.items.clear();
    commands.entity(entity).remove::<PlayerDead>();
}

/// Server-only: processes respawn timers — the pay-to-respawn gate.
///
/// On timer expiry, `solana::check_respawn_authorization()` decides:
/// - Dev mode (default): authorized → revive immediately.
/// - Payment mode, wallet unverified: re-queue (auth may be in flight).
/// - Payment mode, wallet verified: dispatch an async on-chain balance check
///   (worker thread + channel); `poll_chain_checks` delivers the verdict.
///   FixedUpdate never blocks on RPC.
fn process_respawns(
    mut pending: ResMut<PendingRespawns>,
    mut query: Query<(&mut PlayerHealth, &mut Position, &mut avian3d::prelude::Rotation, &PlayerId, &mut PlayerEquipped, &mut PlayerInventory), With<PlayerDead>>,
    checks_in_flight: Query<(), With<PendingChainCheck>>,
    living_query: Query<&Position, (With<PlayerId>, Without<PlayerDead>)>,
    mut commands: Commands,
    time: Res<Time>,
    respawn_config: Res<RespawnConfig>,
    verified_wallets: Res<VerifiedWallets>,
    verifier: Res<ChainVerifier>,
) {
    let now = time.elapsed_secs();
    let mut i = 0;
    while i < pending.timers.len() {
        if now >= pending.timers[i].1 {
            let (entity, _) = pending.timers.remove(i);

            let Ok((mut health, mut position, mut rotation, player_id, mut equipped, mut inventory)) = query.get_mut(entity) else {
                continue;
            };

            // Defensive: never stack a second check on an entity that already
            // has one in flight (would double-charge once payments land).
            if checks_in_flight.contains(entity) {
                continue;
            }

            match solana::check_respawn_authorization(&respawn_config, player_id.0, &verified_wallets) {
                RespawnAuth::Authorized => {
                    let living_positions: Vec<Vec3> = living_query
                        .iter()
                        .map(|p| p.0)
                        .collect();
                    revive_player(
                        entity, player_id, &mut health, &mut position, &mut rotation,
                        &mut equipped, &mut inventory, &living_positions, &mut commands,
                    );
                }
                RespawnAuth::WalletNotVerified => {
                    warn!(
                        "[RESPAWN] Player {} denied — wallet not verified yet",
                        player_id.0
                    );
                    // Re-queue — wallet auth may still be in flight
                    pending.timers.push((entity, now + RESPAWN_RETRY_DELAY));
                }
                RespawnAuth::RequiresChainCheck { wallet } => {
                    info!(
                        "[RESPAWN] Player {} — checking on-chain balance of {} (need {} lamports)",
                        player_id.0, wallet, respawn_config.respawn_cost_lamports
                    );
                    let rx = solana::spawn_balance_check(
                        verifier.0.clone(),
                        wallet,
                        respawn_config.respawn_cost_lamports,
                    );
                    commands
                        .entity(entity)
                        .insert(PendingChainCheck(std::sync::Mutex::new(rx)));
                }
            }
        } else {
            i += 1;
        }
    }
}

/// Server-only: delivers verdicts from in-flight on-chain balance checks.
/// Funded → revive. Insufficient funds or RPC failure → stay dead, re-queue
/// (fail closed: an RPC outage must never grant free respawns).
fn poll_chain_checks(
    mut pending: ResMut<PendingRespawns>,
    mut query: Query<(Entity, &PendingChainCheck, &mut PlayerHealth, &mut Position, &mut avian3d::prelude::Rotation, &PlayerId, &mut PlayerEquipped, &mut PlayerInventory), With<PlayerDead>>,
    living_query: Query<&Position, (With<PlayerId>, Without<PlayerDead>)>,
    mut commands: Commands,
    time: Res<Time>,
) {
    let now = time.elapsed_secs();
    for (entity, check, mut health, mut position, mut rotation, player_id, mut equipped, mut inventory) in query.iter_mut() {
        let verdict = match check.0.lock().expect("chain check receiver poisoned").try_recv() {
            Ok(verdict) => verdict,
            Err(std::sync::mpsc::TryRecvError::Empty) => continue, // still in flight
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                ChainCheckResult::RpcError("balance check thread died".to_string())
            }
        };
        commands.entity(entity).remove::<PendingChainCheck>();

        match verdict {
            ChainCheckResult::Funded { available_lamports } => {
                info!(
                    "[RESPAWN] Player {} funded on-chain ({} lamports available) — authorized",
                    player_id.0, available_lamports
                );
                let living_positions: Vec<Vec3> = living_query
                    .iter()
                    .map(|p| p.0)
                    .collect();
                revive_player(
                    entity, player_id, &mut health, &mut position, &mut rotation,
                    &mut equipped, &mut inventory, &living_positions, &mut commands,
                );
            }
            ChainCheckResult::InsufficientFunds { required_lamports, available_lamports } => {
                warn!(
                    "[RESPAWN] Player {} denied — insufficient funds ({} available, {} required lamports)",
                    player_id.0, available_lamports, required_lamports
                );
                // Re-queue with a retry delay — player may fund wallet
                pending.timers.push((entity, now + RESPAWN_RETRY_DELAY));
            }
            ChainCheckResult::RpcError(e) => {
                warn!(
                    "[RESPAWN] Player {} — balance check RPC failed ({}); failing closed, will retry",
                    player_id.0, e
                );
                pending.timers.push((entity, now + RESPAWN_RETRY_DELAY));
            }
        }
    }
}

// ========================================
// Wallet Auth Verification
// ========================================

/// Process incoming wallet auth messages from clients.
/// Reads WalletAuthMessage from each client's MessageReceiver, verifies the
/// Ed25519 signature, and maps the pubkey -> Solana wallet address on the player entity.
/// Session IDs queued for a kick because the same wallet authenticated on a
/// newer link. Filled by `process_wallet_auth`, drained by `kick_superseded_sessions`.
///
/// Two phases because the kick needs `&mut Link`/`Disconnecting` on a DIFFERENT
/// ClientOf entity than the one being iterated during auth processing.
#[derive(Resource, Default)]
struct SupersededSessions {
    ids: Vec<u64>,
}

/// Disconnect sessions superseded by a newer authenticated session of the same
/// wallet.
///
/// Inserting `Disconnecting` is lightyear 0.26's supported single-client kick —
/// it is exactly what `NetcodeServerPlugin::stop` does per client. In `Last`,
/// `lightyear_connection::server::ConnectionPlugin::disconnect` turns it into
/// `Disconnected { reason: None }` and despawns the link entity. Inserting
/// `Disconnected` directly (or despawning the entity outright) does NOT work:
/// the former leaves a zombie the netcode layer keeps alive forever, and the
/// latter skips `ControlledBy::handle_disconnection` entirely, orphaning the
/// player entity we are trying to remove.
///
/// The `Disconnected` insert is what despawns the stale player entity, via
/// `ControlledBy { lifetime: SessionBased }` — that despawn replicates, so other
/// clients stop seeing the ghost body. The kicked client is not sent a netcode
/// disconnect packet (that path is `pub(crate)`), so it notices on its own
/// timeout; harmless here because the kicked session is already dead or healing.
fn kick_superseded_sessions(
    mut superseded: ResMut<SupersededSessions>,
    client_query: Query<(Entity, &RemoteId), With<ClientOf>>,
    mut commands: Commands,
) {
    if superseded.ids.is_empty() {
        return;
    }
    for old_id in superseded.ids.drain(..) {
        let Some((entity, _)) = client_query
            .iter()
            .find(|(_, remote_id)| remote_id.0.to_bits() == old_id)
        else {
            // Already gone (timed out on its own) — nothing to kick.
            info!("[KICK-OLD] Session {} already gone, no kick needed", old_id);
            continue;
        };
        info!("[KICK-OLD] Disconnecting superseded session {} ({:?})", old_id, entity);
        commands
            .entity(entity)
            .insert(lightyear::connection::client::Disconnecting);
    }
}

fn process_wallet_auth(
    mut client_query: Query<(&RemoteId, &mut MessageReceiver<WalletAuthMessage>), With<ClientOf>>,
    mut player_query: Query<(&PlayerId, &mut WalletAddress)>,
    mut verified_wallets: ResMut<VerifiedWallets>,
    mut superseded: ResMut<SupersededSessions>,
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

            // The signature proves ownership of the WALLET, so it is verified
            // against the wallet-derived ID from the message's own pubkey — NOT
            // against the connection's netcode ID, which is now a random
            // per-session handle (see `mint_session_id` in client.rs). The
            // netcode ID and the wallet identity are deliberately decoupled;
            // `verify_auth_signature`'s internal pubkey→ID check still holds
            // because we hand it the derived ID.
            let signed_id = auth::pubkey_to_client_id(&auth_msg.pubkey);

            match auth::verify_auth_signature(
                &auth_msg.pubkey,
                &auth_msg.signature,
                signed_id,
            ) {
                Ok(wallet_address) => {
                    info!(
                        "[AUTH] Wallet VERIFIED for client {}: {}",
                        client_id_bits, wallet_address
                    );

                    // KICK-OLD: this wallet just PROVED itself on this session,
                    // so any other live session holding the same wallet is a
                    // stale predecessor (killed client, crash, self-heal) and is
                    // queued for disconnect. Gating on the verified signature is
                    // what makes this safe: connect tokens are minted with an
                    // all-zero private key, so anyone can forge a token for any
                    // ID — kicking on raw ID collision would let anyone boot any
                    // player. You can only kick sessions of a wallet you own.
                    let stale: Vec<u64> = verified_wallets
                        .wallets
                        .iter()
                        .filter(|(id, addr)| **id != client_id_bits && *addr == &wallet_address)
                        .map(|(id, _)| *id)
                        .collect();
                    for old_id in stale {
                        warn!(
                            "[KICK-OLD] Wallet {} re-authenticated on session {}; \
                             disconnecting superseded session {}",
                            wallet_address, client_id_bits, old_id
                        );
                        verified_wallets.remove(old_id);
                        superseded.ids.push(old_id);
                    }

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

// ========================================
// Input lag diagnostics
// ========================================

/// Measures how late client inputs arrive relative to the tick being simulated.
/// `lag = simulated_tick - last_received_input_tick`:
///   lag <= 0 → the input for this tick had already arrived (healthy)
///   lag  > 0 → simulating with a STALE input. Harmless while input is constant,
///              but accumulates position drift whenever input is changing
///              (turning while running, jump presses) — the client predicted with
///              the new input, the server simulated with the old one.
/// Logged once per second per connected player.
fn log_input_lag(
    timeline: Res<LocalTimeline>,
    query: Query<(
        &PlayerId,
        &InputBuffer<LeafwingSnapshot<PlayerActions>, PlayerActions>,
    )>,
    mut acc: Local<std::collections::HashMap<u64, (i64, i64, u32, u32)>>,
    mut ticks: Local<u32>,
) {
    let tick = timeline.tick();
    for (pid, buffer) in query.iter() {
        let (sum, max, samples, stale) = acc.entry(pid.0).or_default();
        if let Some(remote) = buffer.last_remote_tick {
            let lag = (tick - remote) as i64;
            *sum += lag;
            *max = (*max).max(lag);
            *samples += 1;
            if lag > 0 {
                *stale += 1;
            }
        }
    }

    *ticks += 1;
    if *ticks >= 64 {
        for (pid, (sum, max, samples, stale)) in acc.drain() {
            if samples == 0 {
                continue;
            }
            info!(
                "[INPUT LAG] player={} avg={:.1} ticks, max={} ticks, stale={}/{} ticks ({:.0}%)",
                pid,
                sum as f64 / samples as f64,
                max,
                stale,
                samples,
                stale as f64 * 100.0 / samples as f64,
            );
        }
        *ticks = 0;
    }
}

// ========================================
// Headless integration tests: death → respawn gate
// ========================================
//
// These prove the pay-to-spawn flow server-side with no networking, no
// physics, and no renderer. Game time is advanced manually for determinism;
// on-chain balance checks run against an in-process mock provider, so the
// full async dispatch → poll → verdict path is exercised for real.
#[cfg(test)]
mod respawn_gate_tests {
    use super::*;
    use avian3d::prelude::Rotation;
    use multiplayer::solana::{BalanceProvider, RespawnConfig};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const WALLET: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    fn paid_config() -> RespawnConfig {
        RespawnConfig {
            require_payment: true,
            ..RespawnConfig::default()
        }
    }

    /// Mock chain: shared mutable balances, so tests can "fund a wallet"
    /// while the app is running (die broke → top up → respawn).
    /// Unknown addresses have balance 0 — same as a real cluster.
    #[derive(Clone, Default)]
    struct MockChain(Arc<Mutex<HashMap<String, u64>>>);

    impl MockChain {
        fn set_balance(&self, address: &str, lamports: u64) {
            self.0.lock().unwrap().insert(address.to_string(), lamports);
        }
    }

    impl BalanceProvider for MockChain {
        fn get_balance(&self, address: &str) -> Result<u64, String> {
            Ok(*self.0.lock().unwrap().get(address).unwrap_or(&0))
        }
    }

    /// MockChain behind a latch: `get_balance` blocks while the test holds
    /// the latch guard. Makes the in-flight `PendingChainCheck` state
    /// deterministic — without it, the instant mock can reply before the
    /// same-tick `poll_chain_checks`, and asserts on the transient state race.
    struct LatchedChain {
        inner: MockChain,
        gate: Arc<Mutex<()>>,
    }

    impl BalanceProvider for LatchedChain {
        fn get_balance(&self, address: &str) -> Result<u64, String> {
            let _open = self.gate.lock().unwrap();
            self.inner.get_balance(address)
        }
    }

    /// Provider that fails every request — simulates an RPC outage.
    struct DeadRpc;
    impl BalanceProvider for DeadRpc {
        fn get_balance(&self, _address: &str) -> Result<u64, String> {
            Err("connection refused".to_string())
        }
    }

    /// Provider that panics if consulted — proves a code path never
    /// touches the chain (dev mode must work with no RPC at all).
    struct NoChainAllowed;
    impl BalanceProvider for NoChainAllowed {
        fn get_balance(&self, _address: &str) -> Result<u64, String> {
            panic!("this code path must never consult the chain");
        }
    }

    /// Headless app with only the death/respawn systems and their resources.
    fn gate_app_with(config: RespawnConfig, provider: Arc<dyn BalanceProvider>) -> App {
        let mut app = App::new();
        app.insert_resource(Time::<()>::default());
        app.init_resource::<PendingRespawns>();
        app.init_resource::<VerifiedWallets>();
        app.insert_resource(config);
        app.insert_resource(ChainVerifier(provider));
        app.add_systems(
            Update,
            (kill_plane, check_player_death, process_respawns, poll_chain_checks).chain(),
        );
        app
    }

    fn gate_app(config: RespawnConfig) -> App {
        gate_app_with(config, Arc::new(NoChainAllowed))
    }

    fn verify_wallet(app: &mut App, client_id: u64, address: &str) {
        app.world_mut()
            .resource_mut::<VerifiedWallets>()
            .wallets
            .insert(client_id, address.to_string());
    }

    /// Run update cycles (without advancing game time) until the in-flight
    /// chain check on `e` resolves. Panics if it never does.
    fn settle_chain_check(app: &mut App, e: Entity) {
        for _ in 0..400 {
            if !app.world().entity(e).contains::<PendingChainCheck>() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            app.update();
        }
        panic!("chain check did not resolve within ~2s");
    }

    /// Spawn a living player with the full component set the death/respawn
    /// systems query for.
    fn spawn_player(app: &mut App, client_id: u64) -> Entity {
        app.world_mut()
            .spawn((
                PlayerId(client_id),
                PlayerDisplayId(client_id as u32),
                LastDamagedBy(0),
                PlayerHealth(100),
                Position(Vec3::new(0.0, 1.0, 0.0)),
                Rotation(Quat::IDENTITY),
                PlayerEquipped(None),
                PlayerInventory::default(),
            ))
            .id()
    }

    fn advance(app: &mut App, secs: f32) {
        app.world_mut()
            .resource_mut::<Time<()>>()
            .advance_by(std::time::Duration::from_secs_f32(secs));
        app.update();
    }

    fn is_dead(app: &App, e: Entity) -> bool {
        app.world().entity(e).contains::<PlayerDead>()
    }

    fn health(app: &App, e: Entity) -> i32 {
        app.world().entity(e).get::<PlayerHealth>().unwrap().0
    }

    fn kill(app: &mut App, e: Entity) {
        app.world_mut()
            .entity_mut(e)
            .get_mut::<PlayerHealth>()
            .unwrap()
            .0 = 0;
    }

    // ---- Dev mode (default): free respawns ----

    #[test]
    fn dev_mode_full_death_to_respawn_loop() {
        let mut app = gate_app(RespawnConfig::default());
        let e = spawn_player(&mut app, 1);

        // Give the player loot so death has something to drop.
        {
            let mut em = app.world_mut().entity_mut(e);
            em.get_mut::<PlayerEquipped>().unwrap().0 = Some("ak47".into());
            em.get_mut::<PlayerInventory>().unwrap().items.push("pickaxe".into());
        }

        kill(&mut app, e);
        advance(&mut app, 0.0);

        // Death processed: marked dead, loot stripped, respawn queued.
        assert!(is_dead(&app, e), "health 0 must mark PlayerDead");
        assert_eq!(app.world().entity(e).get::<PlayerEquipped>().unwrap().0, None);
        assert!(app.world().entity(e).get::<PlayerInventory>().unwrap().items.is_empty());
        assert_eq!(app.world().resource::<PendingRespawns>().timers.len(), 1);

        // Before the delay elapses: still dead.
        advance(&mut app, RESPAWN_DELAY - 0.5);
        assert!(is_dead(&app, e), "must stay dead before RESPAWN_DELAY elapses");
        assert_eq!(health(&app, e), 0);

        // After the delay: alive, restored, timer consumed.
        advance(&mut app, 1.0);
        assert!(!is_dead(&app, e), "must respawn after RESPAWN_DELAY in dev mode");
        assert_eq!(health(&app, e), 100);
        assert_eq!(app.world().entity(e).get::<Rotation>().unwrap().0, Quat::IDENTITY);
        assert!(app.world().resource::<PendingRespawns>().timers.is_empty());
    }

    #[test]
    fn kill_plane_triggers_death() {
        let mut app = gate_app(RespawnConfig::default());
        let e = spawn_player(&mut app, 1);

        app.world_mut().entity_mut(e).get_mut::<Position>().unwrap().0.y = KILL_PLANE_Y - 1.0;
        advance(&mut app, 0.0);

        assert!(is_dead(&app, e), "falling below the kill plane must kill");
        assert_eq!(health(&app, e), 0);
    }

    // ---- Payment mode: the pay-to-spawn gate ----

    #[test]
    fn payment_mode_unverified_wallet_stays_dead_and_requeues() {
        let mut app = gate_app(paid_config());
        let e = spawn_player(&mut app, 7);
        kill(&mut app, e);
        advance(&mut app, 0.0);

        advance(&mut app, RESPAWN_DELAY + 0.1);
        assert!(is_dead(&app, e), "unverified wallet must not respawn in payment mode");
        assert_eq!(
            app.world().resource::<PendingRespawns>().timers.len(),
            1,
            "denied respawn must re-queue for retry"
        );

        // Still dead across multiple retry windows.
        advance(&mut app, 30.0);
        assert!(is_dead(&app, e));
        assert_eq!(app.world().resource::<PendingRespawns>().timers.len(), 1);
    }

    // ---- Payment mode: on-chain balance gate (async, mocked chain) ----

    #[test]
    fn payment_mode_funded_wallet_respawns_after_chain_check() {
        let chain = MockChain::default();
        let config = paid_config();
        chain.set_balance(WALLET, config.respawn_cost_lamports);
        let gate = Arc::new(Mutex::new(()));
        let mut app = gate_app_with(
            config,
            Arc::new(LatchedChain { inner: chain, gate: gate.clone() }),
        );
        let e = spawn_player(&mut app, 7);
        verify_wallet(&mut app, 7, WALLET);

        // Hold the latch: the chain check stays in flight until we release it.
        let hold = gate.lock().unwrap();

        kill(&mut app, e);
        advance(&mut app, 0.0);
        advance(&mut app, RESPAWN_DELAY + 0.1);

        // Timer expired → async chain check dispatched, still dead meanwhile.
        assert!(
            app.world().entity(e).contains::<PendingChainCheck>(),
            "verified wallet in payment mode must dispatch a chain check"
        );
        assert!(is_dead(&app, e), "must stay dead until the chain check resolves");

        drop(hold);
        settle_chain_check(&mut app, e);
        assert!(!is_dead(&app, e), "funded wallet must respawn once the chain confirms");
        assert_eq!(health(&app, e), 100);
        assert!(app.world().resource::<PendingRespawns>().timers.is_empty());
    }

    #[test]
    fn payment_mode_underfunded_wallet_stays_dead_then_respawns_after_topup() {
        let chain = MockChain::default();
        let config = paid_config();
        let cost = config.respawn_cost_lamports;
        chain.set_balance(WALLET, cost - 1);
        let mut app = gate_app_with(config, Arc::new(chain.clone()));
        let e = spawn_player(&mut app, 7);
        verify_wallet(&mut app, 7, WALLET);

        kill(&mut app, e);
        advance(&mut app, 0.0);
        advance(&mut app, RESPAWN_DELAY + 0.1);
        settle_chain_check(&mut app, e);

        // InsufficientFunds: stays dead, re-queued for retry.
        assert!(is_dead(&app, e), "underfunded wallet must not respawn");
        assert_eq!(
            app.world().resource::<PendingRespawns>().timers.len(),
            1,
            "denied respawn must re-queue for retry"
        );

        // Player funds their wallet, next retry window authorizes.
        chain.set_balance(WALLET, cost);
        advance(&mut app, RESPAWN_RETRY_DELAY + 0.1);
        settle_chain_check(&mut app, e);
        assert!(!is_dead(&app, e), "topped-up wallet must respawn on retry");
        assert_eq!(health(&app, e), 100);
    }

    #[test]
    fn payment_mode_rpc_outage_fails_closed() {
        let mut app = gate_app_with(paid_config(), Arc::new(DeadRpc));
        let e = spawn_player(&mut app, 7);
        verify_wallet(&mut app, 7, WALLET);

        kill(&mut app, e);
        advance(&mut app, 0.0);
        advance(&mut app, RESPAWN_DELAY + 0.1);
        settle_chain_check(&mut app, e);

        // RPC down → deny and retry. An outage must never grant free respawns.
        assert!(is_dead(&app, e), "RPC failure must fail closed, not authorize");
        assert_eq!(
            app.world().resource::<PendingRespawns>().timers.len(),
            1,
            "RPC failure must re-queue for retry"
        );

        // Still failing across another retry window.
        advance(&mut app, RESPAWN_RETRY_DELAY + 0.1);
        settle_chain_check(&mut app, e);
        assert!(is_dead(&app, e));
    }

    // ---- LIVE DEVNET E2E ----

    /// The full pay-to-spawn loop against REAL devnet: two players die; the
    /// one whose wallet holds >= respawn_cost on devnet respawns, the one
    /// with a fresh (0-balance) wallet stays dead. Ignored by default:
    /// network access + requires ~/.anima/keypair.json to be faucet-funded
    /// on devnet (`solana airdrop 1 <addr> --url devnet`).
    /// Run with: `cargo test --bin server -- --ignored`
    #[test]
    #[ignore = "hits real devnet RPC; needs faucet-funded ~/.anima/keypair.json"]
    fn devnet_e2e_funded_respawns_unfunded_stays_dead() {
        let config = RespawnConfig {
            require_payment: true,
            rpc_url: "https://api.devnet.solana.com".to_string(),
            ..RespawnConfig::default()
        };
        let verifier = ChainVerifier::json_rpc(&config.rpc_url);
        let mut app = gate_app_with(config, verifier.0);

        // Funded: the real client identity wallet (same keypair the
        // production client signs wallet-auth challenges with).
        let (_, pubkey) = auth::load_or_create_keypair(None);
        let funded_wallet = auth::pubkey_address(&pubkey);
        // Unfunded: a wallet that has never existed on devnet.
        let fresh = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let unfunded_wallet = auth::pubkey_address(&fresh.verifying_key().to_bytes());

        let funded = spawn_player(&mut app, 1);
        let unfunded = spawn_player(&mut app, 2);
        verify_wallet(&mut app, 1, &funded_wallet);
        verify_wallet(&mut app, 2, &unfunded_wallet);

        kill(&mut app, funded);
        kill(&mut app, unfunded);
        advance(&mut app, 0.0);
        advance(&mut app, RESPAWN_DELAY + 0.1);

        settle_chain_check(&mut app, funded);
        settle_chain_check(&mut app, unfunded);

        assert!(
            !is_dead(&app, funded),
            "devnet-funded wallet {funded_wallet} must respawn (is it faucet-funded?)"
        );
        assert_eq!(health(&app, funded), 100);
        assert!(
            is_dead(&app, unfunded),
            "fresh 0-balance wallet {unfunded_wallet} must stay dead"
        );
        assert_eq!(
            app.world().resource::<PendingRespawns>().timers.len(),
            1,
            "denied player must be re-queued for retry"
        );
    }
}

// ========================================
// Kill-feed display names
// ========================================
//
// These pin the naming rule that replaced client-id-derived names: the netcode
// client id became random per connection in #19, so naming from it gave a player
// a different name every session.
#[cfg(test)]
mod display_name_tests {
    use super::*;

    const WALLET: &str = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

    fn wallet(address: &str) -> WalletAddress {
        WalletAddress(address.to_string())
    }

    #[test]
    fn verified_wallet_is_truncated_to_a_short_name() {
        let name = player_display_name(&wallet(WALLET), &PlayerDisplayId(3));
        assert_eq!(name, "7xKXtg2C");
        assert_eq!(name.chars().count(), WALLET_NAME_CHARS);
    }

    /// The whole point of the change: the same wallet yields the same name no
    /// matter which session it is on.
    #[test]
    fn same_wallet_yields_same_name_across_sessions() {
        let first = player_display_name(&wallet(WALLET), &PlayerDisplayId(1));
        let reconnected = player_display_name(&wallet(WALLET), &PlayerDisplayId(7));
        assert_eq!(first, reconnected, "name must survive a reconnect");
    }

    #[test]
    fn different_wallets_yield_different_names() {
        let a = player_display_name(&wallet(WALLET), &PlayerDisplayId(1));
        let b = player_display_name(
            &wallet("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"),
            &PlayerDisplayId(1),
        );
        assert_ne!(a, b);
    }

    #[test]
    fn unverified_wallet_falls_back_to_player_number() {
        assert_eq!(
            player_display_name(&WalletAddress::default(), &PlayerDisplayId(4)),
            "Player 4"
        );
    }

    /// A whitespace-only address is treated as unverified, not rendered blank.
    #[test]
    fn blank_wallet_falls_back_to_player_number() {
        assert_eq!(
            player_display_name(&wallet("   "), &PlayerDisplayId(2)),
            "Player 2"
        );
    }

    /// A wallet shorter than the truncation length must not panic.
    #[test]
    fn short_wallet_is_returned_whole() {
        assert_eq!(player_display_name(&wallet("abc"), &PlayerDisplayId(1)), "abc");
    }

    /// Truncation is char-wise, so a non-ASCII address cannot panic on a byte
    /// boundary (base58 is always ASCII, but the component is a plain String).
    #[test]
    fn non_ascii_wallet_does_not_panic() {
        let name = player_display_name(&wallet("日本語テストアドレス"), &PlayerDisplayId(1));
        assert_eq!(name.chars().count(), WALLET_NAME_CHARS);
    }

    /// Environmental deaths must never render the base58 of id 0 ("11111111"),
    /// which is what the previous client-id-derived naming produced.
    #[test]
    fn no_killer_sentinel_is_not_a_base58_id() {
        assert_eq!(NO_KILLER, 0);
        let old_behavior = multiplayer::auth::client_id_to_base58(NO_KILLER);
        assert_ne!(ENVIRONMENT_KILLER, old_behavior);
        assert_eq!(ENVIRONMENT_KILLER, "the void");
    }
}
