use std::f32::consts::PI;

use bevy::prelude::*;
use bevy_renet::renet::{ClientId, RenetServer};

use crate::{
    network::{ServerLobby, ServerMessages},
    player::Player,
    ServerChannel, Velocity,
};

#[derive(Debug, Component)]
pub struct Bot {
    auto_cast: Timer,
}

#[derive(Debug, Resource)]
pub struct BotId(pub u64);

fn spawn_bot(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut lobby: ResMut<ServerLobby>,
    mut server: ResMut<RenetServer>,
    mut bot_id: ResMut<BotId>,
    mut commands: Commands,
) {
    if keyboard_input.just_pressed(KeyCode::Space) {
        let client_id: ClientId = bot_id.0;
        bot_id.0 += 1;
        // Spawn new player
        let transform = Transform::from_xyz(
            (fastrand::f32() - 0.5) * 40.,
            0.51,
            (fastrand::f32() - 0.5) * 40.,
        );
        let player_entity = commands
            .spawn((
                Mesh3d(meshes.add(Mesh::from(Capsule3d::default()))),
                MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
                transform,
            ))
            .insert(Player { id: client_id })
            .insert(Bot {
                auto_cast: Timer::from_seconds(3.0, TimerMode::Repeating),
            })
            .id();

        lobby.players.insert(client_id, player_entity);

        let translation: [f32; 3] = transform.translation.into();
        let message = bincode::serialize(&ServerMessages::PlayerCreate {
            id: client_id,
            entity: player_entity,
            translation,
        })
        .unwrap();
        server.broadcast_message(ServerChannel::ServerMessages, message);
    }
}

fn bot_autocast(
    time: Res<Time>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut server: ResMut<RenetServer>,
    mut bots: Query<(&Transform, &mut Bot), With<Player>>,
    mut commands: Commands,
) {
    for (transform, mut bot) in &mut bots {
        bot.auto_cast.tick(time.delta());
        if !bot.auto_cast.just_finished() {
            continue;
        }

        for i in 0..8 {
            let direction = Vec2::from_angle(PI / 4. * i as f32);
            let direction = Vec3::new(direction.x, 0., direction.y).normalize();
            let translation: Vec3 = transform.translation + direction;

            let fireball_entity = spawn_fireball(
                &mut commands,
                &mut meshes,
                &mut materials,
                translation,
                direction,
            );
            let message = ServerMessages::SpawnProjectile {
                entity: fireball_entity,
                translation: translation.into(),
            };
            let message = bincode::serialize(&message).unwrap();
            server.broadcast_message(ServerChannel::ServerMessages, message);
        }
    }
}

#[derive(Debug, Component)]
pub struct Projectile {
    pub duration: Timer,
}

pub fn spawn_fireball(
    commands: &mut Commands,
    meshes: &mut ResMut<Assets<Mesh>>,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    translation: Vec3,
    mut direction: Vec3,
) -> Entity {
    if !direction.is_normalized() {
        direction = Vec3::X;
    }
    commands
        .spawn((
            Mesh3d(meshes.add(Sphere { radius: 0.1 })),
            MeshMaterial3d(materials.add(Color::srgb(1.0, 0.0, 0.0))),
            Transform::from_translation(translation),
        ))
        .insert(Velocity(direction * 10.))
        .insert(Projectile {
            duration: Timer::from_seconds(1.5, TimerMode::Once),
        })
        .id()
}
