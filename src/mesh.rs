//! Server mesh integration for ANIMA.
//!
//! Wires lightyear_mesh into the game server. Configurable via environment variables:
//! - `MESH_ZONE`: Zone bounds as "min_x,max_x" (default: no mesh, single server mode)
//! - `MESH_PORT`: UDP port for mesh communication (default: 6000)
//! - `MESH_NEIGHBORS`: Semicolon-separated neighbor configs as "id:addr:zone_min:zone_max"
//!   e.g. "2:127.0.0.1:6001:500:1000" (default: none)
//! - `MESH_SERVER_ID`: This server's ID, 1-65535 (default: 1)

use std::net::SocketAddr;

use avian3d::prelude::*;
use bevy::ecs::schedule::common_conditions::run_once;
use bevy::prelude::*;
use lightyear::prelude::*;
use lightyear_mesh::prelude::*;
use lightyear_mesh::connection::initiate_handshake;
use lightyear_mesh::dual_sim::BoundaryAxis;

use crate::protocol::{CharacterVelocity, PlayerHealth};

/// Resource indicating mesh is active (zone config was provided).
#[derive(Resource)]
pub struct MeshActive;

/// Resource holding the entity ID generator for this server.
#[derive(Resource)]
pub struct MeshIdGen(pub lightyear_mesh::entity_id::MeshEntityIdGenerator);

/// Mesh configuration parsed from environment variables.
#[derive(Resource, Clone, Debug)]
pub struct MeshConfig {
    pub server_id: u16,
    pub zone_min_x: f32,
    pub zone_max_x: f32,
    pub mesh_port: u16,
    pub neighbors: Vec<NeighborConfig>,
}

#[derive(Clone, Debug)]
pub struct NeighborConfig {
    pub id: u16,
    /// Mesh transport address (server-to-server UDP)
    pub addr: SocketAddr,
    /// Client-facing game address (lightyear connection)
    pub game_addr: SocketAddr,
    pub zone_min_x: f32,
    pub zone_max_x: f32,
}

impl MeshConfig {
    /// Parse mesh config from environment variables. Returns None if MESH_ZONE is not set
    /// (single-server mode).
    pub fn from_env() -> Option<Self> {
        let zone = std::env::var("MESH_ZONE").ok()?;
        let parts: Vec<&str> = zone.split(',').collect();
        if parts.len() != 2 {
            warn!("MESH_ZONE must be 'min_x,max_x', got: {}", zone);
            return None;
        }
        let zone_min_x: f32 = parts[0].parse().ok()?;
        let zone_max_x: f32 = parts[1].parse().ok()?;

        let server_id: u16 = std::env::var("MESH_SERVER_ID")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);

        let mesh_port: u16 = std::env::var("MESH_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6000);

        let neighbors = std::env::var("MESH_NEIGHBORS")
            .ok()
            .map(|s| parse_neighbors(&s))
            .unwrap_or_default();

        Some(MeshConfig {
            server_id,
            zone_min_x,
            zone_max_x,
            mesh_port,
            neighbors,
        })
    }
}

fn parse_neighbors(s: &str) -> Vec<NeighborConfig> {
    // Format: "id,mesh_addr,game_addr,zone_min,zone_max" separated by semicolons
    // e.g. "2,127.0.0.1:6001,127.0.0.1:5001,0,500"
    s.split(';')
        .filter(|n| !n.is_empty())
        .filter_map(|n| {
            let parts: Vec<&str> = n.split(',').collect();
            if parts.len() != 5 {
                warn!("Neighbor format: 'id,mesh_addr,game_addr,zone_min,zone_max', got: {}", n);
                return None;
            }
            let id: u16 = parts[0].parse().ok()?;
            let addr: SocketAddr = parts[1].parse().ok()?;
            let game_addr: SocketAddr = parts[2].parse().ok()?;
            let zone_min_x: f32 = parts[3].parse().ok()?;
            let zone_max_x: f32 = parts[4].parse().ok()?;
            Some(NeighborConfig { id, addr, game_addr, zone_min_x, zone_max_x })
        })
        .collect()
}

/// Lightweight plugin for clients — just registers MeshControlled so
/// lightyear can deserialize it from server replication.
pub struct GameMeshClientPlugin;

