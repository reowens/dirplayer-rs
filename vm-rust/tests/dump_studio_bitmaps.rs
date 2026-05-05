//! Native integration test: dump studio room background bitmaps from
//! `cc_studio.cct` to authoritative PNGs we can drop into Furni's
//! `packages/client/public/assets/rooms/<id>.png`.
//!
//! Sibling of `dump_cct_bitmaps.rs.example`. Same toolchain (dirplayer-rs
//! TestPlayer + `mcp_get_cast_member_picture`), but pointed at one
//! cast-library cct (`cc_studio.cct`) that holds all 9 studio variants
//! as named bitmap members — instead of looping per-room cct files.
//!
//! Why this exists: studio rooms (Star Suite, Personal Suite,
//! studio_a..g) are NOT publicrooms. They live as cast members inside
//! `cc_studio.cct`, so the publicroom dumper (which iterates
//! `client2/publicrooms/*.cct`) skips them entirely. Without this test
//! the studio bg PNGs in `packages/client/public/assets/rooms/` are
//! whatever was hand-dropped from the wiki — wrong dimensions, wrong
//! padding (upstream Studio xml has `bgx=6 bgy=61` margins that wiki
//! crops don't respect), and no alpha channel.
//!
//! Member → room mapping is derived from
//! `upstream/cokemusic-casts/extracted/studio/cc_studio.json`:
//!   star_suite_model       (member 142) → star_suite
//!   personal_suite_model   (member 139) → personal_suite
//!   studio_model_a         (member 5)   → studio_a
//!   studio_model_b         (member 22)  → studio_b
//!   studio_model_c         (member 24)  → studio_c
//!   studio_model_d         (member 25)  → studio_d
//!   studio_model_e         (member 226) → studio_e
//!   studio_model_f         (member 228) → studio_f
//!   studio_model_g         (member 229) → studio_g
//!
//! Note: studio_e/f/g currently have no Furni room JSON (per
//! `docs/casts/EXTRACTION-PLAN.md` task #5 — "Translate the 7 base
//! studio templates" is still pending). The dumped PNGs land
//! anyway; later translator work will register them.
//!
//! Install:
//!   cp upstream/cokemusic-casts/dirplayer-patches/dump_studio_bitmaps.rs.example \
//!      ~/Development/packages/dirplayer-rs/vm-rust/tests/dump_studio_bitmaps.rs
//!
//! Run:
//!   cargo test -p vm-rust --test dump_studio_bitmaps -- --nocapture
//!
//! Output:
//!   - packages/client/public/assets/rooms/<room_id>.png  (the bg)
//!   - packages/client/public/assets/rooms/_studios.json  (regX/regY/width/height/bitDepth/useAlpha per studio)
//!   - /tmp/dirplayer_dumps/cc_studio/<cl>_<cm>_<safe_name>.png  (every bitmap, scratch)

#![cfg(not(target_arch = "wasm32"))]

use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::mcp::{mcp_get_cast_member_picture, mcp_get_cast_member_picture_with_palette};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

/// Path to cc_studio.cct. Computed from CASTS_ROOT (default `./casts`).
fn cc_studio_path() -> String {
    let root = std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string());
    format!("{}/cc_studio.cct", root)
}
/// Output dir for studio bg PNGs and the _studios.json /
/// _studio_members.json sidecars. Defaults to `./out/assets/rooms` under
/// OUTPUT_ROOT. Matches the convention used by `dump_cct_bitmaps` and
/// `dump_studio_palette_variants` so a single `OUTPUT_ROOT=<repo>/packages/client/public`
/// works across all three dumpers (per EXTRACTOR.md "Run the dumpers"
/// block). Prior versions of this dumper wrote to `<OUTPUT_ROOT>/rooms`
/// without the `assets/` prefix, which forced the bitmap dumper to use a
/// different OUTPUT_ROOT than the others.
fn rooms_output_dir() -> String {
    let root = std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string());
    format!("{}/assets/rooms", root)
}
const SCRATCH_DUMP_ROOT: &str = "/tmp/dirplayer_dumps/cc_studio";

