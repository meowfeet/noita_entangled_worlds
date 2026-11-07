use bitcode::{Decode, Encode};

/// Game settings. The host's (from `settings::game_settings`) are sent to everyone.
#[derive(Debug, Decode, Encode, Clone)]
pub struct GameSettings {
    pub game_mode: GameMode,
    pub health_per_player: u32,
    pub health_lost_on_revive: u32,
    pub global_hp_loss: bool,
    pub no_material_damage: bool,
    pub physics_damage: bool,
    pub local_health_alternate_dont_run: bool,
    pub revive_on_drop: bool,
    pub pvp_kill_steal: u32,
    pub dont_steal: bool,
    pub chest_on_win: bool,
    pub timed: bool,
    pub wait_for_time: bool,
    pub time_in_hm: u32,
    pub time_out_hm: u32,
    pub wait_on_players: bool,
    pub constant_seed: Option<u64>,
    pub duplicate: bool,
    pub nice_terraforming: bool,
    pub disable_kummitus: bool,
    pub give_host_sampo: bool,
    pub enemy_hp_mult: f32,
    pub spell_ban_list: String,
    pub max_players: u32,
    pub friendly_fire: bool,
    pub share_gold: bool,
    pub same_loadout: bool,
    pub home_on_players: bool,
    pub randomize_perks: bool,
    pub perk_ban_list: String,
    pub disabled_globals: String,
    pub debug_mode: bool,
}

#[derive(Debug, Decode, Encode, Clone, Copy, PartialEq, Eq)]
pub enum GameMode {
    SharedHealth,
    LocalHealth(LocalHealthMode),
}

#[derive(Debug, Decode, Encode, Clone, Copy, PartialEq, Eq)]
pub enum LocalHealthMode {
    Normal,
    Alternate,
    PermaDeath,
    PvP,
}