impl Plugin for GameMeshClientPlugin {
    fn build(&self, app: &mut App) {
        app.register_component::<MeshControlled>();
    }
}

/// Plugin that sets up mesh networking on the server.
/// Only activates if MESH_ZONE environment variable is set.
pub struct GameMeshPlugin;

impl Plugin for GameMeshPlugin {
    fn build(&self, app: &mut App) {
        let Some(config) = MeshConfig::from_env() else {
            info!("MESH_ZONE not set — running in single-server mode (no mesh)");
            return;
        };

        info!(
            "Mesh enabled: server_id={}, zone=[{}, {}], mesh_port={}, neighbors={}",
            config.server_id, config.zone_min_x, config.zone_max_x,
            config.mesh_port, config.neighbors.len()
        );

        // Add the mesh networking plugin
        app.add_plugins(MeshPlugin);

        // Register mesh components — must match on all servers in same order
        // All player-relevant state that needs to cross server boundaries
        app.register_mesh_component::<Position>();
        app.register_mesh_component::<Rotation>();
        app.register_mesh_component::<CharacterVelocity>();
        app.register_mesh_component::<PlayerHealth>();
        app.register_mesh_component::<crate::protocol::PlayerId>();
        app.register_mesh_component::<crate::protocol::PlayerYaw>();
        app.register_mesh_component::<crate::protocol::PlayerPitch>();
        app.register_mesh_component::<crate::protocol::PlayerEquipped>();
        app.register_mesh_component::<crate::protocol::LastDamagedBy>();
        app.register_mesh_component::<crate::protocol::PlayerDisplayId>();
        app.register_mesh_component::<MeshControlled>();

        // Register MeshControlled for lightyear replication (server → client)
        // so the client knows which server is authoritative
        app.register_component::<MeshControlled>();

        // Resources
        let id_gen = lightyear_mesh::entity_id::MeshEntityIdGenerator::new(config.server_id);
        app.insert_resource(MeshIdGen(id_gen));
        app.insert_resource(MeshLocalServerId(config.server_id));
        app.insert_resource(MeshActive);
        app.insert_resource(MeshDualSimConfig {
            hysteresis_claim_distance: 1.0,
            hysteresis_release_distance: 1.0,
            authority_cooldown_ticks: 0,
        });
        app.insert_resource(MeshDualSimCooldowns::default());
        app.insert_resource(config.clone());

        // Position extractor — tells the mesh system how to read entity positions
        // from avian3d's Position component
        app.insert_resource(MeshDualSimPositionFn::new(|entity_ref| {
            entity_ref.get::<Position>().map(|p| (p.x, p.z))
        }));

        // Startup: bind mesh UDP socket and connect to neighbors
        app.add_systems(Startup, setup_mesh_transport);

        // Initiate handshake on first update after startup
        app.add_systems(Update, initiate_mesh_handshakes.run_if(run_once));

        // Update MeshControlled.authority_server when dual-sim flips authority
        app.add_systems(PostUpdate, sync_mesh_controlled_authority);

        // Observer: when a MeshGhost is added, give it a collider
        // so cross-server combat works via existing avian lag comp infrastructure
        app.add_observer(on_ghost_added);
    }
}

/// Bind mesh UDP socket and initiate connections to configured neighbors.
fn setup_mesh_transport(
    mut commands: Commands,
    config: Res<MeshConfig>,
) {
    let bind_addr: SocketAddr = format!("0.0.0.0:{}", config.mesh_port).parse().unwrap();
    info!("Binding mesh UDP socket on {}", bind_addr);

    // Bind the mesh UDP socket
    let mut io = MeshUdpIo::default();
    io.bind(bind_addr).expect("Failed to bind mesh UDP socket");

    // Spawn neighbor entities first so we have their Entity IDs for routing
    let mut neighbor_entities = Vec::new();
    for neighbor in &config.neighbors {
        let neighbor_entity = commands.spawn((
            MeshNeighbor {
                id: neighbor.id,
                addr: neighbor.addr,
                boundary: BoundaryKind::Seamless,
            },
            MeshLink::default(),
        )).id();

        // Register neighbor address for incoming packet routing
        io.register_neighbor(neighbor.addr, neighbor_entity);
        neighbor_entities.push((neighbor.clone(), neighbor_entity));

        info!(
            "Mesh neighbor configured: id={}, addr={}, zone=[{}, {}], entity={:?}",
            neighbor.id, neighbor.addr, neighbor.zone_min_x, neighbor.zone_max_x, neighbor_entity
        );
    }

    // Spawn the IO entity with the bound socket
    commands.spawn(io);

    info!("Mesh transport ready: {} neighbors connected", neighbor_entities.len());
}