/// Cast members whose bitmap header has no usable `palette_ref`;
/// dirplayer-rs renders rainbow-stripe garbage without an explicit
/// palette override. Discovered empirically 2026-05-04 by visual
/// inspection of `studio_personal_suite/wall_corner_1_a_0_3_0.png`
/// (a/c texture variants are broken; the matching b/d color back-fills
/// extract correctly without override).
///
/// The override palette here is the room's first-listed wall texture
/// palette — per upstream `Wall.ls displayPattern:175` and confirmed
/// via md5 match between the existing `studio_model_a` working wall
/// extractions and the `dump_studio_palette_variants.rs` output, the
/// canonical default is `right_wall_plates` / `left_wall_plates`.
///
/// Returns the palette member name to use as the override, or None if
/// the default extraction path should be used.
fn default_palette_override_for(member_name: &str) -> Option<&'static str> {
    match member_name {
        "wall_corner_1_a_0_3_0" => Some("right_wall_plates"),
        "wall_corner_1_c_0_3_0" => Some("left_wall_plates"),
        _ => None,
    }
}

/// Cast members in cc_studio.cct that are referenced unconditionally
/// from upstream Lingo (NOT from per-room SceneXml), so their per-room
/// `canonical.members` never lists them. Without explicit emission
/// here, the per-room dump loop skips them and runtime asset lookups
/// 404 silently.
///
/// Sources:
///   - `wall_doormask_1_a/b_0_2_0` — drawn unconditionally by every
///     studio's door per `Door.ls:82-83` (`oWall.drawWallTile` calls
///     OUTSIDE the layout `case` block). Required by all 9 studios.
///   - `studio.window.<city>.{1,2,3}` — drawn by `Window.ls drawWindow`
///     when SceneXml has a `<Window>` node. Studios A/B/C/D have
///     `<Window imageBase="london"/>` (verified 2026-05-04). E/F/G +
///     suites have no `<Window>` upstream — exclude.
const SHARED_DOORMASK_MEMBERS: &[&str] = &[
    "wall_doormask_1_a_0_2_0",
    "wall_doormask_1_b_0_2_0",
];
const SHARED_WINDOW_MEMBERS: &[&str] = &[
    "studio.window.london.1",
    "studio.window.london.2",
    "studio.window.london.3",
];

fn extra_shared_members_for(registry_id: &str) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = SHARED_DOORMASK_MEMBERS.to_vec();
    if matches!(
        registry_id,
        "studio_model_a" | "studio_model_b" | "studio_model_c" | "studio_model_d"
    ) {
        out.extend(SHARED_WINDOW_MEMBERS);
    }
    out
}

/// (cast member name in cc_studio.cct, furni room id).
/// Order matches the cc_studio.json declaration order so summary output
/// is stable across runs.
fn studio_mapping() -> Vec<(&'static str, &'static str)> {
    vec![
        ("studio_model_a", "studio_a"),
        ("studio_model_b", "studio_b"),
        ("studio_model_c", "studio_c"),
        ("studio_model_d", "studio_d"),
        ("studio_model_e", "studio_e"),
        ("studio_model_f", "studio_f"),
        ("studio_model_g", "studio_g"),
        ("personal_suite_model", "personal_suite"),
        ("star_suite_model", "star_suite"),
    ]
}

