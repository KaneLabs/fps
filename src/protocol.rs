use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::input::prelude::InputConfig;
use lightyear::prelude::input::leafwing;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// --- Replicated Components ---

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(pub u64);

/// The player's camera yaw, replicated so the server can compute camera-relative movement.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct PlayerYaw(pub f32);

// VectorSpace impls for PlayerYaw/PlayerPitch so lightyear can interpolate them on remote clients.
macro_rules! impl_vector_space_f32 {
    ($T:ident) => {
        impl std::ops::Add for $T {
            type Output = Self;
            fn add(self, rhs: Self) -> Self { Self(self.0 + rhs.0) }
        }
        impl std::ops::Sub for $T {
            type Output = Self;
            fn sub(self, rhs: Self) -> Self { Self(self.0 - rhs.0) }
        }
        impl std::ops::Mul<f32> for $T {
            type Output = Self;
            fn mul(self, rhs: f32) -> Self { Self(self.0 * rhs) }
        }
        impl std::ops::Div<f32> for $T {
            type Output = Self;
            fn div(self, rhs: f32) -> Self { Self(self.0 / rhs) }
        }
        impl std::ops::Neg for $T {
            type Output = Self;
            fn neg(self) -> Self { Self(-self.0) }
        }
        impl bevy::math::VectorSpace for $T {
            type Scalar = f32;
            const ZERO: Self = Self(0.0);
        }
    };
}

impl_vector_space_f32!(PlayerYaw);
impl_vector_space_f32!(PlayerPitch);

/// The player's camera pitch, replicated so remote clients can tilt the player model.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
pub struct PlayerPitch(pub f32);

// --- Input Actions (leafwing) ---
//
// Single enum replaces the per-struct BEI actions. `ActionState<PlayerActions>`
// is queried each FixedUpdate tick — leafwing+lightyear snapshot/restore it
// cleanly across rollback, which BEI's Fire<Action> observers could not.

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone, Copy, Hash, Reflect)]
pub enum PlayerActions {
    /// WASD (client) → Vec2. The client pre-rotates this by camera yaw BEFORE
    /// lightyear's BufferClientInputs captures it, so the value replicated to
    /// the server is already in world-space.
    Move,
    /// Mouse motion → Vec2 (x = yaw delta, y = pitch delta).
    Look,
    /// Space → jump
    Jump,
    /// E → interact (open door / pick up equippable)
    Interact,
    /// G → drop equipped item
    Drop,
    /// Q → left-hand jab (melee)
    Jab,
    /// Left mouse → primary action (shoot / mine depending on equipped item)
    Primary,
}

impl Actionlike for PlayerActions {
    fn input_control_kind(&self) -> InputControlKind {
        match self {
            Self::Move | Self::Look => InputControlKind::DualAxis,
            _ => InputControlKind::Button,
        }
    }
}

/// Tracks which tool a player has equipped. Replicated so the server
/// can validate mining and other players can see held items.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlayerEquipped(pub Option<String>);

/// Player inventory — list of carried item names (weapons, resources, etc).
/// Server-authoritative, replicated to all clients. The equipped item is NOT
/// in this list — it lives in PlayerEquipped. On death, all items (equipped +
/// inventory) drop as world Equippable entities at the death position.
/// These will eventually map to SPL tokens on Solana.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlayerInventory {
    pub items: Vec<String>,
}

/// Player health. Server-authoritative, replicated to all clients.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerHealth(pub i32);

impl Default for PlayerHealth {
    fn default() -> Self {
        Self(100)
    }
}

/// Sequential display ID (Player 1, Player 2, etc). Assigned by server on connect.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlayerDisplayId(pub u32);

/// Tracks who last dealt damage to this player. Server sets this on hit.
/// Used by death system to determine killer for kill feed.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct LastDamagedBy(pub u64);

/// Replicated shot event — server sets this when a player fires.
/// Client watches for changes on remote players to spawn tracers.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct LastShot {
    pub muzzle: Vec3,
    pub hit_point: Vec3,
    pub tick: u32,
}

/// Marker: player is dead. Server-authoritative, replicated.
/// While dead: input is ignored, player cannot move/shoot/interact.
/// Removed by server on respawn (after timer + future payment gate).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlayerDead;

/// Kill feed entry. Server-authoritative, replicated to all clients.
/// Stores truncated base58 addresses for display.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KillFeedEntry {
    pub killer_name: String,
    pub victim_name: String,
    pub timestamp: f32,
}

// --- Wallet Auth (Solana Challenge-Response) ---

/// Lightyear channel for wallet authentication messages.
/// Reliable + ordered — auth must arrive and in sequence.
pub struct AuthChannel;

/// Client → Server: wallet auth proof.
/// Sent immediately after connection to prove ownership of the Ed25519 keypair.
///
/// The server verifies:
/// 1. pubkey_to_client_id(pubkey) == connection's client_id
/// 2. ed25519_verify(pubkey, "ANIMA_AUTH_v1:{client_id}", signature) passes
///
/// On success, the server stores the full Solana wallet address (base58 pubkey)
/// and attaches a WalletAddress component to the player entity.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WalletAuthMessage {
    /// The client's full 32-byte Ed25519 public key (= Solana wallet address bytes)
    pub pubkey: [u8; 32],
    /// 64-byte Ed25519 signature over "ANIMA_AUTH_v1:{client_id}"
    /// Stored as Vec<u8> because serde doesn't support [u8; 64] by default.
    pub signature: Vec<u8>,
}

