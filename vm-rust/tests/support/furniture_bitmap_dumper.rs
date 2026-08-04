use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::mcp::{
    collect_palette_table, mcp_get_cast_member_picture, palettes_manifest_json,
};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

#[derive(Clone, Copy)]
pub(crate) struct FurnitureBitmapProfile {
    pub(crate) source_cct: &'static str,
    pub(crate) output_subdirectory: &'static str,
    pub(crate) members_sidecar: &'static str,
    pub(crate) dumper_name: &'static str,
}

#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct FurnitureBitmapDumpReport {
    pub(crate) named_bitmap_members: usize,
    pub(crate) palette_members: usize,
    pub(crate) duplicate_names: usize,
    pub(crate) empty_named_bitmaps: usize,
    pub(crate) pngs_written: usize,
    pub(crate) member_entries: usize,
    pub(crate) emitted_names: Vec<String>,
    pub(crate) folded_name_collisions: Vec<(String, Vec<String>)>,
    pub(crate) decode_failures: Vec<String>,
}

fn casts_root() -> String {
    std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string())
}

fn output_dir(profile: FurnitureBitmapProfile) -> String {
    let root = std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string());
    format!("{}/{}", root, profile.output_subdirectory)
}

fn png_dir(profile: FurnitureBitmapProfile) -> String {
    format!("{}/data", output_dir(profile))
}

fn synthesize_casts_from_loaded_movie() {
    reserve_player_mut(|player| {
        player.movie.cast_manager.casts.clear();
        let dir_opt = player.movie.file.take();
        if let Some(dir) = dir_opt {
            for (idx, cast_def) in dir.casts.iter().enumerate() {
                let mut cast = CastLib {
                    name: cast_def.name.clone(),
                    file_name: dir.file_name.clone(),
                    number: (idx + 1) as u32,
                    is_external: false,
                    state: CastLibState::Loaded,
                    lctx: cast_def.lctx.clone(),
                    members: FxHashMap::default(),
                    scripts: FxHashMap::default(),
                    preload_mode: 0u16,
                    capital_x: false,
                    dir_version: 0,
                    palette_id_offset: cast_def.palette_id_offset,
                };
                cast.apply_cast_def(&dir, cast_def, &mut player.bitmap_manager, &dir.font_table);
                player.movie.cast_manager.casts.push(cast);
            }
            player.movie.file = Some(dir);
        }
    });
}

