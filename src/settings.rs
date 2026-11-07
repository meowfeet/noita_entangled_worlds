//! Every option of the proxy, set to the upstream defaults. Change a value and rebuild.

use std::{env, path::PathBuf};

use crate::game_settings::{GameMode, GameSettings, LocalHealthMode};
use crate::player_cosmetics::PlayerColor;

/// Your name in game. `None` uses your Steam name.
pub const NICKNAME: Option<&str> = None;

/// Path to noita.exe. `None` finds it through Steam.
pub const NOITA_EXE: Option<&str> = None;

/// Use Steam networking even if you don't own Noita on Steam (e.g. the GOG version).
/// Everyone in the lobby needs the same value.
pub const SPACEWARS: bool = false;

/// Steam lobby type when hosting: public, else friends only, else private (lobby code only).
pub const PUBLIC_LOBBY: bool = false;
pub const ALLOW_FRIENDS: bool = true;

/// Where the host keeps the current run so it can be continued after a restart.
pub fn save_state_path() -> PathBuf {
    PathBuf::from(env::var_os("APPDATA").expect("APPDATA is set"))
        .join(r"quant\entangledworlds\data\save_state")
}

/// Player colors (RGB).
pub const PLAYER_COLOR: PlayerColor = PlayerColor {
    player_main: [155, 111, 154],
    player_alt: [127, 84, 118],
    player_arm: [89, 67, 84],
    player_cape: [118, 84, 127],
    player_cape_edge: [154, 111, 155],
    player_forearm: [158, 115, 154],
};
pub const INVERT_BORDER: bool = false;
/// Cosmetics, only shown if you have unlocked them.
pub const CROWN: bool = true;
pub const AMULET: bool = true;
pub const AMULET_GEM: bool = true;

/// Game settings. Only used when you host, everyone else gets the host's.
pub fn game_settings() -> GameSettings {
    GameSettings {
        // SharedHealth, or LocalHealth(Normal / Alternate / PermaDeath / PvP).
        game_mode: GameMode::LocalHealth(LocalHealthMode::Normal),

        // SharedHealth
        health_per_player: 100,

        // LocalHealth Normal and Alternate
        health_lost_on_revive: 0, // percent of max hp lost on reviving
        global_hp_loss: false,
        // LocalHealth Normal
        no_material_damage: false,
        physics_damage: true,
        // LocalHealth Alternate
        local_health_alternate_dont_run: false, // reviving statue does not run
        revive_on_drop: false,                  // revive player anywhere when dropping heart

        // PvP
        pvp_kill_steal: 50, // percent of money stolen on kill
        dont_steal: true,   // just gain money instead of stealing
        chest_on_win: true,
        timed: true,           // timed rounds/holy mountain
        wait_for_time: false,  // if timed: wait on time to finish round
        time_in_hm: 300,       // if timed: seconds in holy mountain
        time_out_hm: 600,      // if timed: seconds in round
        wait_on_players: true, // if not timed: wait on players to finish round

        // World
        constant_seed: None,     // Some(seed) to always use that seed
        duplicate: false,        // duplicate synced entities
        nice_terraforming: true, // fix blackholes/explosions to work in unseen chunks
        disable_kummitus: false, // on non hosts
        give_host_sampo: false,  // on collection
        enemy_hp_mult: 1.0,
        spell_ban_list: "".into(), // internal names, comma separated

        // Players
        max_players: 250,
        friendly_fire: false,
        share_gold: false,
        same_loadout: false,
        home_on_players: false, // homing targets players

        // Perks
        randomize_perks: true,    // perk pools are independent of each other
        perk_ban_list: "".into(), // internal names, comma separated
        // Global perks to ignore, internal names, comma separated. Undefined behaviour.
        disabled_globals: "".into(),

        debug_mode: false, // debug/cheat mode
    }
}
