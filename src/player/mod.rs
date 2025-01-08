//! This example showcases a 3D first-person camera.
//!
//! The setup presented here is a very common way of organizing a first-person game
//! where the player can see their own arms. We use two industry terms to differentiate
//! the kinds of models we have:
//!
//! - The *view model* is the model that represents the player's body.
//! - The *world model* is everything else.
//!
//! ## Motivation
//!
//! The reason for this distinction is that these two models should be rendered with different field of views (FOV).
//! The view model is typically designed and animated with a very specific FOV in mind, so it is
//! generally *fixed* and cannot be changed by a player. The world model, on the other hand, should
//! be able to change its FOV to accommodate the player's preferences for the following reasons:
//! - *Accessibility*: How prone is the player to motion sickness? A wider FOV can help.
//! - *Tactical preference*: Does the player want to see more of the battlefield?
//!     Or have a more zoomed-in view for precision aiming?
//! - *Physical considerations*: How well does the in-game FOV match the player's real-world FOV?
//!     Are they sitting in front of a monitor or playing on a TV in the living room? How big is the screen?
//!
//! ## Implementation
//!
//! The `Player` is an entity holding two cameras, one for each model. The view model camera has a fixed
//! FOV of 70 degrees, while the world model camera has a variable FOV that can be changed by the player.
//!
//! We use different `RenderLayers` to select what to render.
//!
//! - The world model camera has no explicit `RenderLayers` component, so it uses the layer 0.
//!     All static objects in the scene are also on layer 0 for the same reason.
//! - The view model camera has a `RenderLayers` component with layer 1, so it only renders objects
//!     explicitly assigned to layer 1. The arm of the player is one such object.
//!     The order of the view model camera is additionally bumped to 1 to ensure it renders on top of the world model.
//! - The light source in the scene must illuminate both the view model and the world model, so it is
//!     assigned to both layers 0 and 1.
//!
//! ## Controls
//!
//! | Key Binding          | Action        |
//! |:---------------------|:--------------|
//! | mouse                | Look around   |
//! | arrow up             | Decrease FOV  |
//! | arrow down           | Increase FOV  |

use std::f32::consts::FRAC_PI_2;

use bevy::{
    color::palettes::tailwind, input::mouse::AccumulatedMouseMotion, pbr::NotShadowCaster,
    prelude::*, render::view::RenderLayers,
};
use bevy_rapier3d::prelude::*;
use bevy_renet::renet::{ClientId, RenetClient};
use serde::{Deserialize, Serialize};

use crate::{
    network::{ClientChannel, ClientInput, ControlledPlayer, CurrentClientId, ServerLobby},
    world::WorldModelCamera,
};

#[derive(Debug, Component)]
pub struct Player {
    pub id: ClientId,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, Component, Resource)]
pub struct PlayerInput {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

#[derive(Debug, Serialize, Deserialize, Event)]
pub enum PlayerCommand {
    BasicAttack { cast_at: Vec3 },
}

#[derive(Debug, Component, Deref, DerefMut)]
pub struct CameraSensitivity(Vec2);

impl Default for CameraSensitivity {
    fn default() -> Self {
        Self(
            // These factors are just arbitrary mouse sensitivity values.
            // It's often nicer to have a faster horizontal sensitivity than vertical.
            // We use a component for them so that we can make them user-configurable at runtime
            // for accessibility reasons.
            // It also allows you to inspect them in an editor if you `Reflect` the component.
            Vec2::new(0.003, 0.002),
        )
    }
}

pub const PLAYER_MOVE_SPEED: f32 = 5.0;

/// Used by the view model camera and the player's arm.
/// The light source belongs to both layers.
pub const VIEW_MODEL_RENDER_LAYER: usize = 1;

#[derive(Resource)]
pub struct CursorState {
    locked: bool,
}

impl Default for CursorState {
    fn default() -> Self {
        Self { locked: true }
    }
}

pub fn spawn_view_model(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    client_id: Res<CurrentClientId>,
) {
    commands
        .spawn((
            Player { id: client_id.0 },
            CameraSensitivity::default(),
            Transform::from_xyz(0.0, 1.0, 0.0),
            RigidBody::Dynamic,
            Collider::capsule(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0), 0.5),
            Velocity::default(),
            LockedAxes::ROTATION_LOCKED,
            Friction::coefficient(0.0),
            GravityScale(1.0),
        ))
        .with_children(|parent| {
            // Just one camera with fixed FOV
            parent.spawn(Camera3dBundle {
                projection: Projection::Perspective(PerspectiveProjection {
                    fov: 90.0_f32.to_radians(),
                    ..default()
                }),
                ..default()
            });
        });
}