pub(crate) async fn dump_furniture_profile(
    profile: FurnitureBitmapProfile,
) -> FurnitureBitmapDumpReport {
    fs::create_dir_all(output_dir(profile)).expect("create furniture assets dir");
    fs::create_dir_all(png_dir(profile)).expect("create furniture data dir");

    let cct_path = format!("{}/{}", casts_root(), profile.source_cct);
    if !PathBuf::from(&cct_path).exists() {
        panic!("cct not found at {} - set CASTS_ROOT", cct_path);
    }

    let mut player = TestPlayer::new();
    player.load_movie(&cct_path).await;
    synthesize_casts_from_loaded_movie();

    let mut all_named: Vec<(i32, i32, String)> = Vec::new();
    let mut total_bitmaps = 0usize;
    let mut unnamed_bitmaps = 0usize;
    let mut palette_members = 0usize;
    let mut empty_bitmaps: Vec<String> = Vec::new();

    reserve_player_ref(|player| {
        for cast in &player.movie.cast_manager.casts {
            for (member_num, member) in &cast.members {
                match &member.member_type {
                    CastMemberType::Bitmap(bitmap_member) => {
                        total_bitmaps += 1;
                        if member.name.is_empty() {
                            unnamed_bitmaps += 1;
                            continue;
                        }
                        if bitmap_member.info.width == 0 || bitmap_member.info.height == 0 {
                            empty_bitmaps.push(member.name.clone());
                            continue;
                        }
                        all_named.push((
                            cast.number as i32,
                            *member_num as i32,
                            member.name.clone(),
                        ));
                    }
                    CastMemberType::Palette(_) => palette_members += 1,
                    _ => {}
                }
            }
        }
    });
    empty_bitmaps.sort();
    all_named.sort_by_key(|(cast_lib, cast_member, _)| (*cast_lib, *cast_member));

    let named_bitmap_members = all_named.len() + empty_bitmaps.len();
    let mut folded_names: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, _, name) in &all_named {
        folded_names
            .entry(name.to_ascii_lowercase())
            .or_default()
            .insert(name.clone());
    }
    let folded_name_collisions = folded_names
        .into_iter()
        .filter_map(|(folded, originals)| {
            (originals.len() > 1).then(|| (folded, originals.into_iter().collect()))
        })
        .collect::<Vec<_>>();

    let mut seen_names = HashSet::new();
    let mut targets: Vec<(i32, i32, String)> = Vec::new();
    let mut duplicate_names = 0usize;
    for (cast_lib, cast_member, name) in all_named {
        if seen_names.insert(name.clone()) {
            targets.push((cast_lib, cast_member, name));
        } else {
            duplicate_names += 1;
        }
    }
    targets.sort_by(|a, b| a.2.cmp(&b.2));

    println!();
    println!("=== {} extraction summary ===", profile.source_cct);
    println!(
        "  loaded {} bitmap members ({} named, {} unnamed, {} duplicate names skipped, {} declared 0x0 skipped) and {} palette members",
        total_bitmaps,
        named_bitmap_members,
        unnamed_bitmaps,
        duplicate_names,
        empty_bitmaps.len(),
        palette_members,
    );
    if !empty_bitmaps.is_empty() {
        println!("    declared 0x0: {}", empty_bitmaps.join(", "));
    }

    let mut members_meta: Vec<serde_json::Value> = Vec::new();
    let mut decode_failures: Vec<String> = Vec::new();
    let mut pngs_written = 0usize;
    let mut emitted_names: Vec<String> = Vec::new();
    let mut palette_tables: BTreeMap<String, serde_json::Value> = BTreeMap::new();

    for (cast_lib, cast_member, name) in &targets {
        let json = reserve_player_ref(|player| {
            mcp_get_cast_member_picture(player, *cast_lib, *cast_member)
        });
        let parsed: serde_json::Value = match serde_json::from_str(&json) {
            Ok(value) => value,
            Err(error) => {
                decode_failures.push(format!("{}: invalid MCP JSON ({})", name, error));
                continue;
            }
        };

        let bytes = match decode_png_bytes(&parsed) {
            Ok(bytes) => bytes,
            Err(error) => {
                decode_failures.push(format!("{}: {}", name, error));
                continue;
            }
        };
        let filename = format!("{}.png", name);
        fs::write(format!("{}/{}", png_dir(profile), filename), bytes)
            .expect("write furniture png");
        pngs_written += 1;
        emitted_names.push(name.clone());

        collect_palette_table(&mut palette_tables, &parsed);
        members_meta.push(serde_json::json!({
            "name": name,
            "filename": filename,
            "sourceCct": profile.source_cct,
            "castLib": cast_lib,
            "castMember": cast_member,
            "regX": parsed.get("reg_x"),
            "regY": parsed.get("reg_y"),
            "bitDepth": parsed.get("bit_depth"),
            "originalBitDepth": parsed.get("original_bit_depth"),
            "useAlpha": parsed.get("use_alpha"),
            "paletteRef": parsed.get("palette_ref"),
            "paletteIndexed": parsed.get("palette_indexed"),
            "width": parsed.get("width"),
            "height": parsed.get("height"),
        }));
    }
    decode_failures.sort();

    let meta_path = format!("{}/{}", output_dir(profile), profile.members_sidecar);
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&members_meta).expect("serialize furniture members sidecar"),
    )
    .expect("write furniture members sidecar");

    let palettes_path = format!("{}/_palettes.json", output_dir(profile));
    fs::write(
        &palettes_path,
        palettes_manifest_json(profile.dumper_name, &palette_tables),
    )
    .expect("write _palettes.json");

    println!(
        "  emitted {} member entries, {} PNGs -> {} ({} decode failures)",
        members_meta.len(),
        pngs_written,
        png_dir(profile),
        decode_failures.len(),
    );
    println!("    sidecar -> {}", meta_path);
    println!(
        "    palettes -> {} ({} distinct CLUTs)",
        palettes_path,
        palette_tables.len()
    );

    FurnitureBitmapDumpReport {
        named_bitmap_members,
        palette_members,
        duplicate_names,
        empty_named_bitmaps: empty_bitmaps.len(),
        pngs_written,
        member_entries: members_meta.len(),
        emitted_names,
        folded_name_collisions,
        decode_failures,
    }
}

fn decode_png_bytes(parsed: &serde_json::Value) -> Result<Vec<u8>, String> {
    use base64::Engine;

    if let Some(error) = parsed.get("error").and_then(|value| value.as_str()) {
        return Err(format!("cast bitmap export failed ({})", error));
    }
    let encoded = parsed
        .get("png_base64")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "MCP response has no png_base64 string".to_string())?;
    base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|error| format!("invalid PNG base64 ({})", error))
}
