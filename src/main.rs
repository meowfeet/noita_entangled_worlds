//! Noita Entangled Worlds proxy, command line only. Options are in settings.rs.
//!
//! `noita_proxy` hosts a Steam lobby and prints its code, `noita_proxy <lobby code>` joins one.
//! `--new-game` throws away the saved run instead of continuing it. Stop with Ctrl+C.

mod game_settings;
mod net;
mod player_cosmetics;
mod protocol;
mod save_state;
mod settings;

use std::{env, path::PathBuf, process::exit, sync::atomic::Ordering, thread, time::Duration};

use net::steam_networking::{SteamPeer, parse_lobby_code};
use net::{NetManager, NetManagerInit};
use save_state::SaveState;
use steamworks::{AppId, LobbyType};
use tracing::{error, info};

const NOITA_APP_ID: u32 = 881100;

fn fail(msg: impl std::fmt::Display) -> ! {
    error!("{msg}");
    exit(1)
}

fn main() {
    tracing_subscriber::fmt().init();

    let mut new_game = false;
    let mut lobby = None;
    for arg in env::args().skip(1) {
        if arg == "--new-game" {
            new_game = true;
        } else {
            lobby = Some(arg);
        }
    }

    ctrlc::set_handler(|| net::RUNNING.store(false, Ordering::Relaxed))
        .expect("Ctrl+C handler can be set");

    let client = steamworks::Client::init_app(NOITA_APP_ID)
        .unwrap_or_else(|err| fail(format!("Could not init Steam, is it running? {err}")));
    client.networking_utils().init_relay_network_access();
    if let Err(err) = client.networking_sockets().init_authentication() {
        error!("Failed to init_authentication: {}", err)
    }
    let callbacks = client.clone();
    thread::spawn(move || {
        loop {
            callbacks.run_callbacks();
            thread::sleep(Duration::from_millis(3));
        }
    });

    let noita_dir = match settings::NOITA_EXE {
        Some(exe) => PathBuf::from(exe)
            .parent()
            .expect("NOITA_EXE is a path to noita.exe")
            .to_path_buf(),
        None => {
            let apps = client.apps();
            if !apps.is_app_installed(AppId(NOITA_APP_ID)) {
                fail("Noita isn't installed through Steam, set NOITA_EXE in settings.rs");
            }
            PathBuf::from(apps.app_install_dir(AppId(NOITA_APP_ID)))
        }
    };
    let mod_path = noita_dir.join("mods/quant.ew");
    if !mod_path.join("files/system/player/unmodified.png").exists() {
        fail(format!(
            "The quant.ew mod isn't installed in {}",
            mod_path.display()
        ));
    }
    info!("Mod path: {}", mod_path.display());

    // Noita uses AppData folder instead of %AppData%
    let noita_save = PathBuf::from(env::var_os("USERPROFILE").expect("USERPROFILE is set"))
        .join(r"AppData\LocalLow\Nolla_Games_Noita");
    let noita_save = noita_save.exists().then_some(noita_save);
    if noita_save.is_none() {
        info!("Could not find Noita's save folder");
    }

    let my_nickname = settings::NICKNAME
        .map(String::from)
        .unwrap_or_else(|| client.friends().get_friend(client.user().steam_id()).name());

    let save_state = SaveState::new(settings::save_state_path());
    if new_game {
        info!("Starting a new run");
        save_state.reset();
    }

    let peer = match lobby {
        None => {
            let lobby_type = if settings::PUBLIC_LOBBY {
                LobbyType::Public
            } else if settings::ALLOW_FRIENDS {
                LobbyType::FriendsOnly
            } else {
                LobbyType::Private
            };
            let max_players = settings::game_settings().max_players;
            SteamPeer::new_host(lobby_type, client, max_players)
        }
        Some(code) => {
            let lobby = parse_lobby_code(&code)
                .unwrap_or_else(|| fail(format!("Invalid lobby code: {code}")));
            SteamPeer::new_connect(lobby, client)
        }
    };

    let mut netman = NetManager::new(
        peer,
        NetManagerInit {
            my_nickname,
            save_state,
            mod_path,
            noita_save,
        },
    );
    let result = netman.run();
    // Dropping saves the run, so do it before exiting.
    drop(netman);
    if let Err(err) = result {
        fail(err);
    }
}