// --- Protocol Plugin ---

pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        // Leafwing input replication via lightyear. ActionState<PlayerActions>
        // is captured each tick on the client, buffered + sent to the server,
        // and restored during rollback — which BEI's Fire<Action> observers
        // could not do cleanly.
        app.add_plugins(leafwing::InputPlugin::<PlayerActions> {
            config: InputConfig::<PlayerActions> {
                rebroadcast_inputs: true,
                // Include the client's InterpolationDelay in input messages
                // so the server can rewind to where the client saw targets when shooting
                lag_compensation: true,
                ..default()
            },
        });

        // Replicated components
        app.register_component::<PlayerId>();
        // Yaw/pitch: same treatment as Position/Rotation — predicted with rollback
        // threshold + smooth correction. Threshold is generous (0.1 rad ~5.7°) because
        // client and server run the same shared_look with the same input deltas, so
        // divergence is minimal. Correction smoothing makes any correction invisible.
        app.register_component::<PlayerYaw>()
            .add_prediction()
            .add_should_rollback(|a: &PlayerYaw, b: &PlayerYaw| (a.0 - b.0).abs() >= 0.1)
            .add_linear_interpolation()
            .enable_correction();
        app.register_component::<PlayerPitch>()
            .add_prediction()
            .add_should_rollback(|a: &PlayerPitch, b: &PlayerPitch| (a.0 - b.0).abs() >= 0.1)
            .add_linear_interpolation()
            .enable_correction();
        app.register_component::<PlayerEquipped>()
            .add_prediction();
        app.register_component::<PlayerInventory>();
        app.register_component::<PlayerHealth>();
        app.register_component::<LastShot>();
        app.register_component::<PlayerDisplayId>();
        app.register_component::<LastDamagedBy>();
        app.register_component::<PlayerDead>();
        app.register_component::<KillFeedEntry>();

        // Avian3d physics components with prediction + interpolation.
        // enable_correction() lets lightyear handle smooth corrections on Transform
        // directly (via PositionButInterpolateTransform mode).
        // add_should_rollback() prevents unnecessary rollbacks from floating-point noise.
        app.register_component::<Position>()
            .add_prediction()
            .add_should_rollback(position_should_rollback)
            .add_linear_interpolation()
            .enable_correction();

        app.register_component::<Rotation>()
            .add_prediction()
            .add_should_rollback(rotation_should_rollback)
            .add_linear_interpolation()
            .enable_correction();

        // Avian's authoritative dynamic-body velocities. Replicated + predicted
        // so the client can integrate locally between server snapshots and the
        // server's motion stays authoritative. Threshold absorbs ~4 ticks of
        // per-tick gravity delta.
        app.register_component::<LinearVelocity>()
            .add_prediction()
            .add_should_rollback(linear_velocity_should_rollback);
        app.register_component::<AngularVelocity>()
            .add_prediction()
            .add_should_rollback(angular_velocity_should_rollback);

        // World object components — replicated, server-authoritative (no prediction)
        app.register_component::<crate::world::DoorState>();
        app.register_component::<crate::world::Equippable>();
        app.register_component::<crate::world::Interactable>();

        // Solana wallet address — attached to player entity after auth verification
        app.register_component::<crate::solana::WalletAddress>();

        // --- Wallet Auth Channel + Message ---
        // Reliable ordered channel for auth handshake.
        // Client sends WalletAuthMessage immediately after connection.
        // Server verifies and maps pubkey → player entity.
        app.add_channel::<AuthChannel>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            send_frequency: Duration::default(),
            priority: 10.0,
        })
        .add_direction(NetworkDirection::Bidirectional);

        app.register_message::<WalletAuthMessage>()
            .add_direction(NetworkDirection::ClientToServer);
    }
}

// --- Rollback thresholds ---
// Prevent unnecessary rollbacks from floating-point noise.
// Only rollback if the server/client values differ by more than a small threshold.

// Threshold must exceed "one tick of expected motion" to avoid rollback thrashing
// when input arrival timing differs by a tick between client prediction and server.
// At 7 m/s move speed × 15.6ms tick = 10.9cm, plus margin for FMA/transcendental drift.
fn position_should_rollback(this: &Position, that: &Position) -> bool {
    (this.0 - that.0).length() >= 0.25 // 25cm — 2+ ticks of motion
}

fn rotation_should_rollback(this: &Rotation, that: &Rotation) -> bool {
    this.angle_between(*that) >= 0.05 // ~3°
}

// Per-tick velocity delta from gravity is ~0.15 m/s; threshold needs to be
// much larger than that to absorb input timing jitter without thrashing.
fn linear_velocity_should_rollback(this: &LinearVelocity, that: &LinearVelocity) -> bool {
    (this.0 - that.0).length() >= 2.0 // 2 m/s
}

// Locked rotation means angular velocity is effectively always zero on players,
// so almost any drift is noise; keep the gate permissive.
fn angular_velocity_should_rollback(this: &AngularVelocity, that: &AngularVelocity) -> bool {
    (this.0 - that.0).length() >= 1.0
}
