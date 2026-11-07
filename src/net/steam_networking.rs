use std::{
    collections::{HashMap, VecDeque},
    mem,
    sync::mpsc,
    time::{Duration, Instant},
};

use steamworks::{
    CallbackHandle, LobbyChatUpdate, LobbyId, LobbyType, SteamError, SteamId,
    networking_sockets::{ListenSocket, NetConnection, NetPollGroup},
    networking_types::{
        ListenSocketEvent, NetworkingConnectionState, NetworkingIdentity, NetworkingMessage,
        SendFlags,
    },
};
use tracing::{error, info, warn};

use crate::protocol::PeerId;

/// Stored in the lobby data; only proxies with the same version can join each other.
/// The "-cli" keeps official proxies out, their network format is different.
const VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"), "-cli");

/// All lobby ids share these high bits, so lobby codes leave them out.
const LOBBY_CODE_BASE: u64 = 0x0186000000000000;

/// The code friends join with: the lobby id in hex, without the shared high bits.
fn lobby_code(id: LobbyId) -> String {
    format!("{:x}", id.raw().wrapping_sub(LOBBY_CODE_BASE))
}

pub fn parse_lobby_code(code: &str) -> Option<LobbyId> {
    let code = u64::from_str_radix(code.trim(), 16).ok()?;
    Some(LobbyId::from_raw(code.wrapping_add(LOBBY_CODE_BASE)))
}

impl From<SteamId> for PeerId {
    fn from(value: SteamId) -> Self {
        Self(value.raw())
    }
}

impl From<PeerId> for SteamId {
    fn from(value: PeerId) -> Self {
        Self::from_raw(value.0)
    }
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

pub enum NetworkEvent {
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    Message {
        src: PeerId,
        data: Vec<u8>,
    },
    /// Reliable messages to this peer have been backlogged for too long.
    SendBacklogStuck(PeerId),
}

enum SteamEvent {
    LobbyCreatedOrJoined(LobbyId),
    Failed(String),
    PeerConnectedToLobby(SteamId),
    PeerDisconnectedFromLobby(SteamId),
    PeerStateChanged,
}

#[derive(PartialEq)]
enum PeerState {
    PendingConnection,
    CreatingMesh,
    Connected,
    /// Could not create or join the lobby.
    Failed(String),
}

enum ConnectionState {
    AwaitingIncoming,
    NetConnectionPending(NetConnection),
    NetConnection(NetConnection),
}
impl ConnectionState {
    fn switch_to_connected(&mut self) {
        let current = mem::replace(self, ConnectionState::AwaitingIncoming);
        if let ConnectionState::NetConnectionPending(conn) = current {
            *self = ConnectionState::NetConnection(conn);
        }
    }
    fn connection(&self) -> Option<&NetConnection> {
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
    poll_group: NetPollGroup,

    peers: HashMap<SteamId, ConnectionState>,
    /// Peers whose connection just got established, reported by `SteamPeer::recv`.
    connected: Vec<SteamId>,
    backlogs: HashMap<SteamId, SendBacklog>,
}

impl Connections {
    fn new(client: &steamworks::Client) -> Self {
        let my_id = client.user().steam_id();
        let networking_sockets = client.networking_sockets();
        let listen_socket = networking_sockets
            .create_listen_socket_p2p(0, None)
            .expect("handle to be valid"); // Unclear in what cases this can fail.
        let poll_group = networking_sockets.create_poll_group();
        Connections {
            client: client.clone(),

            my_id,
            listen_socket,
            poll_group,

            peers: HashMap::new(),
            connected: Vec::new(),
            backlogs: HashMap::new(),
        }
    }

    fn flush(&self) {
        for state in self.peers.values() {
            if let Some(c) = state.connection()
                && let Err(err) = c.flush_messages()
            {
                warn!("Error while flushing a message: {err:?}")
            }
        }
    }

