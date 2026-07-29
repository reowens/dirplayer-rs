//! Native integration test: dump per-(bitmap, palette) variant PNGs from
//! `cc_studio.cct` so Furni's runtime can present authentic Director CLUT
//! swaps for the per-occupant wall + floor pattern picker.
//!
//! Sibling of `dump_studio_bitmaps.rs` (player setup) and
//! `dump_cct_bitmaps.rs` (palette-cycle iteration pattern). Same toolchain
//! (TestPlayer + `mcp_get_cast_member_picture_with_palette`); pointed at
//! `cc_studio.cct`'s indexed wall + floor source bitmaps and the
//! `floor_*` / `<dir>_wall_*` palette CLUT members.
//!
//! Why this exists: upstream Coke Studios drives wall + floor pattern
//! customization via `sprite.member.palette = member(<palette_name>)`
//! (Floor.ls:184 + Wall.ls:175). Existing `dump_studio_bitmaps` emits
//! the source bitmaps in their default palette only — RGBA, palette
//! baked in, indexed structure lost. Runtime CLUT remap on those PNGs
//! is impossible. Furni's pattern picker has been wired to mute UI
//! since renderer ship (`7dc2253`) waiting for this dumper.
//!
//! Approach: pre-render each (indexed_bitmap, palette) pair offline.
//! `mcp_get_cast_member_picture_with_palette` already exists at
//! `vm-rust/src/player/mcp.rs:1059` and clones the bitmap with the
//! supplied palette ref before render — no shared mutation between
//! calls. Output: ~70-100 PNGs total (~1 MB), one per (bitmap, palette)
//! combo per Floor.ls / Wall.ls dispatch.
//!
//! Dispatch table (source: Floor.ls:184 + Wall.ls:47-54, 175):
//!   studiofloor_1          × floor_*       palettes  (15 floor patterns)
//!   studiofloor_door_1     × floor_*       palettes  (15)
//!   right_wall_1_a_0_2_0   × right_wall_*  palettes  (~20)
//!   left_wall_1_a_0_0_0    × left_wall_*   palettes  (~20)
//!   wall_corner_1_a_0_3_0  × right_wall_*  palettes  (right-corner texture overlay)
//!   wall_corner_1_c_0_3_0  × left_wall_*   palettes  (left-corner texture overlay)
//!   wall_doormask_1_a_0_2_0 × right_wall_* palettes  (over-door wallpaper texture)
//!
//! The over-door wallpaper (`wall_doormask_1_a`) is a `"right"`-dir wall
//! tile — `Door.ls:82-83` draws it via `oWall.drawWallTile(..., "right",
//! "wall_doormask", "texture", ...)`, so `Wall.ls:175 displayPattern`
//! swaps its palette to `right_wall_<palette>` exactly like every other
//! right-wall texture. Only the `_a` texture layer takes the CLUT swap;
//! the `_b` color layer is tinted via `sprite.color`/`blend`, no variant.
//!
//! Corners participate in the per-dir palette set per Wall.ls:47-54
//! (case-switch has only `right` / `left` branches; `bCorner` only
//! flips the WallScript sId4 flag, not the member-name lookup or the
//! palette dispatch). `cc_studio.json` confirms: there are no
//! `wall_corner_<palette>` palette members; corners share `<dir>_wall_*`.
//!
//! Run:
//!   cargo test -p vm-rust --test dump_studio_palette_variants -- --nocapture
//!
//! Output:
//!   - <OUTPUT_ROOT>/assets/rooms/_studio_palette_variants/<bitmap>__<palette>.png
//!   - <OUTPUT_ROOT>/assets/rooms/_studio_palette_variants/_studio_palette_variants.json
//!     (sidecar manifest for the LoadingScene preloader)

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::mcp::{
    collect_palette_table, mcp_get_cast_member_picture_with_palette, palettes_manifest_json,
};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

fn cc_studio_path() -> String {
    let root = std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string());
    format!("{}/cc_studio.cct", root)
}

fn variants_output_dir() -> String {
    let root = std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string());
    format!("{}/assets/rooms/_studio_palette_variants", root)
}

/// (indexed_bitmap_member_name, palette_name_prefix). Per Floor.ls:184
/// + Wall.ls:175. Order matters only for summary output stability.
const DISPATCH: &[(&str, &str)] = &[
    ("studiofloor_1",          "floor_"),
    ("studiofloor_door_1",     "floor_"),
    ("right_wall_1_a_0_2_0",   "right_wall_"),
    ("left_wall_1_a_0_0_0",    "left_wall_"),
    ("wall_corner_1_a_0_3_0",  "right_wall_"),
    ("wall_corner_1_c_0_3_0",  "left_wall_"),
    // Over-door wallpaper texture. A `"right"`-dir wall tile per
    // Door.ls:82-83 → shares the `right_wall_*` palette set. Only the
    // `_a` texture layer swaps palette; `_b` is the tinted color layer.
    ("wall_doormask_1_a_0_2_0", "right_wall_"),
];

