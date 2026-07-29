//! Native integration test: dump every named bitmap member of
//! `cc_items[1].cct` as PNG plus a regPoint metadata sidecar, and export a
//! frame manifest for every named filmLoop member.
//!
//! `cc_items` is the wall-item cast: the `leftwall <base>` / `rightwall <base>`
//! members behind Furni's `WALL_ITEM_SPRITES`. Unlike `cc_furniture`, it is
//! NOT a flat bitmap cast — the 41 wall items split three ways:
//!
//!   * 23 pairs are plain bitmap members and dump directly here;
//!   * 9 pairs are filmLoops over component bitmaps that also live in this
//!     cast (`mountedfish_1..6`, `sharkporthole_asset_left_0..6`, `torch_1..3`,
//!     `leftwall web alpha`, …), so the components come out of the bitmap pass
//!     and the composition order comes out of the filmLoop manifest;
//!   * 6 pairs are Flash members (`dynamic`, `divamarquee`, `playamarquee`,
//!     `whiteboard`, `basketgoal`, `banner`) and are NOT exportable as
//!     pixels — they are runtime-parameterised (`ACTION_unilogo` picks a frame
//!     by `#logoID`; `ACTION_whiteboard` renders user text inside the SWF).
//!     They are counted in the summary and skipped.
//!
//! Sibling of `dump_furniture_bitmaps.rs`. Same TestPlayer +
//! `mcp_get_cast_member_picture` toolchain, plus `mcp_get_film_loop_frames`
//! from `dump_cct_bitmaps.rs`. PNGs are raw cast pixels — no ink keying (per
//! the mcp.rs rule, ink is a per-sprite property; the white chroma-key for
//! ink 8 is applied downstream, cf. `WallItem.ls:36-41` in the Oct 8 2007
//! client).
//!
//! Member names in this cast contain spaces, so filenames replace each run of
//! non-`[A-Za-z0-9._-]` characters with a single `_`. The sidecar carries both
//! the original `name` and the emitted `filename`; consumers must match on
//! `name`, never on the filename.
//!
//! Run:
//!   CASTS_ROOT=/abs/path/upstream/cokemusic-casts/client2 \
//!   OUTPUT_ROOT=/abs/path/packages/client/public/assets \
//!   cargo test -p vm-rust --test dump_wall_item_bitmaps -- --nocapture
//!
//! Output:
//!   - <OUTPUT_ROOT>/wallitems/data/<filename>.png
//!     One PNG per named bitmap member, first-occurrence (lowest cast member
//!     number) wins on name collisions — matches Director's first-match name
//!     lookup.
//!   - <OUTPUT_ROOT>/wallitems/filmloops/<filename>_frames.json
//!     One frame manifest per named filmLoop member, same shape as the
//!     per-room manifests dump_cct_bitmaps.rs writes.
//!   - <OUTPUT_ROOT>/wallitems/_cc_items_members.json
//!     One entry per named bitmap member: name, filename, sourceCct, castLib,
//!     castMember, regX, regY, bitDepth, originalBitDepth, useAlpha,
//!     paletteRef, width, height.
//!   - <OUTPUT_ROOT>/wallitems/_cc_items_skips.json
//!     Every named member NOT emitted as a PNG, with a reason: `flashMember`,
//!     `declaredZeroBitmap`, `filmLoopExportError`, `decodeFailure`. Written
//!     unconditionally so an empty skip list is a positive assertion rather
//!     than an absent file.

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::bitmap::bitmap::PaletteRef;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::{CastMember, CastMemberType};
use vm_rust::player::mcp::{
    mcp_get_cast_member_picture, mcp_get_cast_member_picture_with_palette, mcp_get_film_loop_frames,
};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

const CCT_NAME: &str = "cc_items[1].cct";

/// Cast number this dumper assigns to `cc_items` once loaded standalone.
const ITEMS_CAST: i32 = 1;
/// Cast number of the synthetic palette-only cast built from PALETTE_SOURCE_CCT.
const PALETTE_CAST: i32 = 2;

/// Two members in `cc_items` — the `arcade_sign` pair — carry a `paletteRef`
/// whose `cast_lib` is 0, i.e. a palette that does not live in this cast. A
/// standalone `.cct` load has no movie cast table to resolve that through, so
/// dirplayer falls back to the default system palette and the pair renders in
/// pure web-safe colours (`(0,85,0)`, `(102,153,0)`, `(204,153,204)` …) that no
/// artist ever picked.
///
/// The referenced member number (1153) is a palette named "DJ-2 Palette" in
/// `cc_furniture[1].cct`. This dumper therefore loads that cast first, harvests
/// its palette members into a synthetic cast, and re-renders any bitmap whose
/// paletteRef points outside `cc_items` against it. The sidecar records
/// `externalPalette` for every member rendered this way, so the substitution is
/// never silent.
const PALETTE_SOURCE_CCT: &str = "cc_furniture[1].cct";