    fn poll_listener(&mut self) {
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
                    if let Some(connection) = event
                        .remote()
                        .steam_id()
                        .and_then(|x| self.peers.get_mut(&x))
                    {
                        info!("Peer {:?} got connected event", event.remote());
                        let taken_connection = event.take_connection();
                        taken_connection.set_poll_group(&self.poll_group);
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

    /// Returns true if everyone is connected.
    fn poll_status(&mut self) -> bool {
        let networking_sockets = self.client.networking_sockets();
        let mut all_connected = true;
        // Postponed, as reconnecting changes `peers` while we iterate it.
        let mut pending_reconnect = Vec::new();
        for (&peer, state) in self.peers.iter_mut() {
            match state {
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
                            info!("Connection of peer {:?} swithed to connected state", peer);
                            self.connected.push(peer);
                            state.switch_to_connected();
                            all_connected = false;
                        }

                        NetworkingConnectionState::None
                        | NetworkingConnectionState::ClosedByPeer
                        | NetworkingConnectionState::ProblemDetectedLocally => {
                            info!(
                                "Some problem happened for peer {:?} while making connection. Will try to connect again.",
                                peer
                            );
                            pending_reconnect.push(peer);
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
                            pending_reconnect.push(peer);
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
                                peer
                            );
                            pending_reconnect.push(peer);
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
    fn poll(&mut self) -> bool {
        self.poll_listener();
        self.poll_status()
    }

    fn connect(&mut self, peer: SteamId) {
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
            connection.set_poll_group(&self.poll_group);
            self.peers
                .insert(peer, ConnectionState::NetConnectionPending(connection));
        }
    }

    fn disconnect(&mut self, id: SteamId) {
        info!("Removing connection to peer {:?}", id);
        self.peers.remove(&id);
        self.backlogs.remove(&id);
    }

    fn recv(&mut self) -> Vec<NetworkingMessage> {
        self.poll_group.receive_messages(1024)
    }

    fn send_message(
        &mut self,
        peer: SteamId,
        reliability: Reliability,
        msg: &[u8],
    ) -> Result<(), SteamError> {
        if reliability == Reliability::Unreliable {
            return self.send_now(peer, SendFlags::UNRELIABLE, msg);
        }
        // Don't overtake anything already waiting.
        if let Some(backlog) = self.backlogs.get_mut(&peer) {
            backlog.push(msg.to_vec());
            return Ok(());
        }
        match self.send_now(peer, SendFlags::RELIABLE, msg) {
            Err(SteamError::LimitExceeded) => {
                warn!(
                    "Steam's send buffer for peer {:?} is full; holding reliable messages to it until there is room",
                    peer
                );
                self.backlogs
                    .insert(peer, SendBacklog::new(msg.to_vec(), Instant::now()));
                Ok(())
            }
            res => res,
        }
    }

    /// Retries every backlog. Returns the peers whose backlog timed out; those
    /// backlogs are dropped.
    fn drain_backlogs(&mut self) -> Vec<SteamId> {
        let now = Instant::now();
        let mut stuck = Vec::new();
        let peers = &self.peers;
        self.backlogs.retain(|&peer, backlog| {
            // Not `send_now`: it reports success when the peer isn't connected yet.
            let connection = peers.get(&peer).and_then(|state| state.connection());
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
            if let Some(connection) = peer.connection() {
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
    lobby_id: Option<LobbyId>,
    host_id: SteamId,
    remote_peers: Vec<SteamId>,
    state: PeerState,
    _cbs: Vec<CallbackHandle>,

    connections: Connections,
}

impl SteamPeer {
    pub fn new_host(lobby_type: LobbyType, client: steamworks::Client, max_players: u32) -> Self {
        let (sender, events) = mpsc::channel();
        {
            let sender = sender.clone();
            let lobby_client = client.clone();
            client
                .matchmaking()
                .create_lobby(lobby_type, max_players, move |lobby| {
                    let event = match lobby {
                        Ok(id) => {
                            lobby_client
                                .matchmaking()
                                .set_lobby_data(id, "ew_version", VERSION);
                            SteamEvent::LobbyCreatedOrJoined(id)
                        }
                        Err(err) => SteamEvent::Failed(format!("Could not create lobby: {err}")),
                    };
                    sender.send(event).ok();
                });
        }
        Self::new(client, sender, events, true)
    }

    pub fn new_connect(lobby: LobbyId, client: steamworks::Client) -> Self {
        let (sender, events) = mpsc::channel();
        {
            let sender = sender.clone();
            let lobby_client = client.clone();
            client.matchmaking().join_lobby(lobby, move |lobby| {
                let event = match lobby {
                    Ok(id) => match lobby_client
                        .matchmaking()
                        .lobby_data(id, "ew_version")
                        .as_deref()
                    {
                        Some(VERSION) => SteamEvent::LobbyCreatedOrJoined(id),
                        Some(remote) => SteamEvent::Failed(format!(
                            "Lobby was created by proxy with a different version: {remote}. You have {VERSION}."
                        )),
                        None => SteamEvent::Failed(
                            "Lobby does not have a version field. The lobby was created by an old proxy version."
                                .to_string(),
                        ),
                    },
                    Err(_) => SteamEvent::Failed("Lobby does not exist.".to_string()),
                };
                sender.send(event).ok();
            });
        }
        Self::new(client, sender, events, false)
    }

    fn new(
        client: steamworks::Client,
        sender: mpsc::Sender<SteamEvent>,
        events: mpsc::Receiver<SteamEvent>,
        is_host: bool,
    ) -> Self {
        let my_id = client.user().steam_id();
        let _cbs = make_callbacks(&sender, &client);
        Self {
            connections: Connections::new(&client),
            my_id,
            client,
            events,
            sender,
            is_host,
            lobby_id: None,
            // Replaced by the lobby owner once a client joined.
            host_id: my_id,
            remote_peers: Vec::new(),
            state: PeerState::PendingConnection,
            _cbs,
        }
    }

    pub fn flush(&self) {
        self.connections.flush()
    }

    pub fn send(
        &mut self,
        peer: PeerId,
        msg: &[u8],
        reliability: Reliability,
    ) -> Result<(), SteamError> {
        self.connections.send_message(peer.into(), reliability, msg)
    }

    pub fn broadcast(&mut self, msg: &[u8], reliability: Reliability) {
        for &peer in &self.remote_peers {
            if let Err(err) = self.connections.send_message(peer, reliability, msg) {
                warn!(
                    "Couldn't send a packet to {:?}, st {:?}, err {}",
                    peer, reliability, err
                )
            };
        }
    }

    pub fn recv(&mut self) -> Vec<NetworkEvent> {
        let all_connected = self.connections.poll();
        if all_connected && self.state == PeerState::CreatingMesh {
            info!("Switched to `all connected` state");
            self.state = PeerState::Connected;
        }
        let mut returned_events = Vec::new();
        for peer in self.connections.drain_backlogs() {
            returned_events.push(NetworkEvent::SendBacklogStuck(peer.into()));
        }
        while let Ok(event) = self.events.try_recv() {
            match event {
                SteamEvent::LobbyCreatedOrJoined(id) => {
                    info!("Lobby ready");
                    self.lobby_id = Some(id);
                    if self.is_host {
                        println!("Lobby code: {}", lobby_code(id));
                    } else {
                        self.host_id = self.client.matchmaking().lobby_owner(id);
                        info!("Got host id: {:?}", self.host_id)
                    }
                    self.update_lobby_list();
                    info!("Switched to `creating mesh` state");
                    self.state = PeerState::CreatingMesh;
                }
                SteamEvent::Failed(err) => self.state = PeerState::Failed(err),
                SteamEvent::PeerConnectedToLobby(id) => self.connections.connect(id),
                SteamEvent::PeerDisconnectedFromLobby(id) => {
                    self.connections.disconnect(id);
                    returned_events.push(NetworkEvent::PeerDisconnected(id.into()))
                }
                SteamEvent::PeerStateChanged => self.update_lobby_list(),
            }
        }

        for message in self.connections.recv() {
            let steam_id = message
                .identity_peer()
                .steam_id()
                .expect("only steam ids are supported");
            returned_events.push(NetworkEvent::Message {
                src: steam_id.into(),
                data: message.data().to_vec(),
            })
        }
        for steam_id in self.connections.connected.drain(..) {
            returned_events.push(NetworkEvent::PeerConnected(steam_id.into()))
        }

        returned_events
    }

    fn update_lobby_list(&mut self) {
        info!("Updating peer list");
        let mut peers = match self.lobby_id {
            Some(lobby_id) => self.client.matchmaking().lobby_members(lobby_id),
            None => Vec::new(),
        };
        peers.retain(|x| *x != self.my_id);
        peers.sort();

        for peer in &peers {
            if !self.remote_peers.contains(peer) {
                self.sender
                    .send(SteamEvent::PeerConnectedToLobby(*peer))
                    .ok();
            }
        }
        for peer in &self.remote_peers {
            if !peers.contains(peer) {
                self.sender
                    .send(SteamEvent::PeerDisconnectedFromLobby(*peer))
                    .ok();
            }
        }

        self.remote_peers = peers;
        info!("Current peer list: {:?}", self.remote_peers);
    }

    pub fn my_id(&self) -> PeerId {
        self.my_id.into()
    }

    pub fn host_id(&self) -> PeerId {
        self.host_id.into()
    }

    pub fn is_host(&self) -> bool {
        self.is_host
    }

    /// Why creating or joining the lobby failed, if it did.
    pub fn error(&self) -> Option<&str> {
        match &self.state {
            PeerState::Failed(err) => Some(err),
            _ => None,
        }
    }
}

fn make_callbacks(
    sender: &mpsc::Sender<SteamEvent>,
    client: &steamworks::Client,
) -> Vec<CallbackHandle> {
    client
        .networking_messages()
        .session_request_callback(|req| {
            info!("Accepting connection with {:?}", req.remote());
            req.accept();
        });
    let sender = sender.clone();
    let cb_ch = client.register_callback(move |update: LobbyChatUpdate| {
        info!("User state changed {:?}", update);
        sender.send(SteamEvent::PeerStateChanged).ok();
    });
    vec![cb_ch]
}

impl Drop for SteamPeer {
    fn drop(&mut self) {
        if let Some(lobby) = self.lobby_id {
            self.client.matchmaking().leave_lobby(lobby);
        }
    }
}
