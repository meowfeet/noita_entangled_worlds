use super::{
    omni::{OmniPeerId, Reliability},
    world::WorldNetMessage,
};
use crate::{game_settings::GameSettings, player_cosmetics::PlayerPngDesc};
use bitcode::{Decode, Encode};

pub(crate) type Destination = crate::protocol::Destination<OmniPeerId>;

pub(crate) struct MessageRequest<T> {
    pub(crate) reliability: Reliability,
    pub(crate) dst: Destination,
    pub(crate) msg: T,
}

#[derive(Debug, Decode, Encode, Clone)]
pub(crate) enum NetMsg {
    Kick,
    PeerDisconnected {
        id: OmniPeerId,
    },
    StartGame {
        settings: GameSettings,
        seed: u64,
        world_num: u16,
        progress: Vec<String>,
    },
    ModRaw {
        data: Vec<u8>,
    },
    WorldMessage(WorldNetMessage),
    /// Appearance, is host, who to answer with our own appearance.
    PlayerColor(PlayerPngDesc, bool, Option<OmniPeerId>),
    RemoteMsg(crate::protocol::RemoteMessage),
    ForwardDesToProxy(crate::protocol::des::DesToProxy),
    ForwardProxyToDes(crate::protocol::des::ProxyToDes),
    NoitaDisconnected,
    Flags(String),
    RespondFlagNormal(String, bool),
    RespondFlagSlow(usize, bool),
    RespondFlagMoon(i32, i32, bool),
    RespondFlagStevari(i32, i32, OmniPeerId),
}

impl From<MessageRequest<WorldNetMessage>> for MessageRequest<NetMsg> {
    fn from(value: MessageRequest<WorldNetMessage>) -> Self {
        Self {
            msg: NetMsg::WorldMessage(value.msg),
            reliability: value.reliability,
            dst: value.dst,
        }
    }
}
