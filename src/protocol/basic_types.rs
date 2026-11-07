use bitcode::{Decode, Encode};
use std::fmt;

#[derive(Debug, Encode, Decode, Clone, Copy)]
pub struct WorldPos {
    pub x: i32,
    pub y: i32,
}

impl WorldPos {
    pub fn as_array(&self) -> [i64; 2] {
        [self.x as i64, self.y as i64]
    }
}

/// A player's id, the raw value of their SteamId.
#[derive(Debug, Encode, Decode, Hash, PartialEq, Eq, Clone, Copy)]
pub struct PeerId(pub u64);

impl PeerId {
    pub fn as_hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Encode, Decode, Debug, PartialEq, Eq)]
pub enum Destination {
    Peers(Vec<PeerId>),
    Peer(PeerId),
    Host,
    Broadcast,
}
