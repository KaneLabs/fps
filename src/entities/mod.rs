use crate::{
    network::ControlledPlayer, player::{Player, PlayerInput}, world::DEFAULT_RENDER_LAYER
};
use bevy::prelude::*;
use bevy::render::view::RenderLayers;
use bevy_rapier3d::prelude::*;
use bevy_renet::renet::ClientId;

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
            friction: Friction {
                coefficient: 0.0,
                combine_rule: CoefficientCombineRule::Min,
            },
        }
    }

    pub fn wall() -> Self {
        Self {
            rigid_body: RigidBody::Fixed,
            collider: Collider::cuboid(0.25, 2.0, 5.0),
            friction: Friction {
                coefficient: 0.0,
                combine_rule: CoefficientCombineRule::Min,
            },
        }
    }
}

#[derive(Bundle)]
pub struct PlayerBundle {
    // Identity
    pub player: Player,
    pub name: Name,

    // Transform & Visual
    pub transform: Transform,
    pub mesh: Mesh3d,
    pub material: MeshMaterial3d<StandardMaterial>,
    pub render_layer: RenderLayers,

    // Physics
    pub rigid_body: RigidBody,
    pub collider: Collider,
    pub character_controller: KinematicCharacterController,

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
            player: Player { id },
            name: Name::new(format!("Player_{}", id)),
            transform,
            mesh: Mesh3d(meshes.add(Mesh::from(Capsule3d::default()))),
            material: MeshMaterial3d(materials.add(Color::srgb(0.8, 0.7, 0.6))),
            render_layer: RenderLayers::from_layers(&[DEFAULT_RENDER_LAYER]),
            rigid_body: RigidBody::KinematicPositionBased,
            collider: Collider::capsule(Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, 1.5, 0.0), 0.5),
            character_controller: KinematicCharacterController {
                max_slope_climb_angle: 0.0,
                min_slope_slide_angle: 0.0,
                offset: CharacterLength::Absolute(0.01),
                apply_impulse_to_dynamic_bodies: true,
                slide: true,
                autostep: None,
                up: Vec3::Y,
                translation: None,
                ..default()
            },
            input: PlayerInput::default(),
        }
    }
}

#[derive(Bundle)]
pub struct PictureFrameBundle {
    pub mesh: Mesh3d,
    pub material: MeshMaterial3d<StandardMaterial>,
    pub transform: Transform,
}

impl PictureFrameBundle {
    pub fn new(
        meshes: &mut ResMut<Assets<Mesh>>,
        materials: &mut ResMut<Assets<StandardMaterial>>,
        asset_server: &Res<AssetServer>,
        position: Vec3,
        size: Vec2, // width and height
    ) -> Self {
        let texture = asset_server.load("ryan-gun.png");

        let frame_depth = 0.01;
        let frame_mesh = meshes.add(Cuboid::new(size.x, size.y, frame_depth));

        let material = materials.add(StandardMaterial {
            base_color_texture: Some(texture),
            ..default()
        });

        Self {
            mesh: Mesh3d(frame_mesh),
            material: MeshMaterial3d(material),
            transform: Transform::from_translation(position)
                .with_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2))
                .with_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2))
                .with_rotation(Quat::from_rotation_z(std::f32::consts::PI)),
        }
    }
}
