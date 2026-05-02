//! Native integration test: dump every bitmap cast member from each
//! Coke Music publicroom .cct file to authoritative PNGs we can drop
//! into Furni's `packages/client/public/assets/rooms/<id>.png`.
//!
//! Reads the room mapping from FURNI_DUMP_CONFIG env var (path to a
//! TSV: `<cct_basename>\t<furni_room_id>` lines), or falls back to a
//! hardcoded Tokyo run.
//!
//! Each cct is loaded as a standalone cast (movies have a cast_table,
//! .cct files don't, so we synthesize a CastLib from dir.casts[0]).
//! Then for each member we call mcp_get_cast_member_picture and write
//! the resulting PNG.
//!
//! Run:
//!   cargo test -p vm-rust --test dump_cct_bitmaps -- --nocapture

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::bitmap::bitmap::PaletteRef;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::mcp::{
    mcp_get_cast_member_picture, mcp_get_cast_member_picture_with_palette,
    mcp_get_film_loop_frames,
};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

/// Input cct mirror — set CASTS_ROOT to the directory containing client2/ contents
/// (must contain a `publicrooms/` subdir with the per-room .cct files).
fn casts_root() -> String {
    std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string())
}
fn publicrooms_dir() -> String {
    format!("{}/publicrooms", casts_root())
}
/// Output root for the rendered PNGs + _members.json sidecars.
fn output_root() -> String {
    std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string())
}
fn rooms_output_dir() -> String {
    format!("{}/rooms", output_root())
}
const PER_ROOM_DUMP_ROOT: &str = "/tmp/dirplayer_dumps";

/// (cct basename, furni room id, bg cast member name) — bg name comes from
/// the room's canonical.roomBitmaps[id=floor].member field, which the
/// translator emits from the upstream room.room block.
fn room_mapping() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("alaska",            "alaska",            "reykjavik_bg"),
        ("atlantis",          "neptune",           "atlantis_background"),
        ("auditiongold",      "audition_gold",     "audition_gold_8_bit"),
        ("auditionred",       "casting_call_red",  "audition_red_8_bit"),
        ("castingcallroom",   "casting_call_blue", "casting-call-room-bg"),
        ("centralpark",       "central_park",      "backgroundflatspeakerwire"),
        ("clubcherry",        "club_cherry",       "cherry_v12_15-bit_"),
        ("goa",               "goa",               "goa-cut"),
        ("london",            "london",            "cc.london.bg"),
        ("mexico",            "mexico",            "mexicobg"),
        ("miami",             "miami",             "miami_bass"),
        ("mombasa",           "mombasa",           "mombassa991278"),
        ("moscow",            "moscow",            "mos_cow_split"),
        ("ncaa",              "mycoke_arena",      "arena_2"),
        ("neworleans",        "new_orleans",       "neworleans_background_8bit"),
        ("newyork",           "new_york",          "ny bg"),
        ("raysrooftopparty",  "rays_rooftop",      "roof_top_256"),
        ("redroom",           "red_room",          "redroom_bg"),
        ("rio",               "rio",               "rio.background"),
        ("sanfrancisco",      "san_francisco",     "sanfrancisco_background"),
        ("seattle",           "seattle",           "roof_garden"),
        ("sydney",            "sydney",            "sydney_background"),
        ("tokyo",             "tokyo",             "tokyo_bg"),
    ]
}