/// Initiate handshake with all mesh neighbors. Runs once on first update.
fn initiate_mesh_handshakes(
    mut neighbor_query: Query<(Entity, &MeshNeighbor, &mut MeshLink)>,
    config: Res<MeshConfig>,
    registry: Res<MeshComponentRegistry>,
    mut commands: Commands,
) {
    let registry_hash = registry.fingerprint();

    for (entity, neighbor, mut link) in neighbor_query.iter_mut() {
        let mut entity_commands = commands.entity(entity);
        initiate_handshake(
            &mut link,
            neighbor,
            config.server_id,
            registry_hash,
            &mut entity_commands,
        );
        info!("Initiated mesh handshake with neighbor {} at {}", neighbor.id, neighbor.addr);
    }
}

/// When this server releases authority (MeshAuthority removed), update
/// MeshControlled.authority_server to point to the neighbor that claimed it.
/// This is how the client learns about authority changes — Server 1 (still
/// connected to the client) tells the client "go to Server 2."
fn sync_mesh_controlled_authority(
    mut query: Query<(&mut MeshControlled, Has<MeshAuthority>, &MeshDualSim, &MeshEntityId, Option<&Position>)>,
    config: Res<MeshConfig>,
    mut log_timer: Local<f32>,
    time: Res<Time>,
) {
    *log_timer += time.delta_secs();
    for (mut controlled, has_authority, dual_sim, mesh_id, pos) in query.iter_mut() {
        // Periodic debug log
        if *log_timer > 5.0 {
            let pos_str = pos.map(|p| format!("x={:.1}", p.x)).unwrap_or("no pos".into());
            info!(
                "[MESH-DEBUG] Entity {:?} auth={} authority_server={} {} boundaries={:?}",
                mesh_id, has_authority, controlled.authority_server, pos_str, dual_sim.boundaries.len()
            );
            *log_timer = 0.0;
        }
        if has_authority {
            // We have authority — make sure MeshControlled reflects us
            if controlled.authority_server != config.server_id {
                info!(
                    "[MESH] Claimed authority — updating MeshControlled: {} -> {}",
                    controlled.authority_server, config.server_id
                );
                controlled.authority_server = config.server_id;
            }
        } else if controlled.authority_server == config.server_id {
            // We LOST authority but MeshControlled still says we're authoritative.
            // Update it to point to the neighbor. For now, pick the first neighbor
            // (in a multi-neighbor setup, we'd need to determine which one claimed).
            if let Some(neighbor) = controlled.neighbors.first() {
                info!(
                    "[MESH] Released authority — updating MeshControlled: {} -> {}",
                    controlled.authority_server, neighbor.id
                );
                controlled.authority_server = neighbor.id;
            }
        }
    }
}

/// Observer: when a ghost entity arrives from a mesh neighbor, add physics
/// components so it can participate in lag compensation and collision.
fn on_ghost_added(
    trigger: On<Add, MeshGhost>,
    mut commands: Commands,
    config: Option<Res<MeshConfig>>,
) {
    let entity = trigger.event_target();

    // Add collider for lag comp
    commands.entity(entity).insert(
        Collider::capsule(0.5, 0.4),
    );

    // Add dual-sim boundary rule so this server can claim authority
    // if the ghost crosses into our zone
    if let Some(config) = config {
        let boundary_x = if config.zone_min_x >= 0.0 {
            config.zone_min_x
        } else {
            config.zone_max_x
        };
        let my_side_positive = config.zone_min_x >= boundary_x;
        commands.entity(entity).insert(
            MeshDualSim::single(BoundaryAxis::X, boundary_x, my_side_positive),
        );
    }

    info!("Ghost entity {:?} received — added collider + dual-sim", entity);
}
