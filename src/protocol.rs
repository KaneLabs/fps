use std::time::Duration;

use avian3d::prelude::*;
use bevy::math::VectorSpace;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
use lightyear::prelude::input::bei::InputPlugin;
use lightyear::prelude::input::InputConfig;
use lightyear::prelude::input::InputRegistryExt;
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

/// Player velocity managed by our kinematic character controller.
/// Not Avian's LinearVelocity — we own this completely.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct CharacterVelocity(pub Vec3);

// --- BEI Input Context ---

/// Marker component identifying a player input context for BEI + lightyear.
/// Named PlayerContext to avoid collision with player::Player.
#[derive(Component, Serialize, Deserialize, Reflect, Clone, Debug, PartialEq)]
pub struct PlayerContext;

// --- Input Actions ---

/// WASD movement → Vec2 (x = right/left, y = forward/back)
#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct MoveAction;

/// Space → jump (bool, fires while held)
#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct JumpAction;

/// E → interact (bool, fires while held)
#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct InteractAction;

/// G → drop equipped item
#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct DropAction;

/// Q → left hand jab (melee)
#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct JabAction;

/// Left click → primary action (mine, shoot, etc. depending on equipped item)
#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct PrimaryAction;

/// Mouse motion → look delta (Vec2: x = yaw delta, y = pitch delta).
/// Replicated to server via lightyear's BEI input system so the server
/// knows the player's facing direction for hit detection.
#[derive(Debug, InputAction)]
#[action_output(Vec2)]
pub struct LookAction;

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
        // BEI input replication
        app.add_plugins(InputPlugin::<PlayerContext> {
            config: InputConfig::<PlayerContext> {
                rebroadcast_inputs: true,
                ..default()
            },
        });
        app.register_input_action::<MoveAction>();
        app.register_input_action::<JumpAction>();
        app.register_input_action::<InteractAction>();
        app.register_input_action::<DropAction>();
        app.register_input_action::<JabAction>();
        app.register_input_action::<PrimaryAction>();
        app.register_input_action::<LookAction>();

        // Replicated components
        app.register_component::<PlayerId>();
        app.register_component::<PlayerYaw>()
            .add_prediction()
            .add_linear_interpolation();
        app.register_component::<PlayerPitch>()
            .add_prediction()
            .add_linear_interpolation();
        app.register_component::<PlayerEquipped>()
            .add_prediction();
        app.register_component::<PlayerInventory>();
        app.register_component::<PlayerHealth>();
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

        // Our kinematic velocity (replaces Avian's LinearVelocity for players)
        app.register_component::<CharacterVelocity>()
            .add_prediction()
            .add_should_rollback(velocity_should_rollback);

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

fn position_should_rollback(this: &Position, that: &Position) -> bool {
    (this.0 - that.0).length() >= 0.01
}

fn rotation_should_rollback(this: &Rotation, that: &Rotation) -> bool {
    this.angle_between(*that) >= 0.01
}

fn velocity_should_rollback(this: &CharacterVelocity, that: &CharacterVelocity) -> bool {
    (this.0 - that.0).length() >= 0.01
}

