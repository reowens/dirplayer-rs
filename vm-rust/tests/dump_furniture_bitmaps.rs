//! Native integration test: dump every named bitmap member of
//! `cc_furniture[1].cct` as PNG plus a regPoint metadata sidecar.
//!
//! Sibling of `dump_engine_bitmaps.rs`. Same TestPlayer +
//! `mcp_get_cast_member_picture` toolchain. PNGs are raw cast pixels —
//! no ink keying (per the mcp.rs rule, ink is a per-sprite property;
//! buildFurnitureAtlases.ts bakes the white chroma-key using the
//! `.ink` text members from extracted/engine/cc_furniture.json).
//!
//! Run:
//!   CASTS_ROOT=/abs/path/upstream/cokemusic-casts/client2 \
//!   OUTPUT_ROOT=/abs/path/packages/client/public/assets \
//!   cargo test -p vm-rust --test dump_furniture_bitmaps -- --nocapture
//!
//! Output:
//!   - <OUTPUT_ROOT>/furniture/data/<member_name>.png
//!     One PNG per named bitmap member, first-occurrence (lowest cast
//!     member number) wins on name collisions — matches Director's
//!     first-match name lookup. Stems follow the same
//!     `{base}_{layer}_{cell}_{w}_{h}_{dir}_{frame}` convention as the
//!     cokephase PNGs, because cokephase named its files after these
//!     same cast members.
//!   - <OUTPUT_ROOT>/furniture/_cc_furniture_members.json
//!     One entry per named bitmap member: name, filename, sourceCct,
//!     castLib, castMember, regX, regY, bitDepth, originalBitDepth,
//!     useAlpha, width, height. Consumers (buildFurnitureAtlases.ts)
//!     look up anchors by `name` matching the PNG stem.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::mcp::mcp_get_cast_member_picture;
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

const CCT_NAME: &str = "cc_furniture[1].cct";

fn casts_root() -> String {
    std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string())
}
fn furniture_output_dir() -> String {
    let root = std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string());
    format!("{}/furniture", root)
}
fn furniture_png_dir() -> String {
    format!("{}/data", furniture_output_dir())
}

#[test]
fn dump_furniture_cct_bitmaps() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    fs::create_dir_all(furniture_output_dir()).expect("create furniture assets dir");
    fs::create_dir_all(furniture_png_dir()).expect("create furniture data dir");

    let cct_path = format!("{}/{}", casts_root(), CCT_NAME);
    if !PathBuf::from(&cct_path).exists() {
        panic!("cct not found at {} — set CASTS_ROOT", cct_path);
    }

    let mut player = TestPlayer::new();
    let mut summary: Vec<String> = Vec::new();
    let mut members_meta: Vec<serde_json::Value> = Vec::new();

    // Load + synthesize cast (mirrors dump_studio_bitmaps.rs / dump_engine_bitmaps.rs).
    player.load_movie(&cct_path).await;
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

    // Collect every named bitmap. Members keep first-occurrence registration
    // (matches Director's lowest-cm name resolution); duplicates are skipped.
    // `cast.members` is a HashMap, so collect everything first and sort by
    // (castLib, castMember) before deduping — otherwise the collision winner
    // is non-deterministic (cf. the same fix in dump_cct_bitmaps.rs).
    let mut all_named: Vec<(i32, i32, String)> = Vec::new();
    let mut total_bitmaps = 0usize;
    let mut unnamed_bitmaps = 0usize;

    reserve_player_ref(|player| {
        for cast in player.movie.cast_manager.casts.iter() {
            for (member_num, member) in cast.members.iter() {
                if matches!(member.member_type, CastMemberType::Bitmap(_)) {
                    total_bitmaps += 1;
                    if member.name.is_empty() {
                        unnamed_bitmaps += 1;
                        continue;
                    }
                    all_named.push((cast.number as i32, *member_num as i32, member.name.clone()));
                }
            }
        }
    });
    all_named.sort_by_key(|(cl, cm, _)| (*cl, *cm));

    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut targets: Vec<(i32, i32, String)> = Vec::new();
    let mut duplicate_bitmaps = 0usize;
    for (cl, cm, name) in all_named {
        if seen_names.insert(name.clone()) {
            targets.push((cl, cm, name));
        } else {
            duplicate_bitmaps += 1;
        }
    }

    // Sort by name for deterministic JSON output (diff-friendly).
    targets.sort_by(|a, b| a.2.cmp(&b.2));

    summary.push(format!(
        "  loaded {} → {} bitmap members ({} named, {} unnamed, {} duplicate names skipped)",
        CCT_NAME,
        total_bitmaps,
        targets.len(),
        unnamed_bitmaps,
        duplicate_bitmaps,
    ));

    let mut decode_failures = 0usize;
    let mut pngs_written = 0usize;
    for (cl, cm, name) in &targets {
        let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm));
        let parsed: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(_) => {
                decode_failures += 1;
                continue;
            }
        };

        let filename = format!("{}.png", name);
        if let Some(bytes) = decode_png_bytes(&parsed) {
            let png_path = format!("{}/{}", furniture_png_dir(), filename);
            fs::write(&png_path, &bytes).expect("write furniture png");
            pngs_written += 1;
        }

        members_meta.push(serde_json::json!({
            "name": name,
            "filename": filename,
            "sourceCct": CCT_NAME,
            "castLib": cl,
            "castMember": cm,
            "regX": parsed.get("reg_x"),
            "regY": parsed.get("reg_y"),
            "bitDepth": parsed.get("bit_depth"),
            "originalBitDepth": parsed.get("original_bit_depth"),
            "useAlpha": parsed.get("use_alpha"),
            "paletteRef": parsed.get("palette_ref"),
            "width": parsed.get("width"),
            "height": parsed.get("height"),
        }));
    }

    summary.push(format!(
        "  emitted {} member entries, {} PNGs → {} ({} decode failures)",
        members_meta.len(),
        pngs_written,
        furniture_png_dir(),
        decode_failures,
    ));

    let meta_path = format!("{}/_cc_furniture_members.json", furniture_output_dir());
    let meta_json =
        serde_json::to_string_pretty(&members_meta).expect("serialize _cc_furniture_members.json");
    fs::write(&meta_path, meta_json).expect("write _cc_furniture_members.json");
    summary.push(format!("    sidecar → {}", meta_path));

    println!();
    println!("=== Furniture cct extraction summary ===");
    for line in &summary {
        println!("{}", line);
    }
}

fn decode_png_bytes(parsed: &serde_json::Value) -> Option<Vec<u8>> {
    use base64::Engine;
    if let Some(error) = parsed.get("error").and_then(|value| value.as_str()) {
        panic!("cast bitmap export failed: {}", error);
    }
    let b64 = parsed.get("png_base64")?.as_str()?;
    base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).ok()
}
