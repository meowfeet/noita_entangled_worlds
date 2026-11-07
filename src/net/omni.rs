use bitcode::{Decode, Encode};
use std::fmt::Display;
use steamworks::SteamId;

/// A player's id, the raw value of their `SteamId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Decode, Encode)]
pub struct OmniPeerId(pub u64);

impl From<crate::protocol::PeerId> for OmniPeerId {
    fn from(value: crate::protocol::PeerId) -> Self {
        OmniPeerId(value.0)
    }
}

impl From<OmniPeerId> for crate::protocol::PeerId {
    fn from(value: OmniPeerId) -> Self {
        crate::protocol::PeerId(value.0)
    }
}

impl From<SteamId> for OmniPeerId {
    fn from(value: SteamId) -> Self {
        Self(value.raw())
    }
}

impl From<OmniPeerId> for SteamId {
    fn from(value: OmniPeerId) -> Self {
        Self::from_raw(value.0)
    }
}

impl Display for OmniPeerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl OmniPeerId {
    pub(crate) fn as_hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

pub enum OmniNetworkEvent {
    PeerConnected(OmniPeerId),
    PeerDisconnected(OmniPeerId),
    Message {
        src: OmniPeerId,
        data: Vec<u8>,
    },
    /// Reliable messages to this peer have been backlogged for too long.
    SendBacklogStuck(OmniPeerId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reliability {
    /// Message will be delivered at most once.
    Unreliable,
    /// Message will be resent until its arrival is confirmed.
    Reliable,
}

impl Reliability {
    pub fn from_reliability_bool(reliable: bool) -> Reliability {
        if reliable {
            Reliability::Reliable
        } else {
            Reliability::Unreliable
        }
    }
}
