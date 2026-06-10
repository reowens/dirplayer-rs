//! Phase 2c step 0a — sprite-channel dump for FurniFactory2.dcr.
//!
//! Sibling of `dump_dcr_bitmaps.rs`. Mirrors the same load+CastLib-synth
//! preamble, then walks `dir.score.frame_data.frame_channel_data` to
//! emit a `_sprites.json` sidecar next to `_members.json`. Resolves
//! cast_lib/cast_member to member names so downstream consumers
//! (Recycler render port) can look up each fixture's authored stage
//! position by member name without re-walking the score.
//!
//! Behavior attachments (`SpriteDetailInfo.behaviors`) are also
//! emitted: each entry carries the attached BehaviorScript's name
//! (e.g. `ControllerSled`, `StaticScript`) so a port can decide
//! per-channel which obstacle class instantiates that sprite.
//!
//! Run:
//!   CASTS_ROOT=/Users/reoiv/Development/sandbox/decibel/upstream/cokemusic-casts \
//!   OUTPUT_ROOT=/Users/reoiv/Development/sandbox/decibel/packages/client/public/assets \
//!     cargo test --manifest-path ~/Development/projects/cokemusic/dirplayer-rs/vm-rust/Cargo.toml \
//!     --test dump_dcr_score -- --nocapture

#![cfg(not(target_arch = "wasm32"))]

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

fn dcr_path() -> String {
    if let Ok(p) = std::env::var("DCR_PATH") {
        return p;
    }
    let root = std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string());
    format!("{}/games/FurniFactory/FurniFactory2.dcr", root)
}

fn output_root() -> String {
    std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string())
}

fn recycler_output_dir() -> String {
    format!("{}/games/recycler", output_root())
}

const SCRATCH_DUMP_ROOT: &str = "/tmp/dirplayer_dumps/recycler";

