//! Generates the recolored player sprites the mod loads from `files/system/player/tmp`.

use crate::net::omni::OmniPeerId;
use bitcode::{Decode, Encode};
use image::{DynamicImage, Pixel, Rgba, RgbaImage};
use std::fs::{self, File};
use std::io;
use std::path::Path;

#[derive(Debug, Decode, Encode, Copy, Clone)]
pub struct PlayerColor {
    pub player_main: [u8; 3],
    pub player_alt: [u8; 3],
    pub player_arm: [u8; 3],
    pub player_cape: [u8; 3],
    pub player_cape_edge: [u8; 3],
    pub player_forearm: [u8; 3],
}

impl Default for PlayerColor {
    /// Mina's original colors.
    fn default() -> Self {
        Self {
            player_main: [155, 111, 154],
            player_alt: [127, 84, 118],
            player_arm: [89, 67, 84],
            player_cape: [118, 84, 127],
            player_cape_edge: [154, 111, 155],
            player_forearm: [158, 115, 154],
        }
    }
}

#[derive(Clone, Copy, Debug, Decode, Encode, Default)]
pub struct PlayerPngDesc {
    /// Crown, amulet, amulet gem.
    pub cosmetics: [bool; 3],
    pub colors: PlayerColor,
    pub invert_border: bool,
}

const TARGET_MAIN: Rgba<u8> = Rgba([155, 111, 154, 255]);
const TARGET_ALT: Rgba<u8> = Rgba([127, 84, 118, 255]);
const TARGET_ARM: Rgba<u8> = Rgba([89, 67, 84, 255]);
const TARGET_BORDER: Rgba<u8> = Rgba([0, 0, 0, 255]);

fn rgba(c: [u8; 3]) -> Rgba<u8> {
    Rgba([c[0], c[1], c[2], 255])
}

fn compare_rgb(a: Rgba<u8>, b: Rgba<u8>) -> bool {
    a.channels()[0..3] == b.channels()[0..3]
}

fn set_rgb(a: &mut Rgba<u8>, b: Rgba<u8>) {
    a.channels_mut()[0..3].copy_from_slice(&b.channels()[0..3]);
}

fn invert(mut a: Rgba<u8>) -> Rgba<u8> {
    for c in &mut a.channels_mut()[0..3] {
        *c = 255 - *c;
    }
    a
}

fn replace_color(image: &mut RgbaImage, rgb: &PlayerColor, invert_border: bool) {
    let main = rgba(rgb.player_main);
    let alt = rgba(rgb.player_alt);
    let arm = rgba(rgb.player_arm);
    for pixel in image.pixels_mut() {
        if compare_rgb(*pixel, TARGET_MAIN) {
            set_rgb(pixel, main);
        } else if compare_rgb(*pixel, TARGET_ALT) {
            set_rgb(pixel, alt);
        } else if compare_rgb(*pixel, TARGET_ARM) {
            set_rgb(pixel, arm);
        } else if invert_border && compare_rgb(*pixel, TARGET_BORDER) {
            set_rgb(pixel, invert(main));
        }
    }
}

fn recolor(path: &Path, save: &Path, rgb: &PlayerColor, invert_border: bool) {
    let mut img = image::open(path).unwrap().into_rgba8();
    replace_color(&mut img, rgb, invert_border);
    img.save(save).unwrap();
}

fn make_player_image(image: &mut RgbaImage, colors: PlayerColor) {
    let main = rgba(colors.player_main);
    let alt = rgba(colors.player_alt);
    let arm = rgba(colors.player_arm);
    let cape = rgba(colors.player_cape);
    let cape_edge = rgba(colors.player_cape_edge);
    let forearm = rgba(colors.player_forearm);
    for (i, pixel) in image.pixels_mut().enumerate() {
        if *pixel == TARGET_MAIN {
            *pixel = main
        } else if *pixel == TARGET_ALT {
            *pixel = alt
        } else if *pixel == TARGET_ARM {
            *pixel = arm
        } else {
            match i {
                29 | 36 | 43 => *pixel = forearm,
                22 => *pixel = Rgba::from([219, 192, 103, 255]),
                105 | 98 | 91 | 84 | 112 | 99 | 92 | 85 | 78 | 106 | 86 | 79 | 72 | 93 => {
                    *pixel = cape
                }
                70 | 77 | 64 | 71 | 65 => *pixel = cape_edge,
                _ => {}
            }
        }
    }
}

fn add_cosmetics(image: &mut RgbaImage, cosmetics: &[bool]) {
    for (i, pixel) in image.pixels_mut().enumerate() {
        match i {
            2 | 4 | 6 if cosmetics[0] => *pixel = Rgba::from([255, 244, 140, 255]),
            9 | 13 if cosmetics[0] => *pixel = Rgba::from([191, 141, 65, 255]),
            10..=12 if cosmetics[0] => *pixel = Rgba::from([255, 206, 98, 255]),
            54 if cosmetics[2] => *pixel = Rgba::from([255, 242, 162, 255]),
            60 if cosmetics[2] => *pixel = Rgba::from([255, 227, 133, 255]),
            61 if cosmetics[2] => *pixel = Rgba::from([255, 94, 38, 255]),
            62 | 68 if cosmetics[2] => *pixel = Rgba::from([247, 188, 86, 255]),
            45 | 53 if cosmetics[1] => *pixel = Rgba::from([247, 188, 86, 255]),
            54 if cosmetics[1] => *pixel = Rgba::from([255, 227, 133, 255]),
            61 if cosmetics[1] => *pixel = Rgba::from([255, 242, 162, 255]),
            55 if cosmetics[1] => *pixel = Rgba::from([198, 111, 57, 255]),
            48 if cosmetics[1] => *pixel = Rgba::from([177, 97, 48, 149]),
            _ => {}
        }
    }
}