/// Names whose `<prefix><suffix>` shape would false-match the dispatch
/// prefix but are not pattern-swap CLUTs (the `<prefix>_<palette>`
/// flat naming collides with `<prefix>_<bitmap-member-suffix>`).
/// Concretely: `floor_shape_a/b/c/d` are vector-shape members, not
/// floor pattern palettes — they share the `floor_` prefix but carry
/// no palette content of their own.
fn is_dispatchable_palette_name(name: &str, prefix: &str) -> bool {
    if !name.starts_with(prefix) {
        return false;
    }
    let suffix = &name[prefix.len()..];
    // Floor shapes: `floor_shape_<a|b|c|d>` — vector outline, not a CLUT.
    if prefix == "floor_" && suffix.starts_with("shape") {
        return false;
    }
    // Wall numeric variants: `<dir>_wall_1_a_0_<iDir>_0` etc. — these
    // are the source bitmaps themselves, not palette CLUTs. Filter any
    // suffix that starts with a digit to be safe.
    if suffix.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
        return false;
    }
    true
}

#[test]
fn dump_studio_palette_variants() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    fs::create_dir_all(variants_output_dir()).expect("create variants dir");

    let cc_studio = cc_studio_path();
    if !PathBuf::from(&cc_studio).exists() {
        panic!("cc_studio.cct not found at {}", cc_studio);
    }

    let mut player = TestPlayer::new();
    player.load_movie(&cc_studio).await;

    // CastLib synthesis (cct files have no cast_table). Same shape as
    // dump_studio_bitmaps.rs:172-196.
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

    // Index bitmaps + palettes by name.
    let mut bitmap_by_name: std::collections::HashMap<String, (i32, i32)> =
        std::collections::HashMap::new();
    let mut palette_by_name: std::collections::HashMap<String, (i32, i32)> =
        std::collections::HashMap::new();
    reserve_player_ref(|player| {
        for cast in player.movie.cast_manager.casts.iter() {
            for (member_num, member) in cast.members.iter() {
                if member.name.is_empty() { continue; }
                let cl = cast.number as i32;
                let cm = *member_num as i32;
                match &member.member_type {
                    CastMemberType::Bitmap(_) => {
                        bitmap_by_name.insert(member.name.clone(), (cl, cm));
                    }
                    CastMemberType::Palette(_) => {
                        palette_by_name.insert(member.name.clone(), (cl, cm));
                    }
                    _ => {}
                }
            }
        }
    });

    let mut summary: Vec<String> = Vec::new();
    // Deduplicated CLUTs for the overridden palettes each variant was rendered
    // through — this dumper's whole point is the substituted palette, so the
    // exported tables are the effective ones, not the members' declared refs.
    let mut palette_tables: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut manifest: Vec<(String, String, String)> = Vec::new();
    let mut total_writes: usize = 0;
    let mut total_misses: Vec<String> = Vec::new();
    let mut direct_color_skips: Vec<serde_json::Value> = Vec::new();

    summary.push(format!(
        "  loaded cc_studio.cct → {} bitmap members, {} palette members indexed",
        bitmap_by_name.len(),
        palette_by_name.len(),
    ));

    for (bitmap_name, palette_prefix) in DISPATCH {
        let Some(&(bitmap_cl, bitmap_cm)) = bitmap_by_name.get(*bitmap_name) else {
            total_misses.push(format!("bitmap {} missing", bitmap_name));
            summary.push(format!(
                "  ✗ {:<24} ← {:<13} bitmap not in cc_studio",
                bitmap_name, palette_prefix
            ));
            continue;
        };

        // Director CLUT swaps only affect indexed storage. 16/32-bit members
        // expose direct RGB pixels, so rendering them under another palette
        // would only duplicate identical PNGs.
        let is_indexed = reserve_player_ref(|player| -> Option<bool> {
            let cast = player.movie.cast_manager.get_cast(bitmap_cl as u32).ok()?;
            let member = cast.members.get(&(bitmap_cm as u32))?;
            let bm = member.member_type.as_bitmap()?;
            let bitmap = player.bitmap_manager.get_bitmap(bm.image_ref)?;
            Some(bitmap.has_palette())
        });
        let Some(is_indexed) = is_indexed else {
            total_misses.push(format!("bitmap {} metadata unavailable", bitmap_name));
            summary.push(format!(
                "  ✗ {:<24} ← {:<13} bitmap metadata unavailable",
                bitmap_name, palette_prefix
            ));
            continue;
        };
        if !is_indexed {
            summary.push(format!(
                "  · {:<24} ← {:<13} direct-color bitmap; CLUT swap is a no-op",
                bitmap_name, palette_prefix
            ));
            direct_color_skips.push(serde_json::json!({
                "bitmap": bitmap_name,
                "palettePrefix": palette_prefix,
                "reason": "directColorPaletteSwapIsNoOp",
            }));
            continue;
        }

        // Collect candidate palettes by prefix, sorted for stable output.
        let mut candidates: Vec<(String, i32, i32)> = palette_by_name
            .iter()
            .filter(|(name, _)| is_dispatchable_palette_name(name, palette_prefix))
            .map(|(name, &(cl, cm))| (name.clone(), cl, cm))
            .collect();
        candidates.sort_by(|a, b| a.0.cmp(&b.0));

        if candidates.is_empty() {
            summary.push(format!(
                "  ✗ {:<24} ← {:<13} no palette candidates",
                bitmap_name, palette_prefix
            ));
            continue;
        }

        let mut writes_for_bitmap = 0usize;
        for (palette_name, pal_cl, pal_cm) in &candidates {
            let json = reserve_player_ref(|player| {
                mcp_get_cast_member_picture_with_palette(
                    player, bitmap_cl, bitmap_cm, *pal_cl, *pal_cm,
                )
            });
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                collect_palette_table(&mut palette_tables, &v);
            }
            let Some((bytes, _w, _h)) = decode_png(&json) else {
                let first_line = json.lines().next().unwrap_or("(empty)");
                total_misses.push(format!(
                    "{}__{}: png decode failed ({})",
                    bitmap_name, palette_name, first_line
                ));
                continue;
            };
            let fname = format!("{}__{}.png", bitmap_name, palette_name);
            let path = format!("{}/{}", variants_output_dir(), fname);
            fs::write(&path, &bytes).expect("write variant png");
            manifest.push((bitmap_name.to_string(), palette_name.clone(), fname));
            writes_for_bitmap += 1;
        }
        total_writes += writes_for_bitmap;
        summary.push(format!(
            "  ✓ {:<24} ← {:<13} {} variants",
            bitmap_name, palette_prefix, writes_for_bitmap,
        ));
    }

    // Sidecar manifest. Sorted for diff-stable output.
    manifest.sort();
    let manifest_json = serde_json::json!({
        "variants": manifest.iter().map(|(b, p, f)| serde_json::json!({
            "bitmap": b,
            "palette": p,
            "file": f,
        })).collect::<Vec<_>>(),
        "skipped": direct_color_skips,
    });
    let manifest_path = format!(
        "{}/_studio_palette_variants.json",
        variants_output_dir()
    );
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest_json).expect("serialize manifest"),
    ).expect("write manifest");
    summary.push(format!(
        "    manifest: {} entries → {}",
        manifest.len(),
        manifest_path
    ));

    let palettes_path = format!("{}/_palettes.json", variants_output_dir());
    fs::write(
        &palettes_path,
        palettes_manifest_json("dump_studio_palette_variants", &palette_tables),
    )
    .expect("write _palettes.json");
    summary.push(format!(
        "    palettes: {} distinct CLUTs → {}",
        palette_tables.len(),
        palettes_path
    ));

    println!();
    println!("=== Studio palette variant extraction summary ===");
    for line in &summary {
        println!("{}", line);
    }
    println!("    total writes: {}", total_writes);
    if !total_misses.is_empty() {
        println!("    misses ({}):", total_misses.len());
        for m in &total_misses {
            println!("      - {}", m);
        }
    }

    // Strict mode: dispatch table failures are loud. Unlike the bg
    // dumper (which can tolerate `studio_e/f/g not yet in JSON`), every
    // dispatch entry here references a bitmap that ships extracted +
    // is referenced by canonical.walls/canonical.floor in studio JSONs.
    // Any miss is a real bug.
    if !total_misses.is_empty() {
        panic!(
            "studio palette variant dump: {} misses — see summary above",
            total_misses.len()
        );
    }
}

fn decode_png(json: &str) -> Option<(Vec<u8>, u64, u64)> {
    use base64::Engine;
    if let Some(error) = extract_str(json, "error") {
        panic!("cast bitmap export failed: {}", error);
    }
    let b64 = extract_str(json, "png_base64")?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .ok()?;
    let w = extract_num(json, "width")?;
    let h = extract_num(json, "height")?;
    Some((bytes, w, h))
}

fn extract_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":", key);
    let i = json.find(&needle)?;
    let mut s = i + needle.len();
    while json[s..].starts_with(|c: char| c.is_whitespace()) {
        s += 1;
    }
    if !json[s..].starts_with('"') {
        return None;
    }
    s += 1;
    let rest = &json[s..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn extract_num(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\":", key);
    let i = json.find(&needle)?;
    let mut s = i + needle.len();
    while json[s..].starts_with(|c: char| c.is_whitespace()) {
        s += 1;
    }
    let rest = &json[s..];
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse::<u64>().ok()
}
