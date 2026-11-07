//! Writes the per-player sprite files the mod loads from `files/system/player/tmp`.
//! Everyone looks like default Mina, without crown or amulets.

use crate::protocol::PeerId;
use image::{DynamicImage, Rgba, RgbaImage};
use std::fs::{self, File};
use std::io;
use std::path::Path;

/// Mina's colors as `0xBBGGRR`, which the mod uses for chat text and a few effects.
pub const MINA_COLOR: u32 = 0x9A6F9B;
pub const MINA_COLOR_ALT: u32 = 0x76547F;

const MAIN: Rgba<u8> = Rgba([155, 111, 154, 255]);
const ALT: Rgba<u8> = Rgba([127, 84, 118, 255]);
const ARM: Rgba<u8> = Rgba([89, 67, 84, 255]);
const CAPE: Rgba<u8> = Rgba([118, 84, 127, 255]);
const CAPE_EDGE: Rgba<u8> = Rgba([154, 111, 155, 255]);
const FOREARM: Rgba<u8> = Rgba([158, 115, 154, 255]);
const HAND: Rgba<u8> = Rgba([219, 192, 103, 255]);

/// Empties the generated files folder. The `no_*` files turn off the crown and amulets.
pub fn reset_tmp_dir(mod_path: &Path) -> io::Result<()> {
    let tmp = mod_path.join("files/system/player/tmp");
    fs::remove_dir_all(&tmp).ok();
    fs::create_dir(&tmp).ok();
    for name in ["no_crown", "no_amulet", "no_amulet_gem"] {
        File::create(tmp.join(name))?;
    }
    Ok(())
}

/// The small player picture the mod shows in the PvP UI.
fn icon(player_path: &Path) -> RgbaImage {
    let mut icon = image::open(player_path)
        .unwrap_or(DynamicImage::ImageRgba8(RgbaImage::new(20, 20)))
        .crop(1, 1, 7, 16)
        .into_rgba8();
    for (i, pixel) in icon.pixels_mut().enumerate() {
        if [MAIN, ALT, ARM].contains(pixel) {
            continue;
        }
        match i {
            29 | 36 | 43 => *pixel = FOREARM,
            22 => *pixel = HAND,
            105 | 98 | 91 | 84 | 112 | 99 | 92 | 85 | 78 | 106 | 86 | 79 | 72 | 93 => *pixel = CAPE,
            70 | 77 | 64 | 71 | 65 => *pixel = CAPE_EDGE,
            _ => {}
        }
    }
    icon
}

fn arm() -> RgbaImage {
    let mut img = RgbaImage::new(5, 15);
    for (i, pixel) in img.pixels_mut().enumerate() {
        match i {
            10 | 11 | 17 | 18 | 35 | 40 | 41 | 55 | 56 | 57 | 58 => *pixel = FOREARM,
            19 | 42 | 59 => *pixel = HAND,
            _ => {}
        }
    }
    img
}

