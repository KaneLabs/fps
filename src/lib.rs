pub mod bot;
pub mod network;
pub mod player;
pub mod world;

#[cfg(feature = "netcode")]
pub const PRIVATE_KEY: &[u8; bevy_renet::netcode::NETCODE_KEY_BYTES] =
    b"an example very very secret key."; // 32-bytes

#[cfg(feature = "netcode")]
pub const PROTOCOL_ID: u64 = 7;
