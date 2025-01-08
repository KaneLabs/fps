use crate::{
    bot::Velocity,
    player::{Player, PlayerInput},
    world::DEFAULT_RENDER_LAYER,
};
use bevy::prelude::*;
use bevy::render::view::RenderLayers;
use bevy_rapier3d::prelude::*;
use bevy_renet::renet::ClientId;

#[derive(Bundle)]
pub struct PlayerPhysicsBundle {
    pub rigid_body: RigidBody,
    pub collider: Collider,
    pub velocity: Velocity,
    pub locked_axes: LockedAxes,
    pub friction: Friction,
    pub gravity: GravityScale,
}

impl Default for PlayerPhysicsBundle {
    fn default() -> Self {
        Self {
            rigid_body: RigidBody::Dynamic,
            collider: Collider::capsule(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.5, 0.0), 0.5),
            velocity: Velocity::default(),
            locked_axes: LockedAxes::ROTATION_LOCKED_X | LockedAxes::ROTATION_LOCKED_Z,
            friction: Friction::coefficient(1.0),
            gravity: GravityScale(2.0),
        }
    }
}

#[derive(Bundle)]
pub struct WorldObjectBundle {
    pub rigid_body: RigidBody,
    pub collider: Collider,
    pub friction: Friction,
}

impl WorldObjectBundle {
    pub fn floor() -> Self {
        Self {
            rigid_body: RigidBody::Fixed,
            collider: Collider::cuboid(10.0, 1.0, 10.0),
            friction: Friction::coefficient(1.0),
        }
    }

    pub fn wall() -> Self {
        Self {
            rigid_body: RigidBody::Fixed,
            collider: Collider::cuboid(0.25, 2.0, 5.0),
            friction: Friction::coefficient(1.0),
        }
    }
}

#[derive(Bundle)]
pub struct PlayerBundle {
    // Identity
    pub player: Player,
    pub name: Name,

    // Visual
    pub mesh: Mesh3d,
    pub material: MeshMaterial3d<StandardMaterial>,
    pub transform: Transform,
    pub render_layer: RenderLayers,

    // Physics
    pub rigid_body: RigidBody,
    pub collider: Collider,
    pub velocity: Velocity,
    pub locked_axes: LockedAxes,
    pub friction: Friction,
    pub gravity: GravityScale,

    // Game state
    pub input: PlayerInput,
}

impl PlayerBundle {
    pub fn new(
        id: ClientId,
        transform: Transform,
        meshes: &mut ResMut<Assets<Mesh>>,
        materials: &mut ResMut<Assets<StandardMaterial>>,
    ) -> Self {
        Self {
            // Identity
            player: Player { id },
            name: Name::new(format!("Player_{}", id)),

            // Visual
            mesh: Mesh3d(meshes.add(Mesh::from(Capsule3d::default()))),
            material: MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
            transform,
            render_layer: RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),

            // Physics
            rigid_body: RigidBody::Dynamic,
            collider: Collider::capsule(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.5, 0.0), 0.5),
            velocity: Velocity::default(),
            locked_axes: LockedAxes::ROTATION_LOCKED_X | LockedAxes::ROTATION_LOCKED_Z,
            friction: Friction::coefficient(1.0),
            gravity: GravityScale(1.0),

            // Game state
            input: PlayerInput::default(),
        }
    }
}
