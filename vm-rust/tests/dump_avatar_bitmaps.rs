//! Dump named bitmap members from `avatarengine.cct` and `people.cct`.
//!
//! Run:
//!   CASTS_ROOT=/abs/path/upstream/cokemusic-casts/client2 \
//!   OUTPUT_ROOT=/tmp/avatar-dump \
//!   UTM_PARTS_ROOT=/abs/path/upstream/uncover-the-music/game/src/assets/parts \
//!   cargo test -p vm-rust --test dump_avatar_bitmaps -- --nocapture
//!
//! Output:
//!   - <OUTPUT_ROOT>/avatars/<cast>/data/<member>.png
//!   - <OUTPUT_ROOT>/avatars/<cast>/_members.json
//!   - <OUTPUT_ROOT>/avatars/people/_utm_comparison.json when
//!     UTM_PARTS_ROOT is set

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use base64::Engine;
use fxhash::FxHashMap;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::mcp::{
    build_palette_table, collect_palette_table, mcp_get_cast_member_picture,
    palettes_manifest_json,
};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

const AVATAR_CASTS: &[(&str, &str)] = &[
    ("avatarengine.cct", "avatarengine"),
    ("people.cct", "people"),
];

fn casts_root() -> String {
    std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string())
}

fn output_root() -> PathBuf {
    PathBuf::from(std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string()))
        .join("avatars")
}

