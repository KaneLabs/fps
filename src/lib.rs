use std::{f32::consts::PI, time::Duration};

use bevy::prelude::*;
use bevy_renet::renet::{ChannelConfig, ClientId, ConnectionConfig, SendType};
use network::{ClientChannel, ServerChannel};
use serde::{Deserialize, Serialize};

pub mod bot;
pub mod network;
pub mod player;
pub mod world;

#[cfg(feature = "netcode")]
pub const PRIVATE_KEY: &[u8; bevy_renet::netcode::NETCODE_KEY_BYTES] =
    b"an example very very secret key."; // 32-bytes
#[cfg(feature = "netcode")]
pub const PROTOCOL_ID: u64 = 7;

pub use bevy::prelude::{Mesh3d, MeshMaterial3d};


#[derive(Debug, Default, Component)]
pub struct Velocity(pub Vec3);


// /// set up a simple 3D scene
// pub fn setup_level(
//     mut commands: Commands,
//     mut meshes: ResMut<Assets<Mesh>>,
//     mut materials: ResMut<Assets<StandardMaterial>>,
// ) {
//     // plane
//     commands.spawn((
//         Mesh3d(meshes.add(Mesh::from(Cuboid::new(40., 1., 40.)))),
//         MeshMaterial3d(materials.add(Color::srgb(0.3, 0.5, 0.3))),
//         Transform::from_xyz(0.0, -1.0, 0.0),
//     ));
//     // light
//     commands.spawn((
//         DirectionalLight {
//             shadows_enabled: true,
//             ..default()
//         },
//         Transform {
//             translation: Vec3::new(0.0, 2.0, 0.0),
//             rotation: Quat::from_rotation_x(-PI / 4.),
//             ..default()
//         },
//     ));
// }
