use des::DesManager;
use messages::NetMsg;
use proxy_opt::ProxyOpt;
use rustc_hash::FxHashSet;
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    fmt::Display,
    mem, thread,
    time::{Duration, Instant},
};
use steam_networking::{NetworkEvent, Reliability, SteamPeer};
use tracing::{error, info, warn};
use world::{CellType, ExplosionData, Material, NoitaWorldUpdate, WorldManager};

use crate::game_settings::{GameMode, GameSettings, LocalHealthMode};
use crate::player_cosmetics::{MINA_COLOR, MINA_COLOR_ALT, create_player_png, reset_tmp_dir};
use crate::protocol::des::ProxyToDes;
use crate::protocol::message_socket::MessageSocket;
use crate::protocol::{Destination, NoitaInbound, NoitaOutbound, PeerId};
use crate::save_state::{RunInfo, SaveState, SaveStateEntry};
use crate::settings;

mod des;
mod messages;
mod proxy_opt;
pub mod steam_networking;
mod world;

/// Set to false (by Ctrl+C) to stop the proxy, which saves the run on the way out.
pub static RUNNING: AtomicBool = AtomicBool::new(true);

/// Where Noita connects to the proxy.
const NOITA_ADDR: &str = "127.0.0.1:21251";

/// A text message for the mod's proxy handler: `key value`.
fn proxy_msg(key: &'static str, value: impl Display) -> NoitaInbound {
    let mut buf = vec![2];
    write!(buf, "{key} {value}").unwrap();
    NoitaInbound::RawMessage(buf)
}

/// A binary message for the mod's proxy handler.
fn proxy_bin_msg(key: u8, data: &[u8]) -> NoitaInbound {
    let mut buf = vec![3, key];
    buf.extend(data);
    NoitaInbound::RawMessage(buf)
}

/// A message from another peer's mod.
fn mod_msg(peer: PeerId, data: &[u8]) -> NoitaInbound {
    let mut buf = vec![1];
    buf.extend_from_slice(&peer.0.to_le_bytes());
    buf.extend_from_slice(data);
    NoitaInbound::RawMessage(buf)
}

impl SaveStateEntry for FxHashSet<String> {
    const FILENAME: &'static str = "flags";
}

enum FlagType {
    Normal(String),
    Slow(String, usize),
    Moon(String, i32, i32, bool),
    Stevari(String, i32, i32),
}

/// Cap on the declared uncompressed size of a peer-supplied message.
///
/// `lz4_flex::decompress_size_prepended` reads the u32 length prefix and
/// allocates it *before* decompressing, so ~8 hostile bytes could request a
/// 4 GiB allocation. Read the prefix ourselves and reject implausible sizes.
const MAX_DECOMPRESSED_LEN: usize = 64 * 1024 * 1024;

fn decompress_bounded(data: &[u8]) -> Option<Vec<u8>> {
    let (size, rest) = lz4_flex::block::uncompressed_size(data).ok()?;
    if size > MAX_DECOMPRESSED_LEN {
        warn!("Rejecting message declaring {size} decompressed bytes");
        return None;
    }
    lz4_flex::decompress(rest, size).ok()
}

fn get_flags(mut flags: String) -> Option<FlagType> {
    if flags.is_empty() {
        return None;
    }
    let tag = flags.remove(0);
    // `flags` is the payload of NetMsg::Flags, i.e. it comes from a peer. These
    // used to index c[1]..c[3] directly, so a short message panicked the net
    // thread; match on the split slice instead.
    let c: Vec<&str> = flags.split(' ').collect();
    match tag {
        '0' => Some(FlagType::Normal(flags)),
        '1' => match c.as_slice() {
            [slow, name, ..] => Some(FlagType::Slow(
                (*name).to_string(),
                slow.parse().unwrap_or_default(),
            )),
            _ => None,
        },
        '2' => match c.as_slice() {
            [x, y, flag, name, ..] => Some(FlagType::Moon(
                (*name).to_string(),
                x.parse().unwrap_or_default(),
                y.parse().unwrap_or_default(),
                *flag == "1",
            )),
            _ => None,
        },
        '3' => match c.as_slice() {
            [x, y, name, ..] => Some(FlagType::Stevari(
                (*name).to_string(),
                x.parse().unwrap_or_default(),
                y.parse().unwrap_or_default(),
            )),
            _ => None,
        },
        _ => None,
    }
}

