//! Messages exchanged with the mod's ewext.dll inside Noita.
//! They must stay identical to what the installed mod (v1.6.8) was built with.

use bitcode::{Decode, Encode};

pub mod basic_types;
pub mod des;
pub mod message_socket;

pub use basic_types::*;

#[derive(Debug, Encode, Decode, Clone)]
pub enum SpawnOnce {
    Enemy(String, bool, Option<PeerId>),
    Chest(String, f32, f32),
    BrokenWand,
}

#[derive(Debug, Encode, Decode, Clone)]
pub enum RemoteMessage {
    RemoteDes(des::RemoteDes),
}

#[derive(Encode, Decode)]
pub enum NoitaInbound {
    RawMessage(Vec<u8>),
    Ready {
        my_peer_id: PeerId,
    },
    ProxyToDes(des::ProxyToDes),
    RemoteMessage {
        source: PeerId,
        message: RemoteMessage,
    },
}

#[derive(Encode, Decode)]
pub enum NoitaOutbound {
    Raw(Vec<u8>),
    DesToProxy(des::DesToProxy),
    RemoteMessage {
        reliable: bool,
        destination: Destination,
        message: RemoteMessage,
    },
}
#[derive(Debug, Clone, Copy, Encode, Decode)]
pub enum GameEffectEnum {
    None,
    Electrocution,
    Frozen,
    OnFire,
    Poison,
    Berserk,
    Charm,
    Polymorph,
    PolymorphRandom,
    Blindness,
    Telepathy,
    Teleportation,
    Regeneration,
    Levitation,
    MovementSlower,
    Farts,
    Drunk,
    BreathUnderwater,
    Radioactive,
    Wet,
    Oiled,
    Bloody,
    Slimy,
    CriticalHitBoost,
    Confusion,
    MeleeCounter,
    WormAttractor,
    WormDetractor,
    FoodPoisoning,
    FriendThundermage,
    FriendFiremage,
    InternalFire,
    InternalIce,
    Jarate,
    Knockback,
    KnockbackImmunity,
    MovementSlower2X,
    MovementFaster,
    StainsDropFaster,
    SavingGrace,
    DamageMultiplier,
    HealingBlood,
    Respawn,
    ProtectionFire,
    ProtectionRadioactivity,
    ProtectionExplosion,
    ProtectionMelee,
    ProtectionElectricity,
    Teleportitis,
    StainlessArmour,
    GlobalGore,
    EditWandsEverywhere,
    ExplodingCorpseShots,
    ExplodingCorpse,
    ExtraMoney,
    ExtraMoneyTrickKill,
    HoverBoost,
    ProjectileHoming,
    AbilityActionsMaterialized,
    NoDamageFlash,
    NoSlimeSlowdown,
    MovementFaster2X,
    NoWandEditing,
    LowHpDamageBoost,
    FasterLevitation,
    StunProtectionElectricity,
    StunProtectionFreeze,
    IronStomach,
    ProtectionAll,
    Invisibility,
    RemoveFogOfWar,
    ManaRegeneration,
    ProtectionDuringTeleport,
    ProtectionPolymorph,
    ProtectionFreeze,
    FrozenSpeedUp,
    UnstableTeleportation,
    PolymorphUnstable,
    Custom,
    AllergyRadioactive,
    RainbowFarts,
    Weakness,
    ProtectionFoodPoisoning,
    NoHeal,
    ProtectionEdges,
    ProtectionProjectile,
    PolymorphCessation,
    _Last,
}

#[derive(Debug, Encode, Decode, Clone)]
pub enum GameEffectData {
    Normal(GameEffectEnum),
    Custom(String),
    Projectile((String, Vec<u8>)),
}