/// Build `studio_registry_id → [member_name, ...]` from each private-type
/// room JSON's `canonical.members` keys. Set ROOM_JSON_DIR to point at
/// `packages/common/src/data/rooms/`. If unset, returns empty (per-room
/// dump pass becomes a no-op).
///
/// Studio member names are SHARED across studios (e.g. `wall_corner_1_a_0_3_0`
/// appears in all 9), so the same physical PNG content gets written to
/// multiple studio dirs. That's intentional: matches the publicroom
/// `rooms/<room_id>/<member>.png` LoadingScene path so no runtime change
/// is needed.
///
/// JSON-stem → registry-id map mirrors `packages/common/src/data/rooms/index.ts`
/// (`studio_a.json` → `studio_model_a`, `star_suite.json` → `studio_star_suite`,
/// etc.). If a future studio JSON adds a new ID, both this map and the
/// index registry need updating.
fn read_canonical_studio_members() -> std::collections::HashMap<String, Vec<String>> {
    use std::collections::HashMap;
    use std::path::Path;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let rooms_dir = match std::env::var("ROOM_JSON_DIR") {
        Ok(v) => v,
        Err(_) => return out,
    };
    if !Path::new(&rooms_dir).exists() {
        return out;
    }
    for entry in fs::read_dir(&rooms_dir).unwrap_or_else(|_| panic!("read {}", rooms_dir)).flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") { continue; }
        let stem = match path.file_stem().and_then(|s| s.to_str()) { Some(s) => s.to_string(), None => continue };
        let txt = match fs::read_to_string(&path) { Ok(t) => t, Err(_) => continue };
        let v: serde_json::Value = match serde_json::from_str(&txt) { Ok(v) => v, Err(_) => continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("private") { continue; }
        let members = match v.pointer("/canonical/members").and_then(|m| m.as_object()) {
            Some(obj) => obj,
            None => continue,
        };
        let registry_id = match stem.as_str() {
            "studio_a" => "studio_model_a",
            "studio_b" => "studio_model_b",
            "studio_c" => "studio_model_c",
            "studio_d" => "studio_model_d",
            "studio_e" => "studio_model_e",
            "studio_f" => "studio_model_f",
            "studio_g" => "studio_model_g",
            "star_suite" => "studio_star_suite",
            "personal_suite" => "studio_personal_suite",
            _ => continue,
        };
        let names: Vec<String> = members.keys().cloned().collect();
        if !names.is_empty() {
            out.insert(registry_id.to_string(), names);
        }
    }
    out
}

