use std::{
    fs, io,
    path::{Path, PathBuf},
};

use bitcode::{Decode, Encode};
use tracing::{error, info, warn};

pub trait SaveStateEntry: bitcode::Encode + bitcode::DecodeOwned {
    const FILENAME: &'static str;
}

/// The run's seed. Without it there is no run to continue.
#[derive(Encode, Decode)]
pub struct RunInfo {
    pub seed: u64,
}

impl SaveStateEntry for RunInfo {
    /// Bump whenever any save state format changes incompatibly: `SaveState` discards a save
    /// directory that has no run info under this name.
    const FILENAME: &'static str = "run_info_v2";
}

/// The host's run (seed, flags, world, entities) on disk, so it can be continued after a restart.
pub struct SaveState {
    path: PathBuf,
    /// Nothing is saved before Noita connected, so starting and stopping the proxy
    /// without playing doesn't leave a run behind.
    game_started: bool,
}

impl SaveState {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let has_savestate = path.join(format!("{}.bit", RunInfo::FILENAME)).exists();
        info!("Has savestate: {has_savestate}");
        if !has_savestate {
            // Without a run info there is no run to continue, so anything else in here is either
            // stale or from an older format. bitcode is not self describing, so bumping the run
            // info filename is what keeps old saves from being decoded as the new shapes.
            fs::remove_dir_all(path).ok();
        }
        if let Err(err) = fs::create_dir_all(path) {
            error!("Error while creating directories: {err}");
        }
        let path = path.canonicalize().unwrap_or(path.to_path_buf());
        info!("Will save to: {}", path.display());
        Self {
            path,
            game_started: false,
        }
    }

    pub fn save<D: SaveStateEntry>(&self, data: &D) {
        if !self.game_started {
            info!("Skipping save of {}, game not started yet", D::FILENAME);
            return;
        }

        let path = self.path_for_filename(D::FILENAME);
        let encoded = bitcode::encode(data);
        let compressed = lz4_flex::compress_prepend_size(&encoded);
        if let Err(err) = fs::write(&path, compressed) {
            error!("Error while saving to {:?}: {err}", D::FILENAME);
        }
        info!("Saved {}", path.display());
    }

    pub fn load<D: SaveStateEntry>(&self) -> Option<D> {
        let path = self.path_for_filename(D::FILENAME);
        let data = fs::read(&path)
            .inspect_err(|err| {
                if err.kind() == io::ErrorKind::NotFound {
                    info!("No save for {}, no need to load anything", D::FILENAME)
                } else {
                    warn!("Could not read {:?}: {err}", D::FILENAME)
                }
            })
            .ok()?;
        let data = lz4_flex::decompress_size_prepended(&data)
            .inspect_err(|err| warn!("Could not decompress {:?}: {err}", D::FILENAME))
            .ok()?;
        bitcode::decode(&data)
            .inspect_err(|err| error!("Could not decode {:?}: {err}", D::FILENAME))
            .ok()
    }

    pub fn mark_game_started(&mut self) {
        self.game_started = true;
    }

    pub fn reset(&self) {
        fs::remove_dir_all(&self.path).ok();
        fs::create_dir_all(&self.path).ok();
    }

    fn path_for_filename(&self, filename: &str) -> PathBuf {
        self.path.join(format!("{filename}.bit"))
    }
}