fn casts_root() -> String {
    std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string())
}
fn wallitem_output_dir() -> String {
    let root = std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string());
    format!("{}/wallitems", root)
}
fn wallitem_png_dir() -> String {
    format!("{}/data", wallitem_output_dir())
}
fn wallitem_filmloop_dir() -> String {
    format!("{}/filmloops", wallitem_output_dir())
}

/// Director member names in this cast contain spaces and are used verbatim as
/// lookup keys; the filesystem is not. Collapse every run of unsafe characters
/// to a single `_` so the mapping is stable and readable, and keep the original
/// in the sidecar.
fn safe_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_was_sep = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
            last_was_sep = false;
        } else if !last_was_sep {
            out.push('_');
            last_was_sep = true;
        }
    }
    out.trim_matches('_').to_string()
}

/// Rebuild `cast_manager.casts` from the movie file just loaded. `.cct` files
/// have no cast table, so the cast libs have to be synthesized from
/// `dir.casts` — same dance as `dump_furniture_bitmaps.rs`.
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

/// The palette member a bitmap points at, when that palette lives outside
/// `cc_items` and so cannot resolve from a standalone `.cct` load.
fn external_palette_member(cast_lib: i32, cast_member: i32) -> Option<i32> {
    reserve_player_ref(|player| {
        let cast = player.movie.cast_manager.get_cast(cast_lib as u32).ok()?;
        let bitmap_member = cast.members.get(&(cast_member as u32))?.member_type.as_bitmap()?;
        let bitmap = player.bitmap_manager.get_bitmap(bitmap_member.image_ref)?;
        if !bitmap.has_palette() {
            return None;
        }
        let pal_ref = match &bitmap.palette_ref {
            PaletteRef::Member(r) => r.clone(),
            _ => return None,
        };
        if pal_ref.cast_lib == ITEMS_CAST {
            return None;
        }
        // Only claim a resolution when the harvested cast actually has that
        // member number — otherwise leave the fallback render in place and let
        // the sidecar show an unresolved paletteRef.
        let palette_cast = player.movie.cast_manager.get_cast(PALETTE_CAST as u32).ok()?;
        palette_cast.members.get(&(pal_ref.cast_member as u32))?;
        Some(pal_ref.cast_member)
    })
}

