use std::{
    collections::{HashMap, VecDeque},
    mem,
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

use dashmap::DashMap;
use steamworks::{
    CallbackHandle, LobbyChatUpdate, LobbyId, LobbyType, SteamError, SteamId,
    networking_sockets::{ListenSocket, NetPollGroup},
    networking_types::{
        ListenSocketEvent, NetworkingConnectionState, NetworkingIdentity, SendFlags,
    },
};
use tracing::{error, info, warn};

use crate::{
    lobby_code::{LobbyCode, LobbyKind},
    settings,
};

use super::omni::{OmniNetworkEvent, OmniPeerId, Reliability};

/// Stored in the lobby data; only proxies with the same version can join each other.
/// The "-cli" keeps official proxies out, their network format is different.
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"), "-cli");

enum SteamEvent {
    LobbyCreatedOrJoined(LobbyId),
    Failed(String),
    PeerConnectedToLobby(SteamId),
    PeerDisconnectedFromLobby(SteamId),
    PeerStateChanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerState {
    PendingConnection,
    CreatingMesh,
    Connected,
    /// Could not create or join the lobby.
    Failed(String),
}

pub struct InnerState {
    lobby_id: Option<LobbyId>,
    host_id: SteamId,
    remote_peers: Vec<SteamId>,
    state: PeerState,
}

enum ConnectionState {
    AwaitingIncoming,
    NetConnectionPending(steamworks::networking_sockets::NetConnection),
    NetConnection(steamworks::networking_sockets::NetConnection),
}
impl ConnectionState {
    fn switch_to_connected(&mut self) {
        let current = mem::replace(self, ConnectionState::AwaitingIncoming);
        if let ConnectionState::NetConnectionPending(conn) = current {
            *self = ConnectionState::NetConnection(conn);
        }
    }
    fn connection(&self) -> Option<&steamworks::networking_sockets::NetConnection> {
        if let ConnectionState::NetConnection(conn) = self {
            Some(conn)
        } else {
            None
        }
    }
}

/// How long a peer's backlog may stay non-empty before we give up on the peer.
const SEND_BACKLOG_TIMEOUT: Duration = Duration::from_secs(30);
const SEND_BACKLOG_REPORT_INTERVAL: Duration = Duration::from_secs(5);

/// Reliable messages Steam refused with `LimitExceeded`, held in order until it has room.
struct SendBacklog {
    messages: VecDeque<Vec<u8>>,
    bytes: usize,
    peak_messages: usize,
    peak_bytes: usize,
    /// Time of the first refusal.
    since: Instant,
    last_report: Instant,
    last_error: Option<SteamError>,
}

enum BacklogState {
    Drained,
    Waiting,
    Stuck,
}

impl SendBacklog {
    fn new(first: Vec<u8>, now: Instant) -> Self {
        let mut backlog = Self {
            messages: VecDeque::new(),
            bytes: 0,
            peak_messages: 0,
            peak_bytes: 0,
            since: now,
            last_report: now,
            last_error: Some(SteamError::LimitExceeded),
        };
        backlog.push(first);
        backlog
    }

    fn push(&mut self, msg: Vec<u8>) {
        self.bytes += msg.len();
        self.messages.push_back(msg);
        self.peak_messages = self.peak_messages.max(self.messages.len());
        self.peak_bytes = self.peak_bytes.max(self.bytes);
    }

    /// Sends oldest first, stopping at the first refusal. Only an empty backlog
    /// resets the timeout.
    fn drain(
        &mut self,
        now: Instant,
        mut send: impl FnMut(&[u8]) -> Result<(), SteamError>,
    ) -> BacklogState {
        while let Some(msg) = self.messages.front() {
            let len = msg.len();
            if let Err(err) = send(msg) {
                self.last_error = Some(err);
                break;
            }
            self.messages.pop_front();
            self.bytes -= len;
        }
        if self.messages.is_empty() {
            BacklogState::Drained
        } else if now.duration_since(self.since) >= SEND_BACKLOG_TIMEOUT {
            BacklogState::Stuck
        } else {
            BacklogState::Waiting
        }
    }

    fn report_due(&mut self, now: Instant) -> bool {
        if now.duration_since(self.last_report) >= SEND_BACKLOG_REPORT_INTERVAL {
            self.last_report = now;
            true
        } else {
            false
        }
    }
}

struct Connections {
    client: steamworks::Client,

    my_id: SteamId,
    listen_socket: ListenSocket,
    poll_group: Mutex<NetPollGroup>,

    peers: DashMap<SteamId, ConnectionState>,
    connected: Mutex<Vec<SteamId>>,
    /// Lock before `peers` when taking both.
    backlogs: Mutex<HashMap<SteamId, SendBacklog>>,
}

impl Connections {
    fn new(client: &steamworks::Client) -> Self {
        let my_id = client.user().steam_id();
        let networking_sockets = client.networking_sockets();
        let listen_socket = networking_sockets
            .create_listen_socket_p2p(0, None)
            .expect("handle to be valid"); // Unclear in what cases this can fail.
        let poll_group = networking_sockets.create_poll_group().into();
        Connections {
            client: client.clone(),

            my_id,
            listen_socket,
            poll_group,

            peers: Default::default(),
            connected: Default::default(),
            backlogs: Default::default(),
        }
    }
    fn flush(&self) {
        for i in &self.peers {
            if let Some(c) = i.connection()
                && let Err(err) = c.flush_messages()
            {
                warn!("Error while flushing a message: {err:?}")
            }
        }
    }

    fn poll_listener(&self) {
        while let Some(event) = self.listen_socket.try_receive_event() {
            match event {
                ListenSocketEvent::Connecting(event) => {
                    info!("Peer {:?} connecting", event.remote());
                    event
                        .accept()
                        .inspect_err(|e| warn!("Error when accepting connection: {}", e))
                        .ok();
                }
                ListenSocketEvent::Connected(event) => {
                    if let Some(mut connection) = event
                        .remote()
                        .steam_id()
                        .and_then(|x| self.peers.get_mut(&x))
                    {
                        info!("Peer {:?} got connected event", event.remote());
                        let taken_connection = event.take_connection();
                        taken_connection.set_poll_group(&self.poll_group.lock().unwrap());
                        *connection = ConnectionState::NetConnectionPending(taken_connection);
                    }
                }
                ListenSocketEvent::Disconnected(event) => {
                    info!(
                        "Peer {:?} disconnected, reason: {:?}",
                        event.remote(),
                        event.end_reason(),
                    )
                }
            }
        }
    }
    fn poll_status(&self) -> bool {
        let networking_sockets = self.client.networking_sockets();
        let mut all_connected = true;
        // Postpone reconnect so we don't get deadlocked.
        let mut pending_reconnect = Vec::new();
        for mut state in self.peers.iter_mut() {
            match state.value() {
                ConnectionState::AwaitingIncoming => {
                    all_connected = false;
                }
                ConnectionState::NetConnectionPending(connection) => {
                    let info = networking_sockets
                        .get_connection_info(connection)
                        .expect("handle to be valid");
                    match info.state().expect("assuming state is always valid") {
                        // Wait.
                        NetworkingConnectionState::Connecting
                        | NetworkingConnectionState::FindingRoute => {
                            all_connected = false;
                        }
                        // Switch to connected state.
                        NetworkingConnectionState::Connected => {
                            info!(
                                "Connection of peer {:?} swithed to connected state",
                                *state.key()
                            );
                            self.connected.lock().unwrap().push(*state.key());
                            state.value_mut().switch_to_connected();
                            all_connected = false;
                        }

                        NetworkingConnectionState::None
                        | NetworkingConnectionState::ClosedByPeer
                        | NetworkingConnectionState::ProblemDetectedLocally => {
                            info!(
                                "Some problem happened for peer {:?} while making connection. Will try to connect again.",
                                *state.key()
                            );
                            pending_reconnect.push(*state.key());
                            all_connected = false;
                        }
                    }
                }
                ConnectionState::NetConnection(connection) => {
                    let info = networking_sockets
                        .get_connection_info(connection)
                        .expect("handle to be valid");
                    match info.state().expect("assuming state is always valid") {
                        NetworkingConnectionState::Connecting
                        | NetworkingConnectionState::FindingRoute => {
                            // Not supposed to happen, as we're in NetConnectionPending state while connecting.
                            warn!("Not supposed to be in connecting state here.");
                            // Still recreate connection.
                            pending_reconnect.push(*state.key());
                            all_connected = false;
                        }
                        NetworkingConnectionState::Connected => {
                            // All is ok, no need to do anything.
                        }
                        NetworkingConnectionState::None
                        | NetworkingConnectionState::ClosedByPeer
                        | NetworkingConnectionState::ProblemDetectedLocally => {
                            // Something broke, we need to recreate the connection.
                            info!(
                                "Some problem happened for peer {:?} while connection was established. Will try to connect again.",
                                *state.key()
                            );
                            pending_reconnect.push(*state.key());
                            // TODO: signal that reconnect happened.
                            // Several things should be reset on it, as we certainly lost several messages during it.
                            all_connected = false;
                        }
                    }
                }
            }
        }
        for peer_id in pending_reconnect {
            self.connect(peer_id);
        }

        all_connected
    }

    /// Returns true if everyone is connected.
    fn poll(&self) -> bool {
        self.poll_listener();
        self.poll_status()
    }

    fn connect(&self, peer: SteamId) {
        let networking_sockets = self.client.networking_sockets();
        let peer_identity = NetworkingIdentity::new_steam_id(peer);
        if peer > self.my_id {
            info!("Awaiting incoming connection from {:?}", peer);
            self.peers.insert(peer, ConnectionState::AwaitingIncoming);
        } else {
            info!("Initiating connection to {:?}", peer);
            let connection = networking_sockets
                .connect_p2p(peer_identity, 0, None)
                .expect("handle to be valid");
            connection.set_poll_group(&self.poll_group.lock().unwrap());
            self.peers
                .insert(peer, ConnectionState::NetConnectionPending(connection));
        }
    }
    fn disconnect(&self, id: SteamId) {
        info!("Removing connection to peer {:?}", id);
        self.peers.remove(&id);
        self.backlogs.lock().unwrap().remove(&id);
    }

    fn recv(&self) -> Vec<steamworks::networking_types::NetworkingMessage> {
        self.poll_group.lock().unwrap().receive_messages(1024)
    }

    fn send_message(
        &self,
        peer: SteamId,
        reliability: Reliability,
        msg: &[u8],
    ) -> Result<(), SteamError> {
        if reliability == Reliability::Unreliable {
            return self.send_now(peer, SendFlags::UNRELIABLE, msg);
        }
        let mut backlogs = self.backlogs.lock().unwrap();
        // Don't overtake anything already waiting.
        if let Some(backlog) = backlogs.get_mut(&peer) {
            backlog.push(msg.to_vec());
            return Ok(());
        }
        match self.send_now(peer, SendFlags::RELIABLE, msg) {
            Err(SteamError::LimitExceeded) => {
                warn!(
                    "Steam's send buffer for peer {:?} is full; holding reliable messages to it until there is room",
                    peer
                );
                backlogs.insert(peer, SendBacklog::new(msg.to_vec(), Instant::now()));
                Ok(())
            }
            res => res,
        }
    }

    /// Retries every backlog. Returns the peers whose backlog timed out; those
    /// backlogs are dropped.
    fn drain_backlogs(&self) -> Vec<SteamId> {
        let now = Instant::now();
        let mut stuck = Vec::new();
        self.backlogs.lock().unwrap().retain(|&peer, backlog| {
            let entry = self.peers.get(&peer);
            // Not `send_now`: it reports success when the peer isn't connected yet.
            let connection = entry
                .as_ref()
                .and_then(|state| state.value().connection());
            let state = backlog.drain(now, |msg| match connection {
                Some(connection) => connection
                    .send_message(msg, SendFlags::RELIABLE)
                    .map(|_| ()),
                None => Err(SteamError::NoConnection),
            });
            let waited = now.duration_since(backlog.since).as_secs_f32();
            match state {
                BacklogState::Drained => {
                    warn!(
                        "Steam's send buffer for peer {:?} has room again after {:.1}s; the backlog peaked at {} reliable messages ({} bytes)",
                        peer, waited, backlog.peak_messages, backlog.peak_bytes
                    );
                    false
                }
                BacklogState::Waiting => {
                    if backlog.report_due(now) {
                        warn!(
                            "Reliable messages to peer {:?} still backed up after {:.1}s: {} messages ({} bytes) waiting, last refusal {:?}",
                            peer,
                            waited,
                            backlog.messages.len(),
                            backlog.bytes,
                            backlog.last_error
                        );
                    }
                    true
                }
                BacklogState::Stuck => {
                    error!(
                        "Giving up on reliable messages to peer {:?} after {:.1}s: dropping {} messages ({} bytes), last refusal {:?}",
                        peer,
                        waited,
                        backlog.messages.len(),
                        backlog.bytes,
                        backlog.last_error
                    );
                    stuck.push(peer);
                    false
                }
            }
        });
        stuck
    }

    fn send_now(&self, peer: SteamId, send_flags: SendFlags, msg: &[u8]) -> Result<(), SteamError> {
        if let Some(peer) = self.peers.get(&peer) {
            if let Some(connection) = peer.value().connection() {
                connection.send_message(msg, send_flags)?;
                Ok(())
            } else {
                // TODO: Not exactly the right thing to do, but this can only happen before we properly connected, so it's probably fine..?
                // Might result in a packet loss in case we ever go reconnecting.
                Ok(())
            }
        } else {
            Err(SteamError::InvalidSteamID)
        }
    }
}

pub struct SteamPeer {
    client: steamworks::Client,
    events: mpsc::Receiver<SteamEvent>,
    sender: mpsc::Sender<SteamEvent>,
    my_id: SteamId,
    is_host: bool,
    inner: Mutex<InnerState>,
    _cbs: Vec<CallbackHandle>,

    connections: Connections,
}

impl SteamPeer {
    pub fn flush(&self) {
        self.connections.flush()
    }

    pub fn new_host(lobby_type: LobbyType, client: steamworks::Client, max_players: u32) -> Self {
        let (sender, events) = mpsc::channel();

        let connections = Connections::new(&client);

        let matchmaking = client.matchmaking();
        {
            let sender = sender.clone();
            matchmaking.create_lobby(lobby_type, max_players, {
                let client = client.clone();
                move |lobby| {
                    let event = match lobby {
                        Ok(id) => {
                            client
                                .matchmaking()
                                .set_lobby_data(id, "ew_version", VERSION);
                            SteamEvent::LobbyCreatedOrJoined(id)
                        }
                        Err(err) => SteamEvent::Failed(format!("Could not create lobby: {err}")),
                    };
                    sender.send(event).ok();
                }
            });
        }

        Self::new(client, sender, events, connections, true)
    }

    pub fn new_connect(lobby: LobbyId, client: steamworks::Client) -> Self {
        let (sender, events) = mpsc::channel();
        let connections = Connections::new(&client);
        let matchmaking = client.matchmaking();
        {
            let sender = sender.clone();

            matchmaking.join_lobby(lobby, {
                let client = client.clone();
                move |lobby| {
                    let event = match lobby {
                        Ok(id) => match client.matchmaking().lobby_data(id, "ew_version").as_deref() {
                            Some(VERSION) => SteamEvent::LobbyCreatedOrJoined(id),
                            Some(remote) => SteamEvent::Failed(format!(
                                "Lobby was created by proxy with a different version: {remote}. You have {VERSION}."
                            )),
                            None => SteamEvent::Failed(
                                "Lobby does not have a version field. The lobby was created by an old proxy version."
                                    .to_string(),
                            ),
                        },
                        Err(_) => SteamEvent::Failed(
                            "Lobby does not exist. Make sure SPACEWARS in settings.rs matches the host's."
                                .to_string(),
                        ),
                    };

                    sender.send(event).ok();
                }
            });
        }

        Self::new(client, sender, events, connections, false)
    }

    fn new(
        client: steamworks::Client,
        sender: mpsc::Sender<SteamEvent>,
        events: mpsc::Receiver<SteamEvent>,
        connections: Connections,
        is_host: bool,
    ) -> Self {
        let my_id = client.user().steam_id();
        let _cbs = make_callbacks(&sender, &client);
        Self {
            my_id,
            client,

            events,
            sender,
            is_host,
            inner: Mutex::new(InnerState {
                lobby_id: None,
                host_id: my_id,
                remote_peers: Vec::new(),
                state: PeerState::PendingConnection,
            }),
            _cbs,
            connections,
        }
    }

    pub fn send(
        &self,
        peer: OmniPeerId,
        msg: &[u8],
        reliability: Reliability,
    ) -> Result<(), SteamError> {
        self.connections.send_message(peer.into(), reliability, msg)
    }

    pub fn broadcast(&self, msg: &[u8], reliability: Reliability) {
        let peers = self.inner.lock().unwrap().remote_peers.clone();
        for peer in peers {
            if let Err(err) = self.connections.send_message(peer, reliability, msg) {
                warn!(
                    "Couldn't send a packet to {:?}, st {:?}, err {}",
                    peer, reliability, err
                )
            };
        }
    }

    pub fn recv(&self) -> Vec<OmniNetworkEvent> {
        let all_connected = self.connections.poll();
        if all_connected && self.inner.lock().unwrap().state == PeerState::CreatingMesh {
            info!("Switched to `all connected` state");
            self.inner.lock().unwrap().state = PeerState::Connected;
        }
        let mut returned_events = Vec::new();
        for peer in self.connections.drain_backlogs() {
            returned_events.push(OmniNetworkEvent::SendBacklogStuck(peer.into()));
        }
        for event in self.events.try_iter() {
            match event {
                SteamEvent::LobbyCreatedOrJoined(id) => {
                    info!("Lobby ready");
                    self.inner.lock().unwrap().lobby_id = Some(id);
                    if self.is_host {
                        let kind = if settings::SPACEWARS {
                            LobbyKind::Gog
                        } else {
                            LobbyKind::Steam
                        };
                        println!("Lobby code: {}", LobbyCode { kind, code: id }.serialize());
                    } else {
                        let host_id = self.client.matchmaking().lobby_owner(id);
                        self.inner.lock().unwrap().host_id = host_id;
                        info!("Got host id: {:?}", host_id)
                    }
                    self.update_lobby_list();
                    info!("Switched to `creating mesh` state");
                    self.inner.lock().unwrap().state = PeerState::CreatingMesh;
                }
                SteamEvent::Failed(err) => {
                    self.inner.lock().unwrap().state = PeerState::Failed(err);
                }
                SteamEvent::PeerConnectedToLobby(id) => {
                    self.connections.connect(id);
                }
                SteamEvent::PeerDisconnectedFromLobby(id) => {
                    self.connections.disconnect(id);
                    returned_events.push(OmniNetworkEvent::PeerDisconnected(id.into()))
                }
                SteamEvent::PeerStateChanged => self.update_lobby_list(),
            }
        }

        let messages = self.connections.recv();
        for message in messages {
            let steam_id = message
                .identity_peer()
                .steam_id()
                .expect("only steam ids are supported");
            returned_events.push(OmniNetworkEvent::Message {
                src: steam_id.into(),
                data: message.data().to_vec(), // TODO eliminate clone here.
            })
        }
        let mut fully_connected = self.connections.connected.lock().unwrap();
        for steam_id in fully_connected.iter() {
            returned_events.push(OmniNetworkEvent::PeerConnected((*steam_id).into()))
        }
        fully_connected.clear();

        returned_events
    }

    fn update_lobby_list(&self) {
        info!("Updating peer list");
        let matchmaking = self.client.matchmaking();
        let lobby = self.inner.lock().unwrap().lobby_id;
        let mut peers = match lobby {
            Some(lobby_id) => matchmaking.lobby_members(lobby_id),
            None => Vec::new(),
        };
        peers.retain(|x| *x != self.my_id);
        peers.sort();
        let current_peers = &mut self.inner.lock().unwrap().remote_peers;

        // TODO: could be done more efficiently
        for peer in &peers {
            if !current_peers.contains(peer) {
                self.sender
                    .send(SteamEvent::PeerConnectedToLobby(*peer))
                    .ok();
            }
        }
        for peer in &mut *current_peers {
            if !peers.contains(peer) {
                self.sender
                    .send(SteamEvent::PeerDisconnectedFromLobby(*peer))
                    .ok();
            }
        }

        *current_peers = peers;
        info!("Current peer list: {:?}", current_peers);
    }

    pub fn my_id(&self) -> OmniPeerId {
        self.my_id.into()
    }

    pub fn host_id(&self) -> OmniPeerId {
        if self.is_host {
            self.my_id.into()
        } else {
            self.inner.lock().unwrap().host_id.into()
        }
    }

    pub fn lobby_id(&self) -> Option<LobbyId> {
        self.inner.lock().unwrap().lobby_id
    }

    pub fn state(&self) -> PeerState {
        self.inner.lock().unwrap().state.clone()
    }

    pub fn is_host(&self) -> bool {
        self.is_host
    }
}

fn make_callbacks(
    sender: &mpsc::Sender<SteamEvent>,
    client: &steamworks::Client,
) -> Vec<CallbackHandle> {
    {
        client
            .networking_messages()
            .session_request_callback(|req| {
                info!("Accepting connection with {:?}", req.remote());
                req.accept();
            });
    };
    let cb_ch = {
        let sender = sender.clone();
        client.register_callback(move |update: LobbyChatUpdate| {
            info!("User state changed {:?}", update);
            sender.send(SteamEvent::PeerStateChanged).ok();
        })
    };
    vec![cb_ch]
}

impl Drop for SteamPeer {
    fn drop(&mut self) {
        if let Some(lobby) = self.lobby_id() {
            self.client.matchmaking().leave_lobby(lobby);
        }
    }
}
