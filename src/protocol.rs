use avian3d::prelude::*;
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
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlayerYaw(pub f32);

/// The player's camera pitch, replicated so remote clients can tilt the player model.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
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

/// Left click → primary action (mine, shoot, etc. depending on equipped item)
#[derive(Debug, InputAction)]
#[action_output(bool)]
pub struct PrimaryAction;

/// Tracks which tool a player has equipped. Replicated so the server
/// can validate mining and other players can see held items.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PlayerEquipped(pub Option<String>);

/// Player health. Server-authoritative, replicated to all clients.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerHealth(pub i32);

impl Default for PlayerHealth {
    fn default() -> Self {
        Self(100)
    }
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
        app.register_input_action::<PrimaryAction>();

        // Replicated components
        app.register_component::<PlayerId>();
        app.register_component::<PlayerYaw>()
            .add_prediction();
        app.register_component::<PlayerPitch>()
            .add_prediction();
        app.register_component::<PlayerEquipped>()
            .add_prediction();
        app.register_component::<PlayerHealth>();

        // Avian3d physics components with prediction + interpolation
        // Matches lightyear FPS example: enable_correction() lets lightyear handle
        // smooth corrections on Transform directly (via PositionButInterpolateTransform mode).
        app.register_component::<Position>()
            .add_prediction()
            .add_linear_interpolation()
            .enable_correction();

        app.register_component::<Rotation>()
            .add_prediction()
            .add_linear_interpolation()
            .enable_correction();

        // Our kinematic velocity (replaces Avian's LinearVelocity for players)
        app.register_component::<CharacterVelocity>()
            .add_prediction();

        // World object components — replicated, server-authoritative (no prediction)
        app.register_component::<crate::world::DoorState>();
        app.register_component::<crate::world::Equippable>();
        app.register_component::<crate::world::Interactable>();
    }
}