#[test]
fn dump_wall_item_cct_bitmaps() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    fs::create_dir_all(wallitem_output_dir()).expect("create wallitems assets dir");
    fs::create_dir_all(wallitem_png_dir()).expect("create wallitems data dir");
    fs::create_dir_all(wallitem_filmloop_dir()).expect("create wallitems filmloops dir");

    let cct_path = format!("{}/{}", casts_root(), CCT_NAME);
    if !PathBuf::from(&cct_path).exists() {
        panic!("cct not found at {} — set CASTS_ROOT", cct_path);
    }
    let palette_source_path = format!("{}/{}", casts_root(), PALETTE_SOURCE_CCT);
    if !PathBuf::from(&palette_source_path).exists() {
        panic!(
            "palette source cct not found at {} — set CASTS_ROOT",
            palette_source_path
        );
    }

    let mut player = TestPlayer::new();
    let mut summary: Vec<String> = Vec::new();
    let mut members_meta: Vec<serde_json::Value> = Vec::new();
    let mut skips: Vec<serde_json::Value> = Vec::new();

    // Pass 1 — harvest the external palettes. `PaletteMember` owns its colours
    // outright (no `bitmap_manager` handle), so the harvested members stay valid
    // after the second `load_movie` replaces the cast manager.
    player.load_movie(&palette_source_path).await;
    synthesize_casts_from_loaded_movie();
    let external_palettes: Vec<(u32, CastMember)> = reserve_player_ref(|player| {
        let mut out = Vec::new();
        for cast in player.movie.cast_manager.casts.iter() {
            for (member_num, member) in cast.members.iter() {
                if matches!(member.member_type, CastMemberType::Palette(_)) {
                    out.push((*member_num, member.clone()));
                }
            }
        }
        out
    });
    summary.push(format!(
        "  harvested {} palette members from {}",
        external_palettes.len(),
        PALETTE_SOURCE_CCT,
    ));

    // Pass 2 — load cc_items as cast 1, then append the harvested palettes as
    // cast 2 so out-of-cast paletteRefs have somewhere to resolve.
    player.load_movie(&cct_path).await;
    synthesize_casts_from_loaded_movie();
    reserve_player_mut(|player| {
        let mut palette_cast = CastLib {
            name: "__external_palettes".to_string(),
            file_name: PALETTE_SOURCE_CCT.to_string(),
            number: PALETTE_CAST as u32,
            is_external: true,
            state: CastLibState::Loaded,
            lctx: None,
            members: FxHashMap::default(),
            scripts: FxHashMap::default(),
            preload_mode: 0u16,
            capital_x: false,
            dir_version: 0,
            palette_id_offset: 0,
        };
        for (member_num, member) in &external_palettes {
            palette_cast.members.insert(*member_num, member.clone());
        }
        player.movie.cast_manager.casts.truncate(1);
        player.movie.cast_manager.casts.push(palette_cast);
    });

    // Collect named bitmaps, filmLoops and Flash members in one pass.
    // `cast.members` is a HashMap, so collect everything first and sort by
    // (castLib, castMember) before deduping — otherwise the collision winner
    // is non-deterministic (cf. the same fix in dump_cct_bitmaps.rs).
    let mut all_bitmaps: Vec<(i32, i32, String)> = Vec::new();
    let mut all_film_loops: Vec<(i32, i32, String)> = Vec::new();
    let mut all_flash: Vec<(i32, i32, String)> = Vec::new();
    let mut total_bitmaps = 0usize;
    let mut unnamed_bitmaps = 0usize;

    reserve_player_ref(|player| {
        for cast in player.movie.cast_manager.casts.iter() {
            for (member_num, member) in cast.members.iter() {
                let entry = (cast.number as i32, *member_num as i32, member.name.clone());
                match member.member_type {
                    CastMemberType::Bitmap(_) => {
                        total_bitmaps += 1;
                        if member.name.is_empty() {
                            unnamed_bitmaps += 1;
                            continue;
                        }
                        all_bitmaps.push(entry);
                    }
                    CastMemberType::FilmLoop(_) => {
                        if !member.name.is_empty() {
                            all_film_loops.push(entry);
                        }
                    }
                    CastMemberType::Flash(_) => {
                        if !member.name.is_empty() {
                            all_flash.push(entry);
                        }
                    }
                    _ => {}
                }
            }
        }
    });
    all_bitmaps.sort_by_key(|(cl, cm, _)| (*cl, *cm));
    all_film_loops.sort_by_key(|(cl, cm, _)| (*cl, *cm));
    all_flash.sort_by_key(|(cl, cm, _)| (*cl, *cm));

    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut targets: Vec<(i32, i32, String)> = Vec::new();
    let mut duplicate_bitmaps = 0usize;
    for (cl, cm, name) in all_bitmaps {
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

    let mut pngs_written = 0usize;
    let mut external_palette_renders = 0usize;
    for (cl, cm, name) in &targets {
        let external_palette = external_palette_member(*cl, *cm);
        let json = match external_palette {
            Some(palette_cm) => {
                external_palette_renders += 1;
                reserve_player_ref(|player| {
                    mcp_get_cast_member_picture_with_palette(player, *cl, *cm, PALETTE_CAST, palette_cm)
                })
            }
            None => reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm)),
        };
        let parsed: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(_) => {
                skips.push(skip_entry(*cl, *cm, name, "decodeFailure", Some("mcp response was not JSON")));
                continue;
            }
        };

        let filename = format!("{}.png", safe_filename(name));
        match decode_png_bytes(&parsed) {
            DecodeOutcome::Png(bytes) => {
                let png_path = format!("{}/{}", wallitem_png_dir(), filename);
                fs::write(&png_path, &bytes).expect("write wall item png");
                pngs_written += 1;
            }
            DecodeOutcome::DeclaredZero(error) => {
                skips.push(skip_entry(*cl, *cm, name, "declaredZeroBitmap", Some(&error)));
                continue;
            }
            DecodeOutcome::Missing => {
                skips.push(skip_entry(*cl, *cm, name, "decodeFailure", None));
                continue;
            }
        }

        let mut entry = serde_json::json!({
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
        });
        if let (Some(palette_cm), Some(obj)) = (external_palette, entry.as_object_mut()) {
            obj.insert(
                "externalPalette".into(),
                serde_json::json!({
                    "sourceCct": PALETTE_SOURCE_CCT,
                    "castMember": palette_cm,
                }),
            );
        }
        members_meta.push(entry);
    }

    summary.push(format!(
        "  emitted {} member entries, {} PNGs ({} via an external palette) → {}",
        members_meta.len(),
        pngs_written,
        external_palette_renders,
        wallitem_png_dir(),
    ));

    // filmLoop frame manifests — the composition order for the 9 animated
    // wall-item pairs. Components are bitmaps in this same cast and already
    // came out of the pass above.
    let mut film_loops_written = 0usize;
    for (cl, cm, name) in &all_film_loops {
        let json = reserve_player_ref(|player| mcp_get_film_loop_frames(player, *cl, *cm));
        // Skip writing on handler errors (mcp_error returns `{"error": "..."}`);
        // real manifests always include "frame_count".
        if !json.contains("\"frame_count\"") {
            let reason = serde_json::from_str::<serde_json::Value>(&json)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                .unwrap_or_else(|| json.lines().next().unwrap_or("(empty)").to_string());
            skips.push(skip_entry(*cl, *cm, name, "filmLoopExportError", Some(&reason)));
            continue;
        }
        let out_path = format!("{}/{}_frames.json", wallitem_filmloop_dir(), safe_filename(name));
        fs::write(&out_path, json).expect("write filmLoop frames manifest");
        film_loops_written += 1;
    }
    summary.push(format!(
        "  emitted {} filmLoop frame manifests → {}",
        film_loops_written,
        wallitem_filmloop_dir(),
    ));

    // Flash members carry no exportable pixels. Recording them here is the
    // point: it is the evidence that the 6 flash wall items cannot be ported
    // as static PNGs at all.
    for (cl, cm, name) in &all_flash {
        skips.push(skip_entry(*cl, *cm, name, "flashMember", None));
    }
    summary.push(format!("  {} Flash members recorded as unexportable", all_flash.len()));

    skips.sort_by(|a, b| {
        let key = |v: &serde_json::Value| {
            (
                v.get("reason").and_then(|r| r.as_str()).unwrap_or("").to_string(),
                v.get("castLib").and_then(|c| c.as_i64()).unwrap_or(0),
                v.get("castMember").and_then(|c| c.as_i64()).unwrap_or(0),
            )
        };
        key(a).cmp(&key(b))
    });

    let meta_path = format!("{}/_cc_items_members.json", wallitem_output_dir());
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&members_meta).expect("serialize _cc_items_members.json"),
    )
    .expect("write _cc_items_members.json");
    summary.push(format!("    sidecar → {}", meta_path));

    let skips_path = format!("{}/_cc_items_skips.json", wallitem_output_dir());
    fs::write(
        &skips_path,
        serde_json::to_string_pretty(&skips).expect("serialize _cc_items_skips.json"),
    )
    .expect("write _cc_items_skips.json");
    summary.push(format!("    skips ({}) → {}", skips.len(), skips_path));

    println!();
    println!("=== Wall-item cct extraction summary ===");
    for line in &summary {
        println!("{}", line);
    }
}