pub fn move_player(
    accumulated_mouse_motion: Res<AccumulatedMouseMotion>,
    mut query: Query<(&mut Transform, &CameraSensitivity), With<ControlledPlayer>>,
    mut client: ResMut<RenetClient>,
    time: Res<Time>,
    mut last_sent: Local<f32>,
) {
    if let Ok((mut transform, camera_sensitivity)) = query.get_single_mut() {
        let delta = accumulated_mouse_motion.delta;

        if delta != Vec2::ZERO {
            let delta_yaw = -delta.x * camera_sensitivity.x;
            let delta_pitch = -delta.y * camera_sensitivity.y;

            let (mut yaw, mut pitch, roll) = transform.rotation.to_euler(EulerRot::YXZ);
            yaw += delta_yaw;

            // Prevent looking too far up/down
            const PITCH_LIMIT: f32 = FRAC_PI_2 - 0.01;
            pitch = (pitch + delta_pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT);

            transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll);

            // Send rotation updates at most 20 times per second
            if time.elapsed_secs() - *last_sent > 0.05 && client.is_connected() {
                let input = ClientInput::Rotation(transform.rotation);
                let message = bincode::serialize(&input).unwrap();
                client.send_message(ClientChannel::Input, message);
                *last_sent = time.elapsed_secs();
            }
        }
    }
}

pub fn move_player_body(
    mut query: Query<&mut Transform, With<ControlledPlayer>>,
    player_input: Res<PlayerInput>,
    time: Res<Time>,
    mut client: ResMut<RenetClient>,
    mut last_sent: Local<f32>,
) {
    if let Ok(mut transform) = query.get_single_mut() {
        let x = (player_input.right as i8 - player_input.left as i8) as f32;
        let z = (player_input.down as i8 - player_input.up as i8) as f32;

        if x != 0.0 || z != 0.0 {
            let forward = transform.forward();
            let right = transform.right();

            // Project movement onto horizontal plane
            let forward = Vec3::new(forward.x, 0.0, forward.z).normalize();
            let right = Vec3::new(right.x, 0.0, right.z).normalize();

            let movement = (forward * -z + right * x).normalize() * PLAYER_MOVE_SPEED * time.delta_secs();
            
            // Only apply horizontal movement
            transform.translation += Vec3::new(movement.x, 0.0, movement.z);

            // Send position updates at 20Hz
            if time.elapsed_secs() - *last_sent > 0.05 && client.is_connected() {
                let input = ClientInput::Position(transform.translation);
                let message = bincode::serialize(&input).unwrap();
                client.send_message(ClientChannel::Input, message);
                *last_sent = time.elapsed_secs();
            }
        }
    }
}

pub fn change_fov(
    input: Res<ButtonInput<KeyCode>>,
    mut camera: Query<&mut Projection, With<WorldModelCamera>>,
) {
    if let Ok(mut projection) = camera.get_single_mut() {
        let Projection::Perspective(ref mut perspective) = projection.as_mut() else {
            return;
        };

        if input.pressed(KeyCode::ArrowUp) {
            perspective.fov -= 1.0_f32.to_radians();
            perspective.fov = perspective.fov.max(20.0_f32.to_radians());
        }
        if input.pressed(KeyCode::ArrowDown) {
            perspective.fov += 1.0_f32.to_radians();
            perspective.fov = perspective.fov.min(160.0_f32.to_radians());
        }
    }
}

pub fn player_input(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut player_input: ResMut<PlayerInput>,
    mut client: ResMut<RenetClient>,
) {
    // Update input state
    player_input.left = keyboard_input.pressed(KeyCode::KeyA);
    player_input.right = keyboard_input.pressed(KeyCode::KeyD);
    player_input.up = keyboard_input.pressed(KeyCode::KeyW);
    player_input.down = keyboard_input.pressed(KeyCode::KeyS);

    // Send if client is connected
    if client.is_connected() {
        let input = ClientInput::Movement(*player_input);
        let message = bincode::serialize(&input).unwrap();
        client.send_message(ClientChannel::Input, message);
    }
}

pub fn grab_mouse(
    mut windows: Query<&mut Window>,
    mouse: Res<ButtonInput<MouseButton>>,
    key: Res<ButtonInput<KeyCode>>,
    mut cursor_state: ResMut<CursorState>,
) {
    let mut window = windows.single_mut();

    if cursor_state.locked {
        // Force cursor to center
        window.cursor_options.grab_mode = bevy::window::CursorGrabMode::Locked;
        window.cursor_options.visible = false;

        // Get window center
        let half_width = window.resolution.width() / 2.0;
        let half_height = window.resolution.height() / 2.0;

        // Set cursor position to center
        window.set_cursor_position(Some(Vec2::new(half_width, half_height)));
    }

    if key.just_pressed(KeyCode::Escape) {
        cursor_state.locked = false;
        window.cursor_options.grab_mode = bevy::window::CursorGrabMode::None;
        window.cursor_options.visible = true;
    }

    if mouse.just_pressed(MouseButton::Left) && !cursor_state.locked {
        cursor_state.locked = true;
    }
}