#[test]
fn dump_publicroom_cct_bitmaps() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    fs::create_dir_all(rooms_output_dir()).expect("create assets dir");
    fs::create_dir_all(PER_ROOM_DUMP_ROOT).expect("create dump root");

    // Single TestPlayer reused across all loads — load_movie just replaces
    // the loaded movie/cast.
    let mut player = TestPlayer::new();
    let mut summary: Vec<String> = Vec::new();

    // Read each room's canonical.roomBitmaps so we know which FG members
    // (door masks, bears, bardesks, etc.) to dump per room.
    let fg_members_by_room = read_canonical_fg_members();

    let rooms = room_mapping();
    for (cct_base, room_id, bg_name_override) in &rooms {
        let cct_path = format!("{}/{}.cct", publicrooms_dir(), cct_base);
        if !PathBuf::from(&cct_path).exists() {
            summary.push(format!("  ✗ {} → {}: cct not found at {}", cct_base, room_id, cct_path));
            continue;
        }

        // Per-room scratch dir for ALL bitmap members (debugging).
        let room_dump_dir = PathBuf::from(format!("{}/{}", PER_ROOM_DUMP_ROOT, room_id));
        fs::create_dir_all(&room_dump_dir).ok();

        // Load + synthesize cast.
        player.load_movie(&cct_path).await;
        reserve_player_mut(|player| {
            // Reset existing casts so the previous room's data doesn't linger.
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

        // Snapshot bitmap targets.
        let mut targets: Vec<(i32, i32, String)> = Vec::new();
        let mut film_loop_targets: Vec<(i32, i32, String)> = Vec::new();
        let mut bg_target: Option<(i32, i32, String)> = None;
        // Fall back to "<base>_bg" if no override was provided.
        let bg_name: String = if bg_name_override.is_empty() {
            format!("{}_bg", cct_base)
        } else {
            bg_name_override.to_string()
        };
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
                            if name == bg_name {
                                bg_target = Some((cl, cm, name.clone()));
                            }
                            targets.push((cl, cm, name));
                        }
                        CastMemberType::FilmLoop(_) => {
                            // FilmLoops have no PNG of their own — their
                            // frames are individual bitmap members already
                            // covered above. We capture them here so we can
                            // emit a `_frames.json` manifest the Furni
                            // translator + runtime can read.
                            if !member.name.is_empty() {
                                film_loop_targets.push((cl, cm, member.name.clone()));
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
        // `cast.members` is a HashMap so iteration order is non-deterministic.
        // The collision-disambiguation logic below assigns the BARE name to
        // whichever (cl, cm) is processed first — Director's first-match
        // cast lookup uses the LOWEST cast-member-number occurrence, so we
        // sort the same way before processing. Without this sort, e.g.
        // red_room's `redroomsofa2_a_0_1_1_2_0` would non-deterministically
        // give the bare PNG to either the cm=9 sprite (40×46 real sofa) or
        // the cm=17 sprite (2×1 placeholder). We want the lowest cm to win.
        targets.sort_by_key(|(cl, cm, _)| (*cl, *cm));
        film_loop_targets.sort_by_key(|(cl, cm, _)| (*cl, *cm));

        // Dump the bg specifically into Furni assets dir.
        if let Some((cl, cm, name)) = &bg_target {
            let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm));
            if let Some((bytes, w, h)) = decode_png(&json) {
                let out = format!("{}/{}.png", rooms_output_dir(), room_id);
                fs::write(&out, &bytes).expect("write bg png");
                summary.push(format!(
                    "  ✓ {:<22} → {:<22} bg {} {}x{} ({} bytes)",
                    cct_base, room_id, name, w, h, bytes.len()
                ));
            } else {
                summary.push(format!(
                    "  ✗ {:<22} → {:<22} bg dump failed: {}",
                    cct_base, room_id,
                    json.lines().next().unwrap_or("(empty)")
                ));
            }
        } else {
            // Fallback: dump every bitmap member, user can pick.
            summary.push(format!(
                "  ⚠ {:<22} → {:<22} no bitmap named '{}' — dumping all {} bitmaps to {}",
                cct_base, room_id, bg_name, targets.len(), room_dump_dir.display()
            ));
        }

        // Also dump every bitmap into per-room scratch dir for inspection,
        // AND into the Furni assets dir keyed by member name (so static
        // objects emitted by the translator from canonical.sceneFurniture
        // can resolve to `rooms/<id>/<member>.png`).
        //
        // Simultaneously accumulate per-member metadata (regPoint, bit_depth,
        // width/height, use_alpha) into a per-room `_members.json` file so
        // the Furni translator can emit `canonical.members` for runtime
        // anchor + ink-mode decisions.
        let fg_dest_dir = format!("{}/{}", rooms_output_dir(), room_id);
        fs::create_dir_all(&fg_dest_dir).ok();
        let mut members_meta: Vec<serde_json::Value> = Vec::new();
        // Track which <name>.png filenames we've already written in this
        // room to detect cast-member name collisions. When two cast members
        // share a name (rio has two `rio_wave1` at 183x93 and 205x106; rays
        // has multiple `car_256` variants used by different filmLoops), the
        // second write would silently overwrite the first. We disambiguate
        // by writing the second+ as `<name>__cast<castLib>_<castMember>.png`
        // and emit a separate _members.json entry whose key is the matching
        // composite. Translator + runtime use the composite key when a
        // filmLoop manifest specifies (cast_lib, cast_member) for that
        // child, so the right variant is rendered.
        let mut taken_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (cl, cm, name) in &targets {
            let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm));
            // Determine emit name + filename. First occurrence keeps the
            // bare name (matches Director's first-match name-lookup
            // semantics for canonical.roomBitmaps / canonical.staticItems
            // references which carry only a name). Collisions get a
            // `<name>#cl_cm` composite key and `<name>__castCL_CM.png`
            // file.
            let (emit_name, file_stem): (String, String) = if name.is_empty() {
                (String::new(), format!("__cast{}_{}", cl, cm))
            } else if taken_names.insert(name.clone()) {
                (name.clone(), name.clone())
            } else {
                (
                    format!("{}#{}_{}", name, cl, cm),
                    format!("{}__cast{}_{}", name, cl, cm),
                )
            };
            let safe = name
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
                .collect::<String>();
            if let Some((bytes, _, _)) = decode_png(&json) {
                // Per-room scratch dir (always disambiguated by cast pair).
                let scratch = room_dump_dir.join(format!("{}_{}_{}.png", cl, cm, safe));
                fs::write(scratch, &bytes).ok();
                // Furni assets dir (canonical PNG, possibly disambiguated).
                if !file_stem.is_empty() {
                    let asset_path = format!("{}/{}.png", fg_dest_dir, file_stem);
                    fs::write(asset_path, &bytes).ok();
                }
            }
            // Palette-cycle detection: for indexed bitmaps with a Member
            // palette, find sibling palettes in the same cast that share the
            // bitmap's default palette name with a numeric suffix
            // (`<group>_<n>`). If ≥3 siblings, this is a Director palette-
            // cycle fixture (Tokyo disco floor, London neon, Neptune disco
            // floor, secret room marquee, etc.). Render one PNG per palette
            // frame via mcp_get_cast_member_picture_with_palette and emit a
            // `paletteFrames` map on the member's metadata for the runtime
            // cycler to consume.
            //
            // Discovery is done in one `reserve_player_ref` to avoid holding
            // the read-lock during PNG dumping (each MCP call takes its own).
            let palette_cycle_info: Option<(String, i32, Vec<(i32, i32, i32)>)> =
                reserve_player_ref(|player| {
                    let cast = player.movie.cast_manager.get_cast(*cl as u32).ok()?;
                    let bitmap_member = cast.members.get(&(*cm as u32))?
                        .member_type.as_bitmap()?;
                    let bitmap = player.bitmap_manager.get_bitmap(bitmap_member.image_ref)?;
                    if bitmap.original_bit_depth > 8 { return None; }
                    let pal_ref = match &bitmap.palette_ref {
                        PaletteRef::Member(r) => r.clone(),
                        _ => return None,
                    };
                    // Resolve the palette member's name (palettes can live in
                    // any cast lib — use the ref's own cast_lib).
                    let pal_cast = player.movie.cast_manager
                        .get_cast(pal_ref.cast_lib as u32).ok()?;
                    let pal_member = pal_cast.members.get(&(pal_ref.cast_member as u32))?;
                    let pal_name = pal_member.name.clone();
                    if pal_name.is_empty() { return None; }
                    // Strip trailing digits to get group prefix + this
                    // bitmap's starting frame index. Director's palette
                    // naming conventions are mixed: some groups use
                    // `<prefix>_<digit>` (londonlights_0, peaceful_1) and
                    // others omit the underscore (GoalightPalette0,
                    // RrLitePal3). Walk back from the end to find the digit
                    // boundary so both forms detect cleanly.
                    let digit_start = pal_name
                        .char_indices()
                        .rev()
                        .take_while(|(_, c)| c.is_ascii_digit())
                        .last()
                        .map(|(i, _)| i)?;
                    if digit_start == 0 { return None; }
                    let group = &pal_name[..digit_start];
                    let default_frame: i32 = pal_name[digit_start..].parse().ok()?;
                    let group = group.to_string();
                    // Helper: collect palette siblings matching `<g><digits>`.
                    let collect = |g: &str| -> Vec<(i32, i32, i32)> {
                        let mut out: Vec<(i32, i32, i32)> = Vec::new();
                        for (sib_num, sib) in pal_cast.members.iter() {
                            if sib.member_type.as_palette().is_none() { continue; }
                            let n = &sib.name;
                            if !n.starts_with(g) { continue; }
                            let suffix = &n[g.len()..];
                            // Suffix must be entirely digits; otherwise we'd
                            // false-match e.g. group "Goalight" against name
                            // "GoalightWeird1" (suffix "Weird1").
                            if let Ok(frame_n) = suffix.parse::<i32>() {
                                out.push((frame_n, pal_ref.cast_lib, *sib_num as i32));
                            }
                        }
                        out
                    };
                    let mut siblings = collect(&group);
                    let mut chosen_group = group.clone();
                    let mut chosen_default_frame = default_frame;
                    if siblings.len() < 3 {
                        // Cousin-group fallback for room-specific PaletteAnimator
                        // subclasses that store the bitmap's idle palette in a
                        // single-frame group (e.g. `neptune_discofloor_peaceful_0`)
                        // but cycle through a sibling group during performance
                        // (`neptune_discofloor_action_0..21`).
                        // NeptuneFloorAnimator (Cast External BehaviorScript 50,
                        // atlantis cct) keys this off `aPatternNames` —
                        // we approximate it by walking back one `_` segment to
                        // get a parent prefix and finding the largest cousin
                        // group with ≥ 3 frames. Tokyo's primary group already
                        // has 16 frames so this fallback never fires for it.
                        let trimmed = group.trim_end_matches('_');
                        if let Some(last_us) = trimmed.rfind('_') {
                            let parent_prefix = &trimmed[..=last_us]; // includes trailing `_`
                            let mut cousin_groups: HashMap<String, Vec<(i32, i32, i32)>> = HashMap::new();
                            for (sib_num, sib) in pal_cast.members.iter() {
                                if sib.member_type.as_palette().is_none() { continue; }
                                let n = &sib.name;
                                if !n.starts_with(parent_prefix) { continue; }
                                let rest = &n[parent_prefix.len()..];
                                // rest = "action_5", "peaceful_0", etc.
                                if let Some(us) = rest.rfind('_') {
                                    if let Ok(frame_n) = rest[us+1..].parse::<i32>() {
                                        let cg = format!("{}{}_", parent_prefix, &rest[..us]);
                                        cousin_groups
                                            .entry(cg)
                                            .or_default()
                                            .push((frame_n, pal_ref.cast_lib, *sib_num as i32));
                                    }
                                }
                            }
                            if let Some((best_g, best_sibs)) = cousin_groups
                                .into_iter()
                                .filter(|(g, sibs)| g != &group && sibs.len() >= 3)
                                .max_by_key(|(_, sibs)| sibs.len())
                            {
                                chosen_group = best_g;
                                siblings = best_sibs;
                                siblings.sort_by_key(|t| t.0);
                                chosen_default_frame = siblings[0].0;
                            }
                        }
                    }
                    if siblings.len() < 3 { return None; }
                    siblings.sort_by_key(|t| t.0);
                    // Trim trailing `_` for cleaner group label (e.g.
                    // "londonlights_" → "londonlights"; "GoalightPalette"
                    // unchanged).
                    let group_label = chosen_group.trim_end_matches('_').to_string();
                    Some((group_label, chosen_default_frame, siblings))
                });

            let palette_frames_meta: Option<serde_json::Value> = palette_cycle_info
                .as_ref()
                .map(|(group, _default_frame, siblings)| {
                    // Render one PNG per palette frame and write to the
                    // assets dir. Filenames are `<file_stem>__pal_<n>.png`.
                    let mut frames_map = serde_json::Map::new();
                    for (frame_n, pal_cl, pal_cm) in siblings {
                        let frame_json = reserve_player_ref(|player| {
                            mcp_get_cast_member_picture_with_palette(
                                player, *cl, *cm, *pal_cl, *pal_cm,
                            )
                        });
                        if let Some((bytes, _, _)) = decode_png(&frame_json) {
                            let fname = format!("{}__pal_{}.png", file_stem, frame_n);
                            let path = format!("{}/{}", fg_dest_dir, fname);
                            fs::write(path, &bytes).ok();
                            frames_map.insert(
                                frame_n.to_string(),
                                serde_json::Value::String(fname),
                            );
                        }
                    }
                    summary.push(format!(
                        "    palette-cycle: {} → {} frames (group={}, start={})",
                        emit_name, siblings.len(), group, _default_frame
                    ));
                    serde_json::Value::Object(frames_map)
                });

            // Capture metadata regardless of PNG decode success — name lookup
            // and bitmap shape are useful even for empty/zero-byte members.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                if !emit_name.is_empty() {
                    let mut entry = serde_json::json!({
                        "name": name,
                        "key": emit_name,
                        "filename": format!("{}.png", file_stem),
                        "castLib": cl,
                        "castMember": cm,
                        "regX": v.get("reg_x"),
                        "regY": v.get("reg_y"),
                        "bitDepth": v.get("bit_depth"),
                        "originalBitDepth": v.get("original_bit_depth"),
                        "useAlpha": v.get("use_alpha"),
                        "width": v.get("width"),
                        "height": v.get("height"),
                    });
                    if let (Some(frames), Some((group, default_frame, _))) =
                        (palette_frames_meta, palette_cycle_info.as_ref())
                    {
                        let obj = entry.as_object_mut().unwrap();
                        obj.insert("paletteFrames".into(), frames);
                        obj.insert("paletteDefaultFrame".into(),
                                   serde_json::Value::from(*default_frame));
                        obj.insert("paletteGroup".into(),
                                   serde_json::Value::String(group.clone()));
                    }
                    members_meta.push(entry);
                }
            }
        }
        // Write per-room members metadata.
        if !members_meta.is_empty() {
            let meta_path = format!("{}/_members.json", fg_dest_dir);
            let meta_json = serde_json::to_string_pretty(&members_meta).expect("serialize members meta");
            fs::write(&meta_path, meta_json).expect("write _members.json");
            summary.push(format!("    members.json: {} entries → {}", members_meta.len(), meta_path));
        }

        // Emit a `<filmloop_name>_frames.json` manifest for every filmLoop
        // in this room's cct. The manifest holds per-frame member refs +
        // baked transforms (loc_h/loc_v/width/height/rotation/skew/blend/ink)
        // + per-frame duration_ms from the tempo channel, so the renderer
        // can drive a sprite ensemble per filmLoop without re-parsing the
        // cast or interpolating tweens client-side.
        let mut film_loops_written = 0usize;
        for (cl, cm, name) in &film_loop_targets {
            let json = reserve_player_ref(|player| mcp_get_film_loop_frames(player, *cl, *cm));
            // Skip writing on handler errors (mcp_error returns
            // `{"error": "..."}`); real manifests always include "frame_count".
            if !json.contains("\"frame_count\"") {
                summary.push(format!(
                    "    filmLoop {} skipped: {}",
                    name,
                    json.lines().next().unwrap_or("(empty)")
                ));
                continue;
            }
            let out_path = format!("{}/{}_frames.json", fg_dest_dir, name);
            fs::write(&out_path, json).expect("write filmLoop frames manifest");
            film_loops_written += 1;
        }
        if film_loops_written > 0 {
            summary.push(format!(
                "    filmLoop manifests: {} → {}/<name>_frames.json",
                film_loops_written, fg_dest_dir
            ));
        }

        // Sanity stat: how many of the canonical.roomBitmaps members we
        // expected actually got dumped (the all-bitmaps loop above writes
        // them by name; this just reports coverage).
        if let Some(fg_names) = fg_members_by_room.get(*room_id) {
            let target_names: std::collections::HashSet<&str> =
                targets.iter().map(|(_, _, n)| n.as_str()).collect();
            let dumped = fg_names.iter().filter(|n| target_names.contains(n.as_str())).count();
            let missing = fg_names.len() - dumped;
            if dumped > 0 || missing > 0 {
                summary.push(format!(
                    "    FG: {} / missing {} (of {} canonical roomBitmaps)",
                    dumped, missing, fg_names.len()
                ));
            }
        }
        // Also report how many sceneFurniture cast members are present.
        let scene_furn_dumped = targets
            .iter()
            .filter(|(_, _, n)| n.contains("_chair") || n.contains("_cokecase") || n.contains("_table"))
            .count();
        if scene_furn_dumped > 0 {
            summary.push(format!("    scene-furn-bitmaps available: {}", scene_furn_dumped));
        }
    }

    println!();
    println!("=== Bg extraction summary ({} rooms) ===", rooms.len());
    for line in &summary {
        println!("{}", line);
    }
}

