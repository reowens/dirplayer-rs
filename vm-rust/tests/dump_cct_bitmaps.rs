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
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::mcp::{mcp_get_cast_member_picture, mcp_get_film_loop_frames};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

const CASTS_ROOT: &str = "/Users/reoiv/Development/sandbox/decibel/upstream/cokemusic-casts/client2/publicrooms";
const FURNI_ASSETS: &str = "/Users/reoiv/Development/sandbox/decibel/packages/client/public/assets/rooms";
const PER_ROOM_DUMP_ROOT: &str = "/tmp/dirplayer_dumps";

/// (cct basename, furni room id, bg cast member name) — bg name comes from
/// the room's canonical.roomBitmaps[id=floor].member field, which the
/// translator emits from the upstream room.room block.
fn room_mapping() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("alaska",            "alaska",            "reykjavik_bg"),
        ("atlantis",          "neptune",           "atlantis_background"),
        ("auditiongold",      "audition_gold",     "audition_gold_8_bit"),
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
    fs::create_dir_all(FURNI_ASSETS).expect("create assets dir");
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
        let cct_path = format!("{}/{}.cct", CASTS_ROOT, cct_base);
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

        // Dump the bg specifically into Furni assets dir.
        if let Some((cl, cm, name)) = &bg_target {
            let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm));
            if let Some((bytes, w, h)) = decode_png(&json) {
                let out = format!("{}/{}.png", FURNI_ASSETS, room_id);
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
        let fg_dest_dir = format!("{}/{}", FURNI_ASSETS, room_id);
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
            // Capture metadata regardless of PNG decode success — name lookup
            // and bitmap shape are useful even for empty/zero-byte members.
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                if !emit_name.is_empty() {
                    members_meta.push(serde_json::json!({
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
                    }));
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

    // After processing the table, also handle the casting_call_red alias —
    // same cct as casting_call_blue.
    if PathBuf::from(format!("{}/castingcallroom.cct", CASTS_ROOT)).exists() {
        let src = format!("{}/casting_call_blue.png", FURNI_ASSETS);
        let dst = format!("{}/casting_call_red.png", FURNI_ASSETS);
        if PathBuf::from(&src).exists() {
            fs::copy(&src, &dst).ok();
            summary.push(format!("  ✓ casting_call_blue.png → casting_call_red.png (alias)"));
        }
        // Also copy the per-room asset dir + _members.json for the alias.
        let blue_dir = PathBuf::from(format!("{}/casting_call_blue", FURNI_ASSETS));
        let red_dir = PathBuf::from(format!("{}/casting_call_red", FURNI_ASSETS));
        if blue_dir.is_dir() {
            fs::create_dir_all(&red_dir).ok();
            if let Ok(entries) = fs::read_dir(&blue_dir) {
                for entry in entries.flatten() {
                    let from = entry.path();
                    let to = red_dir.join(entry.file_name());
                    fs::copy(&from, &to).ok();
                }
            }
            summary.push(format!("  ✓ casting_call_blue/* → casting_call_red/* (alias)"));
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

/// Read each Furni room's canonical.roomBitmaps[].member names (excluding
/// id="floor") so we know which FG cast members to dump for that room.
fn read_canonical_fg_members() -> HashMap<String, Vec<String>> {
    use std::path::Path;
    let mut out = HashMap::new();
    let rooms_dir = "/Users/reoiv/Development/sandbox/decibel/packages/common/src/data/rooms";
    if !Path::new(rooms_dir).exists() {
        return out;
    }
    for entry in fs::read_dir(rooms_dir).unwrap_or_else(|_| panic!("read {}", rooms_dir)).flatten() {
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