pub fn create_player_png(peer: PeerId, mod_path: &Path, is_host: bool) {
    let dir = mod_path.join("files/system/player");
    let player_path = dir.join("unmodified.png");
    let id = peer.as_hex();
    let tmp = |name: &str| dir.join(format!("tmp/{id}{name}"));
    let mod_tmp = |name: &str| format!("mods/quant.ew/files/system/player/tmp/{id}{name}");
    let copy = |from: &Path, name: &str| {
        fs::copy(from, tmp(name)).unwrap();
    };
    let system = mod_path.join("files/system");

    icon(&player_path).save(tmp("_icon.png")).unwrap();
    copy(&player_path, ".png");
    // Faded copy for when the player's game is disconnected.
    let mut dc = image::open(&player_path).unwrap().into_rgba8();
    for px in dc.pixels_mut() {
        px.0[3] = px.0[3].min(64)
    }
    dc.save(tmp("_dc.png")).unwrap();
    copy(&dir.join("unmodified_lukki.png"), "_lukki.png");
    let arrow = if is_host {
        "arrow_host.png"
    } else {
        "arrow.png"
    };
    copy(&system.join("player_arrows").join(arrow), "_arrow.png");
    copy(&system.join("player_ping/arrow.png"), "_ping.png");
    copy(
        &mod_path.join("files/resource/sprites/cursor.png"),
        "_cursor.png",
    );
    copy(&dir.join("knee.png"), "_knee.png");
    copy(&dir.join("limb_a.png"), "_limb_a.png");
    copy(&dir.join("limb_b.png"), "_limb_b.png");
    copy(&system.join("map/icon.png"), "_map.png");

    let mut ragdoll = String::new();
    for s in [
        "head.png",
        "left_hand.png",
        "left_arm.png",
        "left_thigh.png",
        "right_hand.png",
        "right_arm.png",
        "right_thigh.png",
        "torso.png",
    ]
    .iter()
    .rev()
    {
        let name = format!("_ragdoll_{s}");
        copy(&dir.join(s), &name);
        ragdoll += &mod_tmp(&name);
        ragdoll += "\n";
    }
    fs::write(tmp("_ragdoll.txt"), ragdoll).unwrap();

    arm().save(tmp("_arm.png")).unwrap();
    // Mina's cape colors.
    insert_lines(
        &dir.join("unmodified_cape.xml"),
        &tmp("_cape.xml"),
        &[
            (16, r#"cloth_color="0xFF76547F""#.to_string()),
            (16, r#"cloth_color_edge="0xFF9A6F9B""#.to_string()),
        ],
    );
    let unmodified_xml = dir.join("unmodified.xml");
    insert_lines(
        &unmodified_xml,
        &tmp("_dc.xml"),
        &[(1, format!("filename=\"{}\"", mod_tmp("_dc.png")))],
    );
    insert_lines(
        &unmodified_xml,
        &tmp(".xml"),
        &[(1, format!("filename=\"{}\"", mod_tmp(".png")))],
    );
    replace_markers(
        &dir.join("unmodified_lukki.xml"),
        &tmp("_lukki.xml"),
        &[("MARKER_LUKKI_PNG", mod_tmp("_lukki.png"))],
    );
    replace_markers(
        &dir.join("unmodified_base.xml"),
        &tmp("_base.xml"),
        &[
            ("MARKER_HAT2_ENABLED", String::new()),
            ("MARKER_AMULET_ENABLED", String::new()),
            ("MARKER_AMULET_GEM_ENABLED", String::new()),
            ("MARKER_MAIN_SPRITE", mod_tmp(".xml")),
            ("MARKER_LUKKI_SPRITE", mod_tmp("_lukki.xml")),
            ("MARKER_ARM_SPRITE", mod_tmp("_arm.xml")),
            ("MARKER_CAPE", mod_tmp("_cape.xml")),
            ("RAGDOLL_MARKER", mod_tmp("_ragdoll.txt")),
        ],
    );
    insert_lines(
        &dir.join("unmodified_arm.xml"),
        &tmp("_arm.xml"),
        &[(1, format!("filename=\"{}\"", mod_tmp("_arm.png")))],
    );
    // mods.quant.ew.files.system.player.tmp.0000000000000001.png
    fs::copy(
        dir.join("player_uv.png"),
        mod_path
            .join("data/generated/sprite_uv_maps")
            .join(format!("mods.quant.ew.files.system.player.tmp.{id}.png")),
    )
    .unwrap();
}

/// Copies `path` to `out`, inserting each line at its index (applied in order).
fn insert_lines(path: &Path, out: &Path, inserts: &[(usize, String)]) {
    let mut lines: Vec<String> = fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    for (i, line) in inserts {
        lines.insert(*i, line.clone());
    }
    fs::write(out, lines.join("\n") + "\n").unwrap();
}

/// Copies `path` to `out` with every marker replaced.
fn replace_markers(path: &Path, out: &Path, markers: &[(&str, String)]) {
    // Mod files should exist by this point, main.rs checks the mod is installed.
    let mut contents = fs::read_to_string(path).unwrap();
    for (marker, value) in markers {
        contents = contents.replace(marker, value);
    }
    fs::write(out, contents).unwrap();
}
