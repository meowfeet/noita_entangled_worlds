use super::world::WorldNetMessage;
use crate::game_settings::GameSettings;
use crate::protocol::{PeerId, RemoteMessage, des};
use bitcode::{Decode, Encode};

/// Messages between proxies.
#[derive(Debug, Decode, Encode, Clone)]
pub enum NetMsg {
    Kick,
    PeerDisconnected {
        id: PeerId,
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
    RemoteMsg(RemoteMessage),
    ForwardDesToProxy(des::DesToProxy),
    ForwardProxyToDes(des::ProxyToDes),
    NoitaDisconnected,
    Flags(String),
    RespondFlagNormal(String, bool),
    RespondFlagSlow(usize, bool),
    RespondFlagMoon(i32, i32, bool),
    RespondFlagStevari(i32, i32, PeerId),
}