fn create_arm(arm: Rgba<u8>) -> RgbaImage {
    let hand = Rgba::from([219, 192, 103, 255]);
    let mut img = RgbaImage::new(5, 15);
    for (i, pixel) in img.pixels_mut().enumerate() {
        match i {
            10 | 11 | 17 | 18 | 35 | 40 | 41 | 55 | 56 | 57 | 58 => *pixel = arm,
            19 | 42 | 59 => *pixel = hand,
            _ => {}
        }
    }
    img
}

/// Empties the generated files folder and marks the cosmetics that are turned off.
pub fn reset_tmp_dir(mod_path: &Path, cosmetics: [bool; 3]) -> io::Result<()> {
    let tmp = mod_path.join("files/system/player/tmp");
    fs::remove_dir_all(&tmp).ok();
    fs::create_dir(&tmp).ok();
    for (enabled, name) in cosmetics
        .iter()
        .zip(["no_crown", "no_amulet", "no_amulet_gem"])
    {
        if !enabled {
            File::create(tmp.join(name))?;
        }
    }
    Ok(())
}

pub fn create_player_png(peer: OmniPeerId, mod_path: &Path, desc: &PlayerPngDesc, is_host: bool) {
    let dir = mod_path.join("files/system/player");
    let player_path = dir.join("unmodified.png");
    let id = peer.as_hex();
    let tmp = |name: &str| dir.join(format!("tmp/{id}{name}"));
    let mod_tmp = |name: &str| format!("mods/quant.ew/files/system/player/tmp/{id}{name}");
    let system = mod_path.join("files/system");
    let rgb = desc.colors;
    let inv = desc.invert_border;

    let mut icon = image::open(&player_path)
        .unwrap_or(DynamicImage::ImageRgba8(RgbaImage::new(20, 20)))
        .crop(1, 1, 7, 16)
        .into_rgba8();
    add_cosmetics(&mut icon, &desc.cosmetics);
    make_player_image(&mut icon, rgb);
    icon.save(tmp("_icon.png")).unwrap();

    recolor(&player_path, &tmp(".png"), &rgb, false);
    {
        let mut img = image::open(&player_path).unwrap().into_rgba8();
        replace_color(&mut img, &rgb, false);
        for px in img.pixels_mut() {
            px.0[3] = px.0[3].min(64)
        }
        img.save(tmp("_dc.png")).unwrap();
    }
    recolor(
        &dir.join("unmodified_lukki.png"),
        &tmp("_lukki.png"),
        &rgb,
        false,
    );
    let arrow = if is_host {
        "arrow_host.png"
    } else {
        "arrow.png"
    };
    recolor(
        &system.join("player_arrows").join(arrow),
        &tmp("_arrow.png"),
        &rgb,
        inv,
    );
    recolor(
        &system.join("player_ping/arrow.png"),
        &tmp("_ping.png"),
        &rgb,
        inv,
    );
    recolor(
        &mod_path.join("files/resource/sprites/cursor.png"),
        &tmp("_cursor.png"),
        &rgb,
        inv,
    );
    recolor(&dir.join("knee.png"), &tmp("_knee.png"), &rgb, false);
    recolor(&dir.join("limb_a.png"), &tmp("_limb_a.png"), &rgb, false);
    recolor(&dir.join("limb_b.png"), &tmp("_limb_b.png"), &rgb, false);
    recolor(&system.join("map/icon.png"), &tmp("_map.png"), &rgb, false);

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
        recolor(&dir.join(s), &tmp(&name), &rgb, false);
        ragdoll += &mod_tmp(&name);
        ragdoll += "\n";
    }
    fs::write(tmp("_ragdoll.txt"), ragdoll).unwrap();

    create_arm(rgba(rgb.player_forearm))
        .save(tmp("_arm.png"))
        .unwrap();
    insert_lines(
        &dir.join("unmodified_cape.xml"),
        &tmp("_cape.xml"),
        &[
            (
                16,
                format!("cloth_color=\"0xFF{}\"", rgb_to_hex(rgb.player_cape)),
            ),
            (
                16,
                format!(
                    "cloth_color_edge=\"0xFF{}\"",
                    rgb_to_hex(rgb.player_cape_edge)
                ),
            ),
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
    let cosmetic = |enabled: bool, file: &str| {
        if enabled {
            format!("image_file=\"data/enemies_gfx/{file}\"")
        } else {
            String::new()
        }
    };
    replace_markers(
        &dir.join("unmodified_base.xml"),
        &tmp("_base.xml"),
        &[
            (
                "MARKER_HAT2_ENABLED",
                cosmetic(desc.cosmetics[0], "player_hat2.xml"),
            ),
            (
                "MARKER_AMULET_ENABLED",
                cosmetic(desc.cosmetics[1], "player_amulet.xml"),
            ),
            (
                "MARKER_AMULET_GEM_ENABLED",
                cosmetic(desc.cosmetics[2], "player_amulet_gem.xml"),
            ),
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

fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}