#[test]
fn dump_studio_cct_bitmaps() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    fs::create_dir_all(rooms_output_dir()).expect("create assets dir");
    fs::create_dir_all(SCRATCH_DUMP_ROOT).expect("create scratch dir");

    let cc_studio = cc_studio_path();
    if !PathBuf::from(&cc_studio).exists() {
        panic!("cc_studio.cct not found at {}", cc_studio);
    }

    // Single load: cc_studio.cct holds all 9 studios as named bitmap
    // members. The publicroom dumper loads-once-per-cct in a loop;
    // here we load once and read 9 members from the same cast.
    let mut player = TestPlayer::new();
    player.load_movie(&cc_studio).await;

    // Mirror the publicroom dumper's CastLib synthesis: .cct files have
    // no cast_table (movies do), so we manually build a CastLib from
    // dir.casts[0] before any mcp_get_cast_member_picture call can
    // resolve members.
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

    // Index every bitmap member by name once, then look up each studio
    // target. Building a name→(cl, cm) map is faster + clearer than
    // re-scanning per studio, and lets us dump scratch copies of every
    // bitmap (avatar parts, doormasks, lamps, etc.) for inspection.
    let mut name_to_id: std::collections::HashMap<String, (i32, i32)> =
        std::collections::HashMap::new();
    // Palette members are indexed separately (not bitmap members) so the
    // 0.1a default-palette override path can resolve member names like
    // `right_wall_plates`. Same shape as dump_studio_palette_variants.rs.
    let mut palette_to_id: std::collections::HashMap<String, (i32, i32)> =
        std::collections::HashMap::new();
    let mut all_bitmaps: Vec<(i32, i32, String)> = Vec::new();
    reserve_player_ref(|player| {
        for cast in player.movie.cast_manager.casts.iter() {
            for (member_num, member) in cast.members.iter() {
                let cl = cast.number as i32;
                let cm = *member_num as i32;
                match &member.member_type {
                    CastMemberType::Bitmap(_) => {
                        let name = if member.name.is_empty() {
                            format!("member_{}", member_num)
                        } else {
                            member.name.clone()
                        };
                        if !member.name.is_empty() {
                            name_to_id.insert(member.name.clone(), (cl, cm));
                        }
                        all_bitmaps.push((cl, cm, name));
                    }
                    CastMemberType::Palette(_) => {
                        if !member.name.is_empty() {
                            palette_to_id.insert(member.name.clone(), (cl, cm));
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    let mut summary: Vec<String> = Vec::new();
    let mut studios_meta: Vec<serde_json::Value> = Vec::new();

    summary.push(format!(
        "  loaded cc_studio.cct → {} bitmap members indexed",
        all_bitmaps.len()
    ));

    // Dump each named studio bg into Furni's assets dir.
    for (member_name, room_id) in &studio_mapping() {
        let Some(&(cl, cm)) = name_to_id.get(*member_name) else {
            summary.push(format!(
                "  ✗ {:<22} → {:<16} member not found in cc_studio.cct",
                member_name, room_id
            ));
            continue;
        };

        let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, cl, cm));
        let parsed: serde_json::Value = match serde_json::from_str(&json) {
            Ok(v) => v,
            Err(e) => {
                summary.push(format!(
                    "  ✗ {:<22} → {:<16} json parse failed: {}",
                    member_name, room_id, e
                ));
                continue;
            }
        };

        let Some((bytes, w, h)) = decode_png(&json) else {
            let first_line = json.lines().next().unwrap_or("(empty)");
            summary.push(format!(
                "  ✗ {:<22} → {:<16} png decode failed: {}",
                member_name, room_id, first_line
            ));
            continue;
        };

        let out = format!("{}/{}.png", rooms_output_dir(), room_id);
        fs::write(&out, &bytes).expect("write studio bg png");

        // Capture metadata for the sidecar manifest. regX/regY are the
        // bitmap's intrinsic anchor in cast-coords — the Studio xml's
        // `<Background x="0" y="0">` is RELATIVE to that. Furni's
        // RoomDefinition `xAnchor/yAnchor` should be derived as:
        //   xAnchor = (Studio.Background.x) - regX  (if positive)
        //   yAnchor = (Studio.Background.y) - regY
        // …but in practice both are 0 in the upstream xml, so xAnchor
        // ends up = -regX and yAnchor = -regY. The translator (future
        // work, EXTRACTION-PLAN.md task #5) consumes this _studios.json.
        studios_meta.push(serde_json::json!({
            "roomId": room_id,
            "member": member_name,
            "castLib": cl,
            "castMember": cm,
            "regX": parsed.get("reg_x"),
            "regY": parsed.get("reg_y"),
            "bitDepth": parsed.get("bit_depth"),
            "useAlpha": parsed.get("use_alpha"),
            "width": parsed.get("width"),
            "height": parsed.get("height"),
        }));

        summary.push(format!(
            "  ✓ {:<22} → {:<16} {}x{} ({} bytes) regPoint=({:?},{:?}) bd={:?}",
            member_name,
            room_id,
            w,
            h,
            bytes.len(),
            parsed.get("reg_x"),
            parsed.get("reg_y"),
            parsed.get("bit_depth"),
        ));
    }

    // Per-room member dump: write each studio's `canonical.members` PNGs
    // into `<rooms_output_dir>/<registry_id>/<member>.png`, matching the
    // publicroom dump pattern so LoadingScene's `rooms/${roomId}/${file}`
    // resolves without a runtime change. Sourced from each studio JSON's
    // `canonical.members` keys (sorted in Step 1 above).
    let canonical_members_by_studio = read_canonical_studio_members();
    if !canonical_members_by_studio.is_empty() {
        // Loud failure if a studio JSON is missing or misnamed in the
        // stem→registry map. studio_mapping() has 9 entries; if we read
        // ROOM_JSON_DIR successfully we expect 9 studios.
        assert!(
            canonical_members_by_studio.len() == 9,
            "expected 9 studio JSONs in ROOM_JSON_DIR, got {} — check stem→registry map in read_canonical_studio_members()",
            canonical_members_by_studio.len()
        );
    }
    let mut per_room_summary: Vec<String> = Vec::new();
    let mut total_per_room_writes: usize = 0;
    let mut total_per_room_misses: usize = 0;

    // Stable iteration: sort by registry id so the summary lines are
    // deterministic across runs (HashMap iteration is not ordered).
    let mut sorted_studios: Vec<(&String, &Vec<String>)> = canonical_members_by_studio.iter().collect();
    sorted_studios.sort_by(|a, b| a.0.cmp(b.0));
    for (registry_id, member_names) in sorted_studios {
        let dest_dir = format!("{}/{}", rooms_output_dir(), registry_id);
        fs::create_dir_all(&dest_dir).expect("create per-studio assets dir");

        let mut dir_writes: usize = 0;
        let mut dir_misses: Vec<String> = Vec::new();

        // Build a single iteration covering canonical members PLUS the
        // unconditional-Lingo shared members. Dedup so a future overlap
        // (a shared member that ALSO appears in canonical.members) doesn't
        // double-write.
        let extras = extra_shared_members_for(registry_id);
        let mut to_emit: Vec<String> = member_names.clone();
        for name in &extras {
            if !to_emit.iter().any(|m| m == *name) {
                to_emit.push(name.to_string());
            }
        }

        for member_name in &to_emit {
            let Some(&(cl, cm)) = name_to_id.get(member_name) else {
                dir_misses.push(member_name.clone());
                continue;
            };
            // Phase 0.1a: for cast members with no usable palette_ref,
            // force an explicit palette override at extract time.
            // Otherwise dirplayer-rs renders rainbow garbage (e.g.
            // wall_corner_1_a/c_0_3_0 ship without a baked default palette).
            let json = if let Some(pal_name) = default_palette_override_for(member_name) {
                if let Some(&(pal_cl, pal_cm)) = palette_to_id.get(pal_name) {
                    reserve_player_ref(|player| {
                        mcp_get_cast_member_picture_with_palette(
                            player, cl, cm, pal_cl, pal_cm,
                        )
                    })
                } else {
                    // Override palette member missing from cct — fall
                    // back to default extraction (will produce rainbow,
                    // but at least preserves prior behavior).
                    dir_misses.push(format!("{} (override palette '{}' missing)", member_name, pal_name));
                    continue;
                }
            } else {
                reserve_player_ref(|player| mcp_get_cast_member_picture(player, cl, cm))
            };
            let Some((bytes, _w, _h)) = decode_png(&json) else {
                dir_misses.push(format!("{} (png decode failed)", member_name));
                continue;
            };
            let out = format!("{}/{}.png", dest_dir, member_name);
            fs::write(&out, &bytes).expect("write per-studio member png");
            dir_writes += 1;
        }
        total_per_room_writes += dir_writes;
        total_per_room_misses += dir_misses.len();
        per_room_summary.push(format!(
            "  ✓ {:<25} {} PNGs → {} (misses: {})",
            registry_id,
            dir_writes,
            dest_dir,
            if dir_misses.is_empty() { "none".to_string() } else { dir_misses.join(", ") },
        ));
    }

    if !canonical_members_by_studio.is_empty() {
        summary.push(format!(
            "  per-room member dump: {} studios, {} PNGs written, {} misses",
            canonical_members_by_studio.len(),
            total_per_room_writes,
            total_per_room_misses,
        ));
        summary.extend(per_room_summary);
    }

    // Sidecar manifest. The Furni-side translator that promotes wiki
    // bgs to cast-extracted bgs reads this to compute correct
    // xAnchor/yAnchor + sanity-check expected dimensions.
    if !studios_meta.is_empty() {
        let meta_path = format!("{}/_studios.json", rooms_output_dir());
        let meta_json = serde_json::to_string_pretty(&studios_meta).expect("serialize _studios.json");
        fs::write(&meta_path, meta_json).expect("write _studios.json");
        summary.push(format!(
            "    _studios.json: {} entries → {}",
            studios_meta.len(),
            meta_path
        ));
    }

    // Scratch dump: write every bitmap in cc_studio.cct under a
    // sortable filename so the studio-FG overlays (starsuite_lamps,
    // starsuite_doorframe_*, personal_suite_lamps, studio_exitshape,
    // etc.) are available for hand-inspection. Simultaneously capture
    // per-member metadata (regX/regY/width/height/bitDepth/useAlpha) for
    // EVERY bitmap into `_studio_members.json` at the assets root —
    // downstream Python (`translate_studio.py` or hand wiring of an
    // individual studio's `canonical.staticItems`) reads this to emit
    // the right `canonical.members{<name>: {regPoint:[x,y], ...}}` block
    // per Director's sprite-positioning convention. Studio FG sprites
    // have non-zero regPoints (cf. Tokyo's `tokyobar_a_0_0_0` reg=(70,
    // 200), `tokyo_doormask2` reg=(77,85)), so assuming (0,0) is wrong.
    let mut all_members_meta: Vec<serde_json::Value> = Vec::new();
    for (cl, cm, name) in &all_bitmaps {
        let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm));
        let parsed: Option<serde_json::Value> = serde_json::from_str(&json).ok();
        if let Some((bytes, _, _)) = decode_png(&json) {
            let safe = name
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect::<String>();
            let out = PathBuf::from(SCRATCH_DUMP_ROOT)
                .join(format!("{}_{}_{}.png", cl, cm, safe));
            fs::write(out, &bytes).ok();
        }
        if let Some(v) = parsed {
            if !name.is_empty() {
                all_members_meta.push(serde_json::json!({
                    "name": name,
                    "castLib": cl,
                    "castMember": cm,
                    "regX": v.get("reg_x"),
                    "regY": v.get("reg_y"),
                    "bitDepth": v.get("bit_depth"),
                    "useAlpha": v.get("use_alpha"),
                    "width": v.get("width"),
                    "height": v.get("height"),
                }));
            }
        }
    }
    if !all_members_meta.is_empty() {
        let path = format!("{}/_studio_members.json", rooms_output_dir());
        let body = serde_json::to_string_pretty(&all_members_meta).expect("serialize");
        fs::write(&path, body).expect("write _studio_members.json");
        summary.push(format!(
            "    _studio_members.json: {} entries → {}",
            all_members_meta.len(),
            path
        ));
    }
    summary.push(format!(
        "    scratch dump: {} bitmaps → {}",
        all_bitmaps.len(),
        SCRATCH_DUMP_ROOT
    ));

    println!();
    println!("=== Studio bg extraction summary ===");
    for line in &summary {
        println!("{}", line);
    }

    // Strict mode for the per-room member dump only: if any
    // canonical.members reference doesn't resolve to a cast member in
    // cc_studio.cct, fail the test loudly. The studio_mapping bg dump
    // above can still log `✗` lines silently — those misses are tolerated
    // (e.g. studio_e/f/g were dumpable before any Furni JSON existed).
    if total_per_room_misses > 0 {
        panic!(
            "studio per-room member dump: {} misses across {} studios — see summary above",
            total_per_room_misses,
            canonical_members_by_studio.len()
        );
    }
}

fn decode_png(json: &str) -> Option<(Vec<u8>, u64, u64)> {
    use base64::Engine;
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