fn skip_entry(cl: i32, cm: i32, name: &str, reason: &str, error: Option<&str>) -> serde_json::Value {
    let mut value = serde_json::json!({
        "sourceCct": CCT_NAME,
        "castLib": cl,
        "castMember": cm,
        "name": name,
        "reason": reason,
    });
    if let (Some(error), Some(obj)) = (error, value.as_object_mut()) {
        obj.insert("error".into(), serde_json::Value::String(error.to_string()));
    }
    value
}

enum DecodeOutcome {
    Png(Vec<u8>),
    /// Director member declared 0x0 — a real, benign cast condition, not a
    /// dumper failure. Same predicate as config/patches/dirplayer-declared-zero-bitmap.patch.
    DeclaredZero(String),
    Missing,
}

fn decode_png_bytes(parsed: &serde_json::Value) -> DecodeOutcome {
    use base64::Engine;
    if let Some(error) = parsed.get("error").and_then(|value| value.as_str()) {
        if error.contains("declared 0x0, loaded fallback 1x1")
            || error == "PNG encoding failed: Zero width not allowed"
            || error == "PNG encoding failed: Zero height not allowed"
        {
            return DecodeOutcome::DeclaredZero(error.to_string());
        }
        panic!("cast bitmap export failed: {}", error);
    }
    match parsed.get("png_base64").and_then(|v| v.as_str()) {
        Some(b64) => match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
            Ok(bytes) => DecodeOutcome::Png(bytes),
            Err(_) => DecodeOutcome::Missing,
        },
        None => DecodeOutcome::Missing,
    }
}