/// Names of the progress flags in Noita's save, sent to the mod.
fn get_progress(noita_save: Option<&Path>) -> Vec<String> {
    let Some(flags) = noita_save.map(|save| save.join("save00/persistent/flags")) else {
        return Vec::new();
    };
    fs::read_dir(flags)
        .map(|dir| {
            dir.filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default()
}

pub struct NetManagerInit {
    pub my_nickname: String,
    pub save_state: SaveState,
    /// `<noita>/mods/quant.ew`
    pub mod_path: PathBuf,
    /// Noita's save folder, if it was found.
    pub noita_save: Option<PathBuf>,
}

pub struct NetManager {
    peer: SteamPeer,
    init: NetManagerInit,
    settings: GameSettings,
    seed: u64,
    world_num: u16,
    progress: Vec<String>,
    /// Whether Noita may connect yet: from the start for the host, after the host's settings arrive otherwise.
    accept_local: bool,
    /// Leave the lobby (the host left, or we got kicked).
    back_out: bool,
    /// Messages to ourselves.
    loopback: VecDeque<NetMsg>,
    noita: Option<MessageSocket>,
    world: WorldManager,
    des: DesManager,
    explosion_data: Vec<ExplosionData>,
    had_a_disconnect: bool,
    flags: FxHashSet<String>,
}

impl NetManager {
    pub fn new(peer: SteamPeer, init: NetManagerInit) -> Self {
        let is_host = peer.is_host();
        let settings = settings::game_settings();
        let (seed, progress) = if is_host {
            let seed = settings
                .constant_seed
                .or_else(|| init.save_state.load::<RunInfo>().map(|info| info.seed))
                .unwrap_or_else(rand::random);
            info!("Seed: {seed}");
            (seed, get_progress(init.noita_save.as_deref()))
        } else {
            (0, Vec::new())
        };
        Self {
            world: WorldManager::new(is_host, peer.my_id(), &init.save_state),
            des: DesManager::new(&init.save_state),
            flags: init.save_state.load().unwrap_or_default(),
            peer,
            init,
            settings,
            seed,
            world_num: 0,
            progress,
            accept_local: is_host,
            back_out: false,
            loopback: VecDeque::new(),
            noita: None,
            explosion_data: Vec::new(),
            had_a_disconnect: false,
        }
    }

    fn write_noita(&mut self, data: &NoitaInbound) {
        if let Some(noita) = &mut self.noita
            && let Err(err) = noita.write(data)
        {
            error!("Lost the connection to Noita while sending: {}", err);
            self.noita = None;
            self.had_a_disconnect = true;
        };
    }

    fn write_option(&mut self, key: &str, value: impl ProxyOpt) {
        let mut buf = vec![2];
        value.write_opt(&mut buf, key);
        self.write_noita(&NoitaInbound::RawMessage(buf));
    }

    fn send(&mut self, peer: PeerId, msg: &NetMsg, reliability: Reliability) {
        if peer == self.peer.my_id() {
            self.loopback.push_back(msg.clone());
        } else {
            let encoded = lz4_flex::compress_prepend_size(&bitcode::encode(msg));
            if let Err(err) = self.peer.send(peer, &encoded, reliability) {
                warn!(
                    "Error while sending message of len {}: {}",
                    encoded.len(),
                    err
                )
            }
        }
    }

    fn broadcast(&mut self, msg: &NetMsg, reliability: Reliability) {
        let encoded = lz4_flex::compress_prepend_size(&bitcode::encode(msg));
        self.peer.broadcast(&encoded, reliability);
    }

    fn send_to(&mut self, dst: Destination, msg: &NetMsg, reliability: Reliability) {
        match dst {
            Destination::Peers(peers) => {
                for peer in peers {
                    self.send(peer, msg, reliability);
                }
            }
            Destination::Peer(peer) => self.send(peer, msg, reliability),
            Destination::Host => self.send(self.peer.host_id(), msg, reliability),
            Destination::Broadcast => self.broadcast(msg, reliability),
        }
    }

    fn is_host(&self) -> bool {
        self.peer.is_host()
    }

    fn start_game_msg(&self) -> NetMsg {
        NetMsg::StartGame {
            settings: self.settings.clone(),
            seed: self.seed,
            world_num: self.world_num,
            progress: self.progress.clone(),
        }
    }

    /// Runs until Ctrl+C or until we leave the lobby.
    pub fn run(&mut self) -> io::Result<()> {
        reset_tmp_dir(&self.init.mod_path)?;
        create_player_png(self.peer.my_id(), &self.init.mod_path, self.is_host());

        let local_server = TcpListener::bind(NOITA_ADDR)
            .map_err(|e| io::Error::new(e.kind(), format!("Error binding to {NOITA_ADDR}: {e}")))?;
        local_server.set_nonblocking(true)?;
        info!("Listening for noita connection on {NOITA_ADDR}");
        info!("Is host: {}", self.is_host());

        let mut last_iter = Instant::now();
        while RUNNING.load(Ordering::Relaxed) && !self.back_out {
            if let Some(err) = self.peer.error() {
                return Err(io::Error::other(err.to_string()));
            }
            if self.noita.is_none() && self.accept_local {
                thread::sleep(Duration::from_millis(10));
                if let Ok((stream, addr)) = local_server.accept() {
                    info!("New stream incoming from {}", addr);
                    stream.set_nonblocking(false).ok();
                    self.noita = MessageSocket::new(stream)
                        .inspect_err(|e| error!("Could not set up the connection to Noita: {e}"))
                        .ok();
                    if self.noita.is_some() {
                        self.on_noita_connected();
                    }
                }
            }
            if let Some(noita) = &mut self.noita
                && let Err(err) = noita.flush()
            {
                warn!("Could not flush the connection to Noita: {err}");
            }
            for net_event in self.peer.recv() {
                self.handle_network_event(net_event);
            }
            while let Some(net_msg) = self.loopback.pop_front() {
                self.handle_net_msg(self.peer.my_id(), net_msg);
            }
            // Handle all available messages from Noita.
            while let Some(noita) = &self.noita {
                match noita.try_read() {
                    Ok(Some(msg)) => self.handle_mod_message(msg),
                    Ok(None) => break,
                    Err(err) => {
                        warn!("Game closed (Lost connection to noita instance: {})", err);
                        self.had_a_disconnect = true;
                        self.noita = None;
                    }
                }
            }
            for (dst, msg) in self.world.get_emitted_msgs() {
                self.send_to(dst, &NetMsg::WorldMessage(msg), Reliability::Reliable);
            }
            self.world.update();

            for update in self.world.get_noita_updates() {
                self.write_noita(&proxy_bin_msg(0, &update));
            }

            if self.had_a_disconnect {
                self.broadcast(&NetMsg::NoitaDisconnected, Reliability::Reliable);
                if self.is_host() {
                    self.des.noita_disconnected(self.peer.my_id());
                }
                self.had_a_disconnect = false;
            }

            for (dest, msg) in self.des.pending_messages() {
                self.send(dest, &NetMsg::ForwardProxyToDes(msg), Reliability::Reliable);
            }

            // Don't do excessive busy-waiting;
            let min_update_time = Duration::from_millis(8);
            let elapsed = last_iter.elapsed();
            if elapsed < min_update_time {
                thread::sleep(min_update_time - elapsed);
            }
            last_iter = Instant::now();
        }
        Ok(())
    }

    fn kick(&mut self, peer: PeerId) {
        info!("player kicked: {}", peer);
        self.write_noita(&proxy_msg("leave", peer.as_hex()));
        self.world.handle_peer_left(peer);
        self.send(peer, &NetMsg::Kick, Reliability::Reliable);
        self.broadcast(
            &NetMsg::PeerDisconnected { id: peer },
            Reliability::Reliable,
        );
    }

    fn handle_network_event(&mut self, net_event: NetworkEvent) {
        match net_event {
            NetworkEvent::PeerConnected(id) => {
                info!("Peer connected {id}");
                if self.is_host() {
                    info!("Sending start game message");
                    self.send(id, &self.start_game_msg(), Reliability::Reliable);
                }
                if id != self.peer.my_id() {
                    create_player_png(id, &self.init.mod_path, id == self.peer.host_id());
                }
                self.write_noita(&proxy_msg("join", id.as_hex()));
            }
            NetworkEvent::PeerDisconnected(id) => {
                self.write_noita(&proxy_msg("leave", id.as_hex()));
                self.world.handle_peer_left(id);
                self.des.noita_disconnected(id);
                self.write_noita(&NoitaInbound::ProxyToDes(ProxyToDes::RemoveEntities(id)));
                if id == self.peer.host_id() {
                    warn!("The host left the lobby");
                    self.back_out = true
                }
            }
            NetworkEvent::SendBacklogStuck(id) => {
                if self.is_host() {
                    error!(
                        "Kicking {id}: our reliable messages to them have been backed up for too long"
                    );
                    self.kick(id);
                } else {
                    error!(
                        "Leaving the lobby: our reliable messages to {id} have been backed up for too long"
                    );
                    self.back_out = true;
                }
            }
            NetworkEvent::Message { src, data } => {
                let Some(net_msg) = decompress_bounded(&data)
                    .and_then(|decomp| bitcode::decode::<NetMsg>(&decomp).ok())
                else {
                    return;
                };
                self.handle_net_msg(src, net_msg);
            }
        }
    }

    fn handle_net_msg(&mut self, src: PeerId, net_msg: NetMsg) {
        match net_msg {
            NetMsg::PeerDisconnected { id } => {
                info!("player kicked: {}", id);
                self.write_noita(&proxy_msg("leave", id.as_hex()));
                self.world.handle_peer_left(id);
            }
            NetMsg::StartGame {
                settings,
                seed,
                world_num,
                progress,
            } => {
                self.settings = settings;
                self.seed = seed;
                self.world_num = world_num;
                self.progress = progress;
                info!("Settings updated");
                self.accept_local = true;
                self.world.reset();
                self.des.reset();
                self.flags.clear();
            }
            NetMsg::ModRaw { data } => self.write_noita(&mod_msg(src, &data)),
            NetMsg::WorldMessage(msg) => self.world.handle_msg(src, msg),
            NetMsg::Kick => {
                warn!("Kicked from the lobby");
                self.back_out = true
            }
            NetMsg::RemoteMsg(message) => self.write_noita(&NoitaInbound::RemoteMessage {
                source: src,
                message,
            }),
            NetMsg::ForwardDesToProxy(des_to_proxy) => self.des.handle_noita_msg(src, des_to_proxy),
            NetMsg::ForwardProxyToDes(proxy_to_des) => {
                self.write_noita(&NoitaInbound::ProxyToDes(proxy_to_des));
            }
            NetMsg::NoitaDisconnected => {
                self.des.noita_disconnected(src);
                self.write_noita(&NoitaInbound::ProxyToDes(ProxyToDes::RemoveEntities(src)));
                self.write_noita(&proxy_msg("dc", src.as_hex()));
            }
            NetMsg::Flags(flags) => {
                let duplicate = self.settings.duplicate;
                if let Some(flag) = get_flags(flags) {
                    match flag {
                        FlagType::Normal(flag) => {
                            let new = duplicate || self.flags.insert(flag.clone());
                            self.send(
                                src,
                                &NetMsg::RespondFlagNormal(flag, new),
                                Reliability::Reliable,
                            )
                        }
                        FlagType::Slow(flag, ent) => {
                            let new = duplicate || self.flags.insert(flag);
                            self.send(
                                src,
                                &NetMsg::RespondFlagSlow(ent, new),
                                Reliability::Reliable,
                            )
                        }
                        FlagType::Moon(flag, x, y, b) => {
                            let new = duplicate || self.flags.insert(flag);
                            if new {
                                self.send(
                                    src,
                                    &NetMsg::RespondFlagMoon(x, y, b),
                                    Reliability::Reliable,
                                )
                            }
                        }
                        FlagType::Stevari(flag, x, y) => {
                            let new = duplicate || self.flags.insert(flag);
                            if new {
                                self.broadcast(
                                    &NetMsg::RespondFlagStevari(x, y, src),
                                    Reliability::Reliable,
                                );
                                self.send(
                                    self.peer.my_id(),
                                    &NetMsg::RespondFlagStevari(x, y, src),
                                    Reliability::Reliable,
                                )
                            }
                        }
                    }
                }
            }
            NetMsg::RespondFlagNormal(flag, new) => {
                self.write_noita(&proxy_msg("normal_flag", format!("{flag} {new}")));
            }
            NetMsg::RespondFlagSlow(ent, new) => {
                self.write_noita(&proxy_msg("slow_flag", format!("{ent} {new}")));
            }
            NetMsg::RespondFlagMoon(x, y, b) => {
                self.write_noita(&proxy_msg(
                    "moon_flag",
                    format!("{x} {y}{}", if b { "1" } else { "0" }),
                ));
            }
            NetMsg::RespondFlagStevari(x, y, id) => {
                let mine = id == self.peer.my_id();
                self.write_noita(&proxy_msg(
                    "stevari_flag",
                    format!("{x} {y}{}", if mine { "1" } else { "0" }),
                ));
            }
        }
    }

    fn on_noita_connected(&mut self) {
        self.init.save_state.mark_game_started();
        info!("New stream connected");

        let s = self.settings.clone();
        self.write_noita(&proxy_msg("seed", self.seed));
        let my_id = self.peer.my_id();
        self.write_noita(&proxy_msg("peer_id", my_id.as_hex()));
        self.write_noita(&proxy_msg("host_id", self.peer.host_id().as_hex()));
        let name = self.init.my_nickname.clone();
        info!("Chosen nickname: {}", name);
        self.write_option("name", name.as_str());
        self.write_option("world_num", self.world_num as u32);
        self.write_option(
            "friendly_fire",
            s.friendly_fire || s.game_mode == GameMode::LocalHealth(LocalHealthMode::PvP),
        );
        self.write_option("share_gold", s.share_gold);
        self.write_option("same_loadout", s.same_loadout);
        self.write_option("home_on_players", s.home_on_players);
        self.write_option("debug", s.debug_mode);
        self.write_option("randomize_perks", s.randomize_perks);
        self.write_option("enemy_hp_scale", s.enemy_hp_mult);
        self.write_option("game_mode", s.game_mode);
        match s.game_mode {
            GameMode::SharedHealth => {
                self.write_option("health_per_player", s.health_per_player);
            }
            GameMode::LocalHealth(LocalHealthMode::Normal) => {
                self.write_option("global_hp_loss", s.global_hp_loss);
                self.write_option("physics_damage", s.physics_damage);
                self.write_option("no_material_damage", s.no_material_damage);
                self.write_option("health_lost_on_revive", s.health_lost_on_revive);
            }
            GameMode::LocalHealth(LocalHealthMode::PermaDeath) => {
                self.write_option("perma_death", true)
            }
            GameMode::LocalHealth(LocalHealthMode::Alternate) => {
                self.write_option("no_notplayer", true);
                self.write_option("global_hp_loss", s.global_hp_loss);
                self.write_option("health_lost_on_revive", s.health_lost_on_revive);
                self.write_option(
                    "local_health_alternate_dont_run",
                    s.local_health_alternate_dont_run,
                );
                self.write_option("revive_on_drop", s.revive_on_drop);
            }
            GameMode::LocalHealth(LocalHealthMode::PvP) => {
                self.write_option("pvp", true);
                self.write_option(
                    "pvp_kill_steal",
                    if s.share_gold { 0 } else { s.pvp_kill_steal },
                );
                self.write_option("dont_steal", s.dont_steal);
                self.write_option("chest_on_win", s.chest_on_win);
                self.write_option("timed", s.timed);
                if s.timed {
                    self.write_option("wait_for_time", s.wait_for_time);
                    self.write_option("time_in", s.time_in_hm);
                    self.write_option("time_out", s.time_out_hm);
                } else {
                    self.write_option("wait_on_players", s.wait_on_players);
                }
            }
        }
        self.write_option("perk_ban_list", s.perk_ban_list.as_str());
        self.write_option("spell_ban_list", s.spell_ban_list.as_str());
        self.write_option("disabled_globals", s.disabled_globals.as_str());
        self.write_option("disable_kummitus", s.disable_kummitus);
        self.write_option("host_sampo", s.give_host_sampo);
        self.world.nice_terraforming = s.nice_terraforming;
        self.write_option("mina_color", MINA_COLOR);
        self.write_option("mina_color_alt", MINA_COLOR_ALT);

        let progress = self.progress.join(",");
        self.write_option("progress", progress.as_str());

        self.write_noita(&NoitaInbound::Ready { my_peer_id: my_id });
        info!("Settings sent")
    }

    fn handle_mod_message(&mut self, msg: NoitaOutbound) {
        match msg {
            NoitaOutbound::Raw(raw_msg) => {
                match raw_msg[0] & 0b11 {
                    // Message to proxy
                    1 => {
                        self.handle_message_to_proxy(&raw_msg[1..]);
                    }
                    // Broadcast
                    2 => {
                        let msg_to_send = NetMsg::ModRaw {
                            data: raw_msg[1..].to_owned(),
                        };
                        let reliable = raw_msg[0] & 4 > 0;
                        self.broadcast(&msg_to_send, Reliability::from_reliability_bool(reliable));
                    }
                    // Binary message to proxy
                    3 => self.handle_bin_message_to_proxy(&raw_msg[1..]),
                    0 => {
                        let flags = String::from_utf8_lossy(&raw_msg[1..]).into();
                        self.send(
                            self.peer.host_id(),
                            &NetMsg::Flags(flags),
                            Reliability::Reliable,
                        )
                    }
                    msg_variant => {
                        error!("Unknown msg variant from mod: {}", msg_variant)
                    }
                }
            }
            NoitaOutbound::DesToProxy(des_to_proxy) => {
                if self.is_host() {
                    self.des.handle_noita_msg(self.peer.my_id(), des_to_proxy)
                } else {
                    self.send(
                        self.peer.host_id(),
                        &NetMsg::ForwardDesToProxy(des_to_proxy),
                        Reliability::Reliable,
                    );
                }
            }
            NoitaOutbound::RemoteMessage {
                reliable,
                destination,
                message,
            } => self.send_to(
                destination,
                &NetMsg::RemoteMsg(message),
                Reliability::from_reliability_bool(reliable),
            ),
        }
    }

    fn handle_message_to_proxy(&mut self, msg: &[u8]) {
        let msg = String::from_utf8_lossy(msg);
        let mut msg = msg.split_ascii_whitespace();
        let key = msg.next();
        match key {
            Some("game_over") => {
                if self.is_host() {
                    info!("Game over, resending game settings");
                    self.end_run()
                }
            }
            // Camera/player position, only used for voice chat and the map, which aren't here.
            Some("cam_pos") => {}
            Some("reset_world") => {
                self.world.reset();
                self.des.reset();
                self.flags.clear();
            }
            Some("material_list") => {
                self.world.materials.clear();
                // The last value of each entry is the material's map color, which isn't used.
                while let (
                    Some(i),
                    Some(durability),
                    Some(hardness),
                    Some(cell_type),
                    Some(liquid_sand),
                    Some(liquid_static),
                    Some(_),
                ) = (
                    msg.next().and_then(|s| s.parse().ok()),
                    msg.next().and_then(|s| s.parse().ok()),
                    msg.next().and_then(|s| s.parse().ok()),
                    msg.next(),
                    msg.next().map(|s| s == "1"),
                    msg.next().map(|s| s == "1"),
                    msg.next()
                        .and_then(|s| u32::from_str_radix(&s.to_lowercase(), 16).ok()),
                ) {
                    self.world.materials.insert(
                        i,
                        Material {
                            durability,
                            hardness,
                            cell_type: CellType::new(cell_type, liquid_static, liquid_sand),
                        },
                    );
                }
                let c = msg.count();
                if c != 0 {
                    error!("bad materials data {}", c);
                }
            }
            Some("cut_through_world") => {
                let x: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let y_min: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let y_max: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let radius: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let (Some(x), Some(y_min), Some(y_max), Some(radius)) = (x, y_min, y_max, radius)
                else {
                    error!("Missing arguments in cut_through_world message");
                    return;
                };

                self.world.cut_through_world(x, y_min, y_max, radius);
            }
            Some("cut_through_world_line") => {
                let x: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let lx: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let y: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let ly: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let (Some(x), Some(y), Some(lx), Some(ly)) = (x, y, lx, ly) else {
                    error!("Missing arguments in cut_through_world_line message");
                    return;
                };
                let r: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let chance: Option<u64> = msg.next().and_then(|s| s.parse().ok());
                self.world.cut_through_world_line(
                    x,
                    y,
                    lx,
                    ly,
                    r.unwrap_or(12),
                    chance.unwrap_or(100).min(100) as u8,
                );
            }
            Some("cut_through_world_circle") => {
                let x: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let y: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let r: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let mat: Option<u16> = msg.next().and_then(|s| s.parse().ok());
                let chance: u64 = msg
                    .next()
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(100);
                let (Some(x), Some(y), Some(r)) = (x, y, r) else {
                    error!("Missing arguments in cut_through_world_circle message");
                    return;
                };
                self.world
                    .cut_through_world_circle(x, y, r, mat, chance.min(100) as u8);
            }
            Some("cut_through_world_explosion") => {
                let x: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let y: Option<i32> = msg.next().and_then(|s| s.parse().ok());
                let r: Option<u64> = msg.next().and_then(|s| s.parse().ok());
                let d: Option<u32> = msg.next().and_then(|s| s.parse().ok());
                let ray: Option<u64> = msg.next().and_then(|s| s.parse().ok());
                let hole: Option<bool> = msg.next().and_then(|s| s.parse().ok());
                let liquid: Option<bool> = msg.next().and_then(|s| s.parse().ok());
                let mat: Option<u16> = msg.next().and_then(|s| s.parse().ok());
                let prob: Option<u64> = msg.next().and_then(|s| s.parse().ok());
                let (
                    Some(x),
                    Some(y),
                    Some(r),
                    Some(d),
                    Some(ray),
                    Some(hole),
                    Some(liquid),
                    Some(mat),
                    Some(prob),
                ) = (x, y, r, d, ray, hole, liquid, mat, prob)
                else {
                    error!("Missing arguments in cut_through_world_expl message");
                    return;
                };
                self.explosion_data.push(ExplosionData::new(
                    x,
                    y,
                    r,
                    d,
                    ray,
                    hole,
                    liquid,
                    mat,
                    prob.min(100) as u8,
                ));
            }
            Some("flush_exp") => {
                self.world
                    .cut_through_world_explosion(mem::take(&mut self.explosion_data));
            }
            Some("flush") => self.peer.flush(),
            key => {
                error!("Unknown msg from mod: {:?}", key)
            }
        }
    }

    fn handle_bin_message_to_proxy(&mut self, msg: &[u8]) {
        let key = msg[0];
        let data = &msg[1..];
        match key {
            // world frame
            0 => {
                let update = NoitaWorldUpdate::load(data);
                self.world.add_update(update);
            }
            // world end
            1 => {
                let pos = data[1..]
                    .split(|b| *b == b':')
                    .map(|s| String::from_utf8_lossy(s).parse::<i32>().unwrap_or(0))
                    .collect::<Vec<i32>>();
                self.world.add_end(data[0], &pos);
            }
            key => {
                error!("Unknown bin msg from mod: {:?}", key)
            }
        }
    }

    fn end_run(&mut self) {
        self.init.save_state.reset();
        self.seed = self.settings.constant_seed.unwrap_or_else(rand::random);
        info!("New seed: {}", self.seed);
        self.progress = get_progress(self.init.noita_save.as_deref());
        self.world_num = self.world_num.wrapping_add(1);
        self.world.reset();
        self.des.reset();
        self.flags.clear();
        self.broadcast(&self.start_game_msg(), Reliability::Reliable);
    }
}

impl Drop for NetManager {
    /// Saves the run, so the host can continue it after a restart.
    fn drop(&mut self) {
        if self.is_host() {
            let save_state = &self.init.save_state;
            save_state.save(&RunInfo { seed: self.seed });
            save_state.save(&self.flags);
            save_state.save(&self.world.chunk_storage);
            save_state.save(&self.des.entity_storage);
        }
    }
}