fn decode_png(json: &str) -> Option<(Vec<u8>, u64, u64)> {
    use base64::Engine;
    let b64 = extract_str(json, "png_base64")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()).ok()?;
    let w = extract_num(json, "width")?;
    let h = extract_num(json, "height")?;
    Some((bytes, w, h))
}

fn extract_str(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\":", key);
    let i = json.find(&needle)?;
    let mut s = i + needle.len();
    while json[s..].starts_with(|c: char| c.is_whitespace()) { s += 1; }
    if !json[s..].starts_with('"') { return None; }
    s += 1;
    let rest = &json[s..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn extract_num(json: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{}\":", key);
    let i = json.find(&needle)?;
    let mut s = i + needle.len();
    while json[s..].starts_with(|c: char| c.is_whitespace()) { s += 1; }
    let rest = &json[s..];
    let end = rest.find(|c: char| !c.is_ascii_digit())?;
    rest[..end].parse::<u64>().ok()
}

/// Optionally read a downstream consumer's per-room JSON to learn which FG
/// cast members are referenced by `canonical.roomBitmaps` (excluding
/// id="floor"). Used to limit per-room output to relevant members.
///
/// Set `ROOM_JSON_DIR` to point at a directory of `<room_id>.json` files
/// matching the schema this dumper consumes (see SCHEMA.md). If unset, the
/// filter is skipped and every FG bitmap in each cct is dumped.
fn read_canonical_fg_members() -> HashMap<String, Vec<String>> {
    use std::path::Path;
    let mut out = HashMap::new();
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
        if v.get("type").and_then(|t| t.as_str()) != Some("public") { continue; }
        let bitmaps = match v.pointer("/canonical/roomBitmaps").and_then(|b| b.as_array()) {
            Some(arr) => arr,
            None => continue,
        };
        let mut names = Vec::new();
        for entry in bitmaps {
            if entry.get("id").and_then(|i| i.as_str()) == Some("floor") { continue; }
            if let Some(member) = entry.get("member").and_then(|m| m.as_str()) {
                names.push(member.to_string());
            }
        }
        if !names.is_empty() {
            out.insert(stem, names);
        }
    }
    out
}