#[test]
fn dump_avatar_cct_bitmaps() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    fs::create_dir_all(output_root()).expect("create avatar output root");

    let custom_inks = std::env::var("UTM_PARTS_ROOT")
        .ok()
        .map(|root| read_custom_inks(&PathBuf::from(root).join("custominks.txt")))
        .unwrap_or_default();
    let mut extracted_by_cast: HashMap<String, BTreeSet<String>> = HashMap::new();
    // Deduplicated CLUTs across both avatar casts, keyed by the same
    // `paletteRef` string every `_members.json` entry carries. 2,435 members
    // share a handful of tables, so this is emitted once at the avatars root
    // as `_palettes.json` rather than inlined per member (~7 MB saved).
    let mut palette_tables: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    for (cct_name, output_name) in AVATAR_CASTS {
        let cct_path = PathBuf::from(casts_root()).join(cct_name);
        assert!(cct_path.exists(), "cct not found at {}", cct_path.display());

        let cast_output = output_root().join(output_name);
        let png_output = cast_output.join("data");
        fs::create_dir_all(&png_output).expect("create avatar PNG output dir");

        let mut player = TestPlayer::new();
        player
            .load_movie(cct_path.to_str().expect("UTF-8 cct path"))
            .await;
        reserve_player_mut(|player| {
            player.movie.cast_manager.casts.clear();
            let dir = player.movie.file.take().expect("loaded Director file");
            for (index, cast_def) in dir.casts.iter().enumerate() {
                let mut cast = CastLib {
                    name: cast_def.name.clone(),
                    file_name: dir.file_name.clone(),
                    number: (index + 1) as u32,
                    is_external: false,
                    state: CastLibState::Loaded,
                    lctx: cast_def.lctx.clone(),
                    members: FxHashMap::default(),
                    scripts: FxHashMap::default(),
                    preload_mode: 0,
                    capital_x: false,
                    dir_version: 0,
                    palette_id_offset: cast_def.palette_id_offset,
                };
                cast.apply_cast_def(&dir, cast_def, &mut player.bitmap_manager, &dir.font_table);
                player.movie.cast_manager.casts.push(cast);
            }
            player.movie.file = Some(dir);
        });

        let mut all_named = Vec::new();
        let mut total_bitmaps = 0usize;
        let mut unnamed_bitmaps = 0usize;
        reserve_player_ref(|player| {
            for cast in &player.movie.cast_manager.casts {
                for (member_number, member) in &cast.members {
                    if !matches!(member.member_type, CastMemberType::Bitmap(_)) {
                        continue;
                    }
                    total_bitmaps += 1;
                    if member.name.is_empty() {
                        unnamed_bitmaps += 1;
                        continue;
                    }
                    let bitmap_member = member.member_type.as_bitmap().expect("bitmap member");
                    all_named.push((
                        cast.number as i32,
                        *member_number as i32,
                        member.name.clone(),
                        bitmap_member.info.width,
                        bitmap_member.info.height,
                    ));
                }
            }
        });
        all_named.sort_by_key(|(cast_lib, cast_member, _, _, _)| (*cast_lib, *cast_member));

        let mut seen_names = HashSet::new();
        let mut targets = Vec::new();
        let mut duplicate_names = 0usize;
        for target in all_named {
            if !seen_names.insert(target.2.clone()) {
                duplicate_names += 1;
            }
            targets.push(target);
        }
        targets.sort_by(|left, right| left.2.cmp(&right.2));

        let mut metadata = Vec::with_capacity(targets.len());
        let mut extracted_names = BTreeSet::new();
        let mut emitted_names = HashSet::new();
        let mut empty_named_bitmaps = 0usize;
        for (cast_lib, cast_member, name, declared_width, declared_height) in targets {
            let first_named_member = emitted_names.insert(name.clone());
            let key = if first_named_member {
                name.clone()
            } else {
                format!("{name}#{cast_lib}_{cast_member}")
            };
            let file_stem = if first_named_member {
                name.clone()
            } else {
                format!("{name}__cast{cast_lib}_{cast_member}")
            };
            if declared_width == 0 || declared_height == 0 {
                let (
                    reg_x,
                    reg_y,
                    bit_depth,
                    original_bit_depth,
                    use_alpha,
                    palette_ref,
                    palette_indexed,
                    palette_table,
                ) = reserve_player_ref(|player| {
                        let cast = player
                            .movie
                            .cast_manager
                            .get_cast(cast_lib as u32)
                            .expect("empty bitmap cast");
                        let member = cast
                            .members
                            .get(&(cast_member as u32))
                            .expect("empty bitmap member");
                        let bitmap_member =
                            member.member_type.as_bitmap().expect("empty bitmap type");
                        let bitmap = player
                            .bitmap_manager
                            .get_bitmap(bitmap_member.image_ref)
                            .expect("empty bitmap data");
                        // 0x0 placeholders never reach the MCP picture path,
                        // so resolve their CLUT here too — otherwise their
                        // `paletteRef` would dangle in `_palettes.json`.
                        let palette_table = bitmap.has_palette().then(|| {
                            serde_json::to_value(build_palette_table(
                                player,
                                &bitmap.palette_ref,
                            ))
                            .expect("serialize palette table")
                        });
                        (
                            bitmap_member.reg_point.0,
                            bitmap_member.reg_point.1,
                            bitmap.bit_depth,
                            bitmap.original_bit_depth,
                            bitmap.use_alpha,
                            format!("{:?}", bitmap.palette_ref),
                            bitmap.has_palette(),
                            palette_table,
                        )
                    });
                if let Some(table) = &palette_table {
                    if let Some(key) = table.get("key").and_then(|v| v.as_str()) {
                        palette_tables
                            .entry(key.to_string())
                            .or_insert_with(|| table.clone());
                    }
                }
                metadata.push(serde_json::json!({
                    "name": name,
                    "key": key,
                    "filename": null,
                    "sourceCct": cct_name,
                    "castLib": cast_lib,
                    "castMember": cast_member,
                    "regX": reg_x,
                    "regY": reg_y,
                    "bitDepth": bit_depth,
                    "originalBitDepth": original_bit_depth,
                    "useAlpha": use_alpha,
                    "paletteRef": palette_ref,
                    "paletteIndexed": palette_indexed,
                    "customInk": null,
                    "width": declared_width,
                    "height": declared_height,
                    "empty": true,
                }));
                empty_named_bitmaps += 1;
                continue;
            }

            let json = reserve_player_ref(|player| {
                mcp_get_cast_member_picture(player, cast_lib, cast_member)
            });
            let parsed: serde_json::Value = serde_json::from_str(&json)
                .unwrap_or_else(|error| panic!("invalid MCP JSON for {name}: {error}"));
            if let Some(error) = parsed.get("error").and_then(|value| value.as_str()) {
                panic!("avatar bitmap export failed for {name}: {error}");
            }
            let encoded = parsed
                .get("png_base64")
                .and_then(|value| value.as_str())
                .unwrap_or_else(|| panic!("missing PNG data for {name}"));
            let png = base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .unwrap_or_else(|error| panic!("invalid PNG base64 for {name}: {error}"));
            fs::write(png_output.join(format!("{file_stem}.png")), png)
                .unwrap_or_else(|error| panic!("write avatar PNG {name}: {error}"));

            collect_palette_table(&mut palette_tables, &parsed);
            let custom_ink = custom_inks
                .iter()
                .find(|(key, _)| name.contains(&format!("_{key}_")))
                .map(|(_, ink)| *ink);
            metadata.push(serde_json::json!({
                "name": name,
                "key": key,
                "filename": format!("{file_stem}.png"),
                "sourceCct": cct_name,
                "castLib": cast_lib,
                "castMember": cast_member,
                "regX": parsed.get("reg_x"),
                "regY": parsed.get("reg_y"),
                "bitDepth": parsed.get("bit_depth"),
                "originalBitDepth": parsed.get("original_bit_depth"),
                "useAlpha": parsed.get("use_alpha"),
                "paletteRef": parsed.get("palette_ref"),
                "paletteIndexed": parsed.get("palette_indexed"),
                "customInk": custom_ink,
                "width": parsed.get("width"),
                "height": parsed.get("height"),
            }));
            if first_named_member {
                extracted_names.insert(name);
            }
        }

        let metadata_path = cast_output.join("_members.json");
        fs::write(
            &metadata_path,
            serde_json::to_string_pretty(&metadata).expect("serialize avatar metadata"),
        )
        .expect("write avatar metadata");
        println!(
            "{}: {} PNGs, {} named empty, {} unnamed, {} duplicate names preserved -> {}",
            cct_name,
            metadata.len() - empty_named_bitmaps,
            empty_named_bitmaps,
            unnamed_bitmaps,
            duplicate_names,
            cast_output.display()
        );
        assert_eq!(total_bitmaps, metadata.len() + unnamed_bitmaps);
        extracted_by_cast.insert((*output_name).to_string(), extracted_names);
    }

    let palettes_path = output_root().join("_palettes.json");
    fs::write(
        &palettes_path,
        palettes_manifest_json("dump_avatar_bitmaps", &palette_tables),
    )
    .expect("write avatar _palettes.json");
    println!(
        "palettes: {} distinct CLUTs -> {}",
        palette_tables.len(),
        palettes_path.display()
    );

    if let Ok(utm_root) = std::env::var("UTM_PARTS_ROOT") {
        let people_names = extracted_by_cast
            .get("people")
            .expect("people extraction names");
        compare_utm_parts(Path::new(&utm_root), people_names);
    }
}