#[test]
fn dump_recycler_dcr_score() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    let primary_dir = recycler_output_dir();
    fs::create_dir_all(&primary_dir).expect("create primary recycler dir");
    fs::create_dir_all(SCRATCH_DUMP_ROOT).ok();

    let dcr = dcr_path();
    if !PathBuf::from(&dcr).exists() {
        panic!("FurniFactory2.dcr not found at {}", dcr);
    }

    let mut player = TestPlayer::new();
    println!("Loading {}", dcr);
    player.load_movie(&dcr).await;

    // Mirror the bitmap dumper's defensive CastLib synthesis.
    reserve_player_mut(|player| {
        let dir_opt = player.movie.file.take();
        if let Some(dir) = dir_opt {
            let needs_synth = player.movie.cast_manager.casts.is_empty();
            if needs_synth {
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
                    cast.apply_cast_def(
                        &dir,
                        cast_def,
                        &mut player.bitmap_manager,
                        &dir.font_table,
                    );
                    player.movie.cast_manager.casts.push(cast);
                }
            }
            player.movie.file = Some(dir);
        }
    });

    // Build a (cast_lib, cast_member) → name lookup, plus a separate
    // BehaviorScript name lookup keyed the same way (for behavior
    // attachments on the sprite_details side).
    let mut name_lookup: HashMap<(u16, u16), (String, String)> = HashMap::new();
    reserve_player_ref(|player| {
        for cast in player.movie.cast_manager.casts.iter() {
            for (member_num, member) in cast.members.iter() {
                let name = if member.name.is_empty() {
                    String::new()
                } else {
                    member.name.clone()
                };
                let kind = match &member.member_type {
                    CastMemberType::Bitmap(_) => "bitmap",
                    CastMemberType::Sound(_) => "sound",
                    CastMemberType::Script(_) => "script",
                    CastMemberType::Field(_) => "field",
                    CastMemberType::Text(_) => "text",
                    CastMemberType::Palette(_) => "palette",
                    _ => "other",
                };
                name_lookup.insert(
                    (cast.number as u16, *member_num as u16),
                    (name, kind.to_string()),
                );
            }
        }
    });

    // Walk score.frame_data.frame_channel_data + score.sprite_details.
    // Score lives on the DirectorFile (player.movie.file.as_ref().unwrap().score).
    let mut sprite_records: Vec<serde_json::Value> = Vec::new();
    let mut frame_count = 0usize;
    let mut channel_count = 0usize;
    let mut detail_count = 0usize;

    reserve_player_ref(|player| {
        let dir = match player.movie.file.as_ref() {
            Some(d) => d,
            None => {
                println!("✗ player.movie.file is None — score not accessible");
                return;
            }
        };

        let score = match dir.score.as_ref() {
            Some(s) => s,
            None => {
                println!("✗ dir.score is None — VWSC chunk not parsed");
                return;
            }
        };

        frame_count = score.frame_data.frame_channel_data.iter()
            .map(|(f, _, _)| *f).max().unwrap_or(0) as usize;
        channel_count = score.frame_data.frame_channel_data.len();
        detail_count = score.sprite_details.len();

        // For each (frame, channel, ScoreFrameChannelData), emit a record.
        // Most stage layouts repeat the same channel across many frames;
        // we still emit every frame because some animations rotate cast
        // members through frames. The downstream consumer is expected
        // to dedupe by (channel, cast_member, pos_x, pos_y) if desired.
        for (frame, channel, data) in &score.frame_data.frame_channel_data {
            // Skip empty channels (cast_member=0 means nothing in slot).
            if data.cast_member == 0 && data.cast_lib == 0 {
                continue;
            }

            let key = (data.cast_lib, data.cast_member);
            let (name, kind) = name_lookup
                .get(&key)
                .cloned()
                .unwrap_or_else(|| (String::new(), "unknown".to_string()));

            // Resolve attached behaviors via sprite_list_idx.
            let sprite_list_idx = data.sprite_list_idx();
            let mut behaviors: Vec<serde_json::Value> = Vec::new();
            if let Some(detail) = score.sprite_details.get(&sprite_list_idx) {
                for b in &detail.behaviors {
                    let bkey = (b.cast_lib, b.cast_member);
                    let (bname, _bkind) = name_lookup
                        .get(&bkey)
                        .cloned()
                        .unwrap_or_else(|| (String::new(), "unknown".to_string()));
                    // Strip the "Cast <castlib> BehaviorScript <n> - " prefix
                    // that Director cast member names sometimes carry to leave
                    // just the script name (e.g. "ControllerSled").
                    let stripped = strip_script_prefix(&bname);
                    behaviors.push(serde_json::json!({
                        "castLib": b.cast_lib,
                        "castMember": b.cast_member,
                        "name": stripped,
                        "rawName": bname,
                        "paramCount": b.parameter.len(),
                    }));
                }
            }

            sprite_records.push(serde_json::json!({
                "frame": frame,
                "channel": channel,
                "castLib": data.cast_lib,
                "castMember": data.cast_member,
                "memberName": name,
                "memberKind": kind,
                "spriteType": data.sprite_type,
                "ink": data.ink,
                "blend": data.blend,
                "posX": data.pos_x,
                "posY": data.pos_y,
                "width": data.width,
                "height": data.height,
                "spriteListIdx": sprite_list_idx,
                "behaviors": behaviors,
            }));
        }
    });

    println!(
        "Score walk: frames=≤{}, frame_channel_records={}, sprite_details={}, emitted={}",
        frame_count, channel_count, detail_count, sprite_records.len()
    );

    // Also emit a deduped "fixtures" view: one record per (member_name,
    // pos_x, pos_y), preserving first-seen frame/channel/behaviors. This
    // is the direct input for static-fixture placement in the render
    // port — most stage objects appear once per cast member at one stage
    // location across many frames.
    let mut seen: HashMap<(String, i16, i16), serde_json::Value> = HashMap::new();
    for rec in &sprite_records {
        let name = rec
            .get("memberName")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }
        let px = rec.get("posX").and_then(|v| v.as_i64()).unwrap_or(0) as i16;
        let py = rec.get("posY").and_then(|v| v.as_i64()).unwrap_or(0) as i16;
        seen.entry((name, px, py)).or_insert_with(|| rec.clone());
    }
    let mut fixtures: Vec<serde_json::Value> = seen.into_values().collect();
    fixtures.sort_by(|a, b| {
        let an = a.get("memberName").and_then(|v| v.as_str()).unwrap_or("");
        let bn = b.get("memberName").and_then(|v| v.as_str()).unwrap_or("");
        an.cmp(bn)
    });

    let payload = serde_json::json!({
        "source": dcr,
        "frameCountUpper": frame_count,
        "frameChannelRecordCount": channel_count,
        "spriteDetailCount": detail_count,
        "fixtures": fixtures,
        "rawSpriteRecords": sprite_records,
    });

    let out_path = format!("{}/_sprites.json", primary_dir);
    let json = serde_json::to_string_pretty(&payload).expect("serialize sprites");
    fs::write(&out_path, &json).expect("write _sprites.json");
    fs::write(format!("{}/_sprites.json", SCRATCH_DUMP_ROOT), &json).ok();

    println!("✓ wrote {} ({} fixtures, {} raw records)", out_path,
        payload.get("fixtures").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0),
        sprite_records.len());

    assert!(!sprite_records.is_empty(),
        "Score chunk parsed but emitted zero sprite records — \
         either the VWSC chunk is empty or frame_channel_data is unpopulated");
}

/// Cast member names for BehaviorScripts in the upstream CokeMusic casts
/// often carry a "Cast <castlib> BehaviorScript <n> - " or similar
/// administrative prefix. Strip it so downstream consumers see the bare
/// script name (e.g. "ControllerSled" instead of
/// "Cast gamecode BehaviorScript 3 - ControllerSled").
fn strip_script_prefix(s: &str) -> String {
    if let Some(idx) = s.rfind(" - ") {
        s[idx + 3..].to_string()
    } else {
        s.to_string()
    }
}
