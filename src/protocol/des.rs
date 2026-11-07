use std::num::NonZero;

use super::{GameEffectData, PeerId, SpawnOnce, WorldPos};
use bitcode::{Decode, Encode};

pub const REQUEST_AUTHORITY_RADIUS: i32 = 512;

/// 64 bit globally unique id. Assigned randomly, should only have 50% chance of collision with 2^32 entities at once.
#[derive(Debug, Encode, Decode, Clone, Copy, Hash, PartialEq, Eq)]
pub struct Gid(pub u64);

/// 32 bit locally unique id.
#[derive(Debug, Encode, Decode, Clone, Copy)]
pub struct Lid(pub u32);

#[derive(Debug, Encode, Decode, Clone)]
pub enum EntitySpawnInfo {
    Filename(String),
    Serialized { data: Vec<u8> },
}

#[derive(Debug, Encode, Decode, Clone)]
pub struct FullEntityData {
    pub gid: Gid,
    pub pos: WorldPos,
    pub data: EntitySpawnInfo,
    pub wand: Option<Vec<u8>>,
    pub hp: f32,
    pub max_hp: Option<f32>,
    pub drops_gold: bool,
    pub is_charmed: bool,
    pub counter: u8,
    pub phys: Vec<Option<PhysBodyInfo>>,
    pub synced_var: Vec<(String, String, i32, f32, bool)>,
}

#[derive(Debug, Encode, Decode, Clone)]
pub struct UpdatePosition {
    pub gid: Gid,
    pub pos: WorldPos,
    pub counter: u8,
    pub is_charmed: bool,
    pub hp: f32,
    pub max_hp: Option<f32>,
    pub phys: Vec<Option<PhysBodyInfo>>,
    pub synced_var: Vec<(String, String, i32, f32, bool)>,
}

#[derive(Debug, Encode, Decode, Clone)]
pub enum UpdateOrUpload {
    Update(UpdatePosition),
    Upload(FullEntityData),
}

#[derive(Debug, Encode, Decode, Clone)]
pub enum DesToProxy {
    DeleteEntity(Gid, Option<NonZero<isize>>),
    ReleaseAuthority(Gid),
    RequestAuthority { pos: WorldPos },
    UpdatePosition(UpdateOrUpload),
    UpdatePositions(Vec<UpdateOrUpload>),
    TransferAuthorityTo(Gid, PeerId),
    UpdateWand(Gid, Option<Vec<u8>>),
}

#[derive(Debug, Encode, Decode, Clone)]
pub enum ProxyToDes {
    /// Got authority over entity.
    GotAuthority(FullEntityData),
    GotAuthoritys(Vec<FullEntityData>),
    RemoveEntities(PeerId),
    DeleteEntity(NonZero<isize>),
}
#[derive(Debug, Encode, Decode, Clone)]
pub struct InterestRequest {
    pub pos: WorldPos,
}

#[derive(Debug, Encode, Decode, Clone, Copy)]
pub struct PhysBodyInfo {
    pub x: f32,
    pub y: f32,
    pub angle: f32,
    pub vx: f32,
    pub vy: f32,
    pub av: f32,
}

#[derive(Debug, Encode, Decode, Clone)]
pub enum EntityKind {
    /// Normal entity, replicated with a filename.
    Normal,
    /// Item entity, can be picked up.
    Item,
}

#[derive(Debug, Encode, Decode, Clone, Copy)]
pub enum Target {
    Peer(PeerId),
    Gid(Gid),
    None,
}

#[derive(Debug, Encode, Decode, Clone)]
pub struct EntityInfo {
    pub spawn_info: EntitySpawnInfo,
    pub kind: EntityKind,
    pub x: f32,
    pub y: f32,
    pub r: f32,
    pub vx: f32,
    pub vy: f32,
    pub hp: f32,
    /// The owner's max hp, so that a receiver converges on it instead of only ever raising its own.
    /// `None` means the owner has nothing to say, either because the entity has no damage model or
    /// because it died before its first collect, and a receiver leaves its own max hp alone.
    pub max_hp: Option<f32>,
    pub phys: Vec<Option<PhysBodyInfo>>,
    pub cost: i64,
    pub game_effects: Vec<GameEffectData>,
    pub current_stains: u64,
    pub animations: Vec<u16>,
    pub wand: Option<(Option<Gid>, Vec<u8>, isize)>,
    pub wand_rotation: f32,
    pub is_global: bool,
    pub drops_gold: bool,
    pub limbs: Vec<(f32, f32)>,
    pub ai_state: i32,
    pub laser: Target,
    pub ai_rotation: f32,
    pub facing_direction: (bool, bool),
    /// For kolmi, runestones etc.
    pub is_enabled: bool,
    /// For mom orbs, the dragon's death script etc.
    pub counter: u8,
    pub synced_var: Vec<(String, String, i32, f32, bool)>,
}

#[derive(Debug, Encode, Decode, Clone)]
pub struct EntityInit {
    pub info: EntityInfo,
    pub lid: Lid,
    pub gid: Gid,
}

#[derive(Debug, Encode, Decode, Clone)]
pub enum EntityUpdate {
    /// Sets the gid that following EntityUpdates will act on.
    CurrentEntity(Lid),
    SetPosition(f32, f32),
    SetRotation(f32),
    SetVelocity(f32, f32),
    SetHp(f32),
    SetPhysInfo(Vec<Option<PhysBodyInfo>>),
    RemoveEntity(Lid),
    LocalizeEntity(Lid, PeerId),
    KillEntity {
        lid: Lid,
        wait_on_kill: bool,
        responsible_peer: Option<PeerId>,
    },
    SetCost(i64),
    SetLaser(Target),
    SetStains(u64),
    SetFacingDirection((bool, bool)),
    SetAnimations(Vec<u16>),
    SetGameEffects(Vec<GameEffectData>),
    SetWand(Option<(Option<Gid>, Vec<u8>, isize)>),
    SetWandRotation(f32),
    SetAiRotation(f32),
    SetAiState(i32),
    SetLimbs(Vec<(f32, f32)>),
    SetIsEnabled(bool),
    SetCounter(u8),
    SetSyncedVar(Vec<(String, String, i32, f32, bool)>),
    // New variants go at the end. bitcode encodes the variant by its index in
    // this list, so inserting one anywhere else renumbers every variant after
    // it and a peer on an older build silently applies the wrong update.
    SetMaxHp(Option<f32>),
}

#[derive(Debug, Encode, Decode, Clone)]
pub enum RemoteDes {
    /// Should be sent when client opens the game, to reset in case of restart.
    Reset,
    InterestRequest(InterestRequest),
    EntityUpdate(Vec<EntityUpdate>),
    EntityInit(Vec<EntityInit>),
    ExitedInterest,
    Projectiles(Vec<ProjectileFired>),
    RequestGrab(Lid),
    CameraPos(WorldPos),
    DeadEntities(Vec<(WorldPos, SpawnOnce)>),
    SpawnOnce(WorldPos, SpawnOnce),
    ChestOpen(Gid, i32, i32, String, f32, f32),
    ChestOpenRequest(Gid, i32, i32, String, f32, f32),
}

#[derive(Debug, Encode, Decode, Clone)]
pub struct ProjectileFired {
    pub shooter_lid: Lid,
    pub position: (f32, f32),
    pub target: (f32, f32),
    pub serialized: Vec<u8>,
    pub vel: Option<(f32, f32)>,
}