fn read_custom_inks(path: &Path) -> BTreeMap<String, u32> {
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .filter_map(|entry| {
            let (key, value) = entry.split_once(':')?;
            Some((
                key.trim().trim_matches('"').to_string(),
                value.trim().parse().ok()?,
            ))
        })
        .collect()
}

fn compare_utm_parts(utm_root: &Path, people_names: &BTreeSet<String>) {
    assert!(
        utm_root.exists(),
        "UTM parts root not found at {}",
        utm_root.display()
    );

    let utm_names: BTreeSet<String> = fs::read_dir(utm_root)
        .expect("read UTM parts root")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|value| value.to_str()) == Some("png"))
                .then(|| path.file_stem()?.to_str().map(str::to_string))?
        })
        .collect();
    let common: Vec<String> = people_names.intersection(&utm_names).cloned().collect();
    let cast_only: Vec<String> = people_names.difference(&utm_names).cloned().collect();
    let utm_only: Vec<String> = utm_names.difference(people_names).cloned().collect();

    let people_output = output_root().join("people/data");
    let mut byte_identical = Vec::new();
    let mut pixel_identical = Vec::new();
    let mut visible_rgb_identical = Vec::new();
    let mut visible_rgb_mismatches = Vec::new();
    let mut dimension_mismatches = Vec::new();
    let mut pixel_mismatches = Vec::new();
    for name in &common {
        let cast_bytes = fs::read(people_output.join(format!("{name}.png")))
            .unwrap_or_else(|error| panic!("read extracted {name}: {error}"));
        let utm_bytes = fs::read(utm_root.join(format!("{name}.png")))
            .unwrap_or_else(|error| panic!("read UTM {name}: {error}"));
        if cast_bytes == utm_bytes {
            byte_identical.push(name.clone());
        }

        let cast_image = image::load_from_memory(&cast_bytes)
            .unwrap_or_else(|error| panic!("decode extracted {name}: {error}"))
            .to_rgba8();
        let utm_image = image::load_from_memory(&utm_bytes)
            .unwrap_or_else(|error| panic!("decode UTM {name}: {error}"))
            .to_rgba8();
        if cast_image.dimensions() != utm_image.dimensions() {
            dimension_mismatches.push(serde_json::json!({
                "name": name,
                "cast": cast_image.dimensions(),
                "utm": utm_image.dimensions(),
            }));
        } else if cast_image == utm_image {
            pixel_identical.push(name.clone());
            visible_rgb_identical.push(name.clone());
        } else {
            let visible_rgb_matches =
                cast_image
                    .pixels()
                    .zip(utm_image.pixels())
                    .all(|(cast_pixel, utm_pixel)| {
                        utm_pixel.0[3] == 0 || cast_pixel.0[..3] == utm_pixel.0[..3]
                    });
            if visible_rgb_matches {
                visible_rgb_identical.push(name.clone());
            } else {
                visible_rgb_mismatches.push(name.clone());
            }
            pixel_mismatches.push(name.clone());
        }
    }

    let comparison = serde_json::json!({
        "castCount": people_names.len(),
        "utmCount": utm_names.len(),
        "commonCount": common.len(),
        "castOnly": cast_only,
        "utmOnly": utm_only,
        "byteIdenticalCount": byte_identical.len(),
        "byteIdentical": byte_identical,
        "pixelIdenticalCount": pixel_identical.len(),
        "pixelIdentical": pixel_identical,
        "visibleRgbIdenticalCount": visible_rgb_identical.len(),
        "visibleRgbIdentical": visible_rgb_identical,
        "visibleRgbMismatchCount": visible_rgb_mismatches.len(),
        "visibleRgbMismatches": visible_rgb_mismatches,
        "dimensionMismatches": dimension_mismatches,
        "pixelMismatchCount": pixel_mismatches.len(),
        "pixelMismatches": pixel_mismatches,
    });
    let comparison_path = output_root().join("people/_utm_comparison.json");
    fs::write(
        &comparison_path,
        serde_json::to_string_pretty(&comparison).expect("serialize UTM comparison"),
    )
    .expect("write UTM comparison");
    println!(
        "UTM comparison: {} common, {} cast-only, {} UTM-only, {} pixel-identical -> {}",
        common.len(),
        comparison["castOnly"].as_array().map_or(0, Vec::len),
        comparison["utmOnly"].as_array().map_or(0, Vec::len),
        comparison["pixelIdenticalCount"].as_u64().unwrap_or(0),
        comparison_path.display()
    );
}

#[cfg(test)]
mod tests {
    use super::read_custom_inks;
    use std::fs;

    #[test]
    fn parses_utm_custom_ink_mapping() {
        let path = std::env::temp_dir().join(format!(
            "dirplayer-custominks-{}-{}.txt",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::write(&path, "[\"hr_017\":36]").expect("write custom ink fixture");

        let result = read_custom_inks(&path);

        fs::remove_file(path).ok();
        assert_eq!(result.get("hr_017"), Some(&36));
    }
}
