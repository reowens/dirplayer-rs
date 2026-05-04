//! Phase 0 / Phase 2a — bitmap dumper for the Recycler mini-game's
//! `FurniFactory2.dcr` Director movie. Sibling of `dump_cct_bitmaps.rs.example`.
//!
//! **Phase 0 goal** (toolchain validation): confirm dirplayer-rs can load
//! a `.dcr` (XFIR-magic, compressed Director movie) and dump one named
//! bitmap member to PNG. Targets `Background` first because it's the
//! simplest non-trivial member and is one of the few whose existence
//! we've verified from Lingo source.
//!
//! **Phase 2a goal** (extraction): once Phase 0 passes, the same test
//! becomes the production-ready dumper for all Recycler bitmaps. It
//! emits per-member PNGs to `<OUTPUT_ROOT>/games/recycler/` (default
//! `./out/games/recycler/` if unset) plus a `_members.json` sidecar
//! containing `{name, regX, regY, bitDepth, useAlpha, width, height}`
//! per member — same schema as the publicroom dump, consumed downstream
//! by hand-curation into the gating `docs/casts/recycler_cast_inventory.md`.
//! A scratch dual-write to `/tmp/dirplayer_dumps/recycler/` is also
//! performed (mirrors `dump_cct_bitmaps.rs`'s `PER_ROOM_DUMP_ROOT`
//! pattern) so debugging from a fixed path stays easy regardless of
//! where the primary output is wired.
//!
//! Notes vs the .cct dumper:
//!   - `.dcr` is a Director MOVIE (not a standalone .cct cast), so
//!     `dir.casts` already contains the proper cast_table; we don't need
//!     to synthesize a CastLib from cast_def[0]. We still defensively
//!     mirror the CastLib-build pattern from the .cct dumper in case
//!     the loader returns an empty cast_manager.
//!   - Compression: `XFIR` is the same Director chunk format as `RIFX`
//!     but ZLIB-compressed end-to-end. dirplayer-rs's `load_movie_file`
//!     handles the decompression transparently — same code path as the
//!     `.cct` test. If this test fails at load time, the issue is most
//!     likely the compression hook, not the cast format.
//!
//! Install:
//!   cp upstream/cokemusic-casts/dirplayer-patches/dump_dcr_bitmaps.rs.example \
//!      ~/Development/packages/dirplayer-rs/vm-rust/tests/dump_dcr_bitmaps.rs
//!
//! Run:
//!   OUTPUT_ROOT=/tmp/dirplayer_test \
//!     cargo test -p vm-rust --test dump_dcr_bitmaps -- --nocapture
//!
//! Acceptance for Phase 0: the test prints
//!   ✓ Background <W>x<H> (<N> bytes)
//! and at least one PNG appears under <OUTPUT_ROOT>/games/recycler/.

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

/// Path to the .dcr file. Set DCR_PATH explicitly, OR set CASTS_ROOT and
/// the file will be looked up at `${CASTS_ROOT}/games/FurniFactory/FurniFactory2.dcr`.
fn dcr_path() -> String {
    if let Ok(p) = std::env::var("DCR_PATH") {
        return p;
    }
    let root = std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string());
    format!("{}/games/FurniFactory/FurniFactory2.dcr", root)
}
/// Output root for the rendered PNGs + `_members.json` sidecar. Mirrors
/// the env-var pattern in `dump_cct_bitmaps.rs`.
fn output_root() -> String {
    std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string())
}
fn recycler_output_dir() -> String {
    format!("{}/games/recycler", output_root())
}
/// Scratch sibling write — preserved at a fixed path so debugging from
/// a known location stays easy regardless of how `OUTPUT_ROOT` is wired.
/// Matches `dump_cct_bitmaps.rs`'s `PER_ROOM_DUMP_ROOT` convention.
const SCRATCH_DUMP_ROOT: &str = "/tmp/dirplayer_dumps/recycler";

/// Per-sound metadata captured during enumeration. WAV bytes are
/// computed inline so the dump-time loop doesn't need a second
/// player borrow.
struct SoundTarget {
    cast_lib: i32,
    cast_member: i32,
    name: String,
    wav_bytes: Vec<u8>,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    sample_count: u32,
    codec: String,
}

/// The single member name we use to gate Phase 0. The DCR's actual
/// background bitmap is named `bg` (Lingo `Background` is a Director
/// channel-zero pun, not a cast-member name). At 760×521 it matches
/// the upstream stage size from `IsoConstants.ls`.
const PHASE_0_GATE_MEMBER: &str = "bg";

/// Member-name prefixes we expect, post-Phase-0-investigation. The
/// CAST member names match wiki display names (`shocker`, `worker`,
/// `robot`, `tenaza`, `bucket`, `stroller`), not Lingo class names
/// (`WhaleBase`, `FishBots`, etc.). The Lingo classes are script-
/// internal labels; the bitmaps were properly renamed for Recycler.
/// The only Whale Wash residue is in easter-egg member names like
/// `smell fishy` and `fin design`.
///
/// Used during the full bitmap dump to categorize what was found,
/// not to filter — every non-avatar bitmap gets dumped regardless.
const EXPECTED_PREFIXES: &[&str] = &[
    "Tool_",                // Tool_<n> (rack, 40x29) and Tool_<n>_<dir> (carried, 20x30, dir=0/2/4/6)
    "shocker",              // Shockercoils — shocker_<row>_<frame>, 5x3 grid, 109x64
    "worker",               // Workers — 4-frame animation, 51x68
    "robot",                // Robots — 4-frame animation, 55x72
    "fence-",               // boundary art
    "alertbox",             // popups
    "play_again",           // button states (-up / -ov)
    "scoreboard",           // scoreboard button states
    "help",                 // help button states
    "back-",                // back button states
    "human_shadow",         // shadow under avatar
];

/// Bitmap members we ignore — already shipped via the standard avatar
/// pipeline (people.cct → packages/common/src/data/spriteManifest.json).
/// `FurniFactory2.dcr` re-bundles the entire avatar engine, but those
/// bitmaps duplicate what we already have.
fn is_avatar_part(name: &str) -> bool {
    name.starts_with("h_")
        || name.starts_with("*h_")
        || name == "human_shadow_h"
        || name == "human_shadow_sh"
}

#[test]
fn dump_recycler_dcr_bitmaps() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    let primary_dir = recycler_output_dir();
    fs::create_dir_all(&primary_dir).expect("create primary recycler dir");
    fs::create_dir_all(SCRATCH_DUMP_ROOT).expect("create scratch dump root");

    let dcr = dcr_path();
    if !PathBuf::from(&dcr).exists() {
        panic!("FurniFactory2.dcr not found at {}", dcr);
    }

    let mut player = TestPlayer::new();
    let mut summary: Vec<String> = Vec::new();

    // === Load the .dcr ===
    println!("Loading {}", dcr);
    player.load_movie(&dcr).await;

    // Mirror the .cct dumper's defensive CastLib synthesis. If
    // dir.casts is populated post-load (which is what we expect for
    // a movie), this just reseats the existing data; if it's empty,
    // this populates it from the loaded movie file.
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

    // === Enumerate bitmap + sound targets ===
    // Sound targets carry the WAV bytes inline (extracted now to avoid a
    // second player.cast_manager walk during the dump section). The
    // bitmap pipeline still defers PNG generation to the per-target loop
    // because mcp_get_cast_member_picture takes its own player ref.
    let mut targets: Vec<(i32, i32, String)> = Vec::new();
    let mut sound_targets: Vec<SoundTarget> = Vec::new();
    let mut gate_target: Option<(i32, i32, String)> = None;
    let mut all_member_count = 0usize;
    let mut bitmap_member_count = 0usize;
    let mut bitmap_avatar_count = 0usize;
    let mut sound_member_count = 0usize;
    let mut sound_unnamed_count = 0usize;

    reserve_player_ref(|player| {
        for cast in player.movie.cast_manager.casts.iter() {
            for (member_num, member) in cast.members.iter() {
                all_member_count += 1;
                let cl = cast.number as i32;
                let cm = *member_num as i32;
                let name = if member.name.is_empty() {
                    format!("member_{}", member_num)
                } else {
                    member.name.clone()
                };
                match &member.member_type {
                    CastMemberType::Bitmap(_) => {
                        bitmap_member_count += 1;
                        if is_avatar_part(&name) {
                            bitmap_avatar_count += 1;
                            continue; // skip avatar-engine duplicates
                        }
                        if name == PHASE_0_GATE_MEMBER {
                            gate_target = Some((cl, cm, name.clone()));
                        }
                        targets.push((cl, cm, name));
                    }
                    CastMemberType::Sound(sm) => {
                        sound_member_count += 1;
                        if member.name.is_empty() {
                            // Unnamed sound members can't be wired into a
                            // gameplay scene without a name to reference,
                            // so we skip them rather than emit `member_N.wav`
                            // files that no one knows what to do with.
                            sound_unnamed_count += 1;
                            continue;
                        }
                        sound_targets.push(SoundTarget {
                            cast_lib: cl,
                            cast_member: cm,
                            name: name.clone(),
                            wav_bytes: sm.sound.to_wav(),
                            channels: sm.sound.channels(),
                            sample_rate: sm.sound.sample_rate(),
                            bits_per_sample: sm.sound.bits_per_sample(),
                            sample_count: sm.sound.sample_count(),
                            codec: sm.sound.codec(),
                        });
                    }
                    _ => {}
                }
            }
        }
    });

    summary.push(format!(
        "Loaded {} cast members ({} bitmaps total: {} avatar parts skipped, {} game bitmaps; {} sounds total: {} unnamed skipped, {} extracting; {} other).",
        all_member_count,
        bitmap_member_count,
        bitmap_avatar_count,
        targets.len(),
        sound_member_count,
        sound_unnamed_count,
        sound_targets.len(),
        all_member_count - bitmap_member_count - sound_member_count
    ));

    if bitmap_member_count == 0 {
        summary.push(
            "  ✗ Phase 0 FAILED: no bitmap members found. dirplayer may not be \
             loading the .dcr at all, or the cast_manager wasn't populated. \
             Investigate: check `dir.casts.len()` post-load; check `XFIR` \
             decompression in dirplayer-rs."
                .to_string(),
        );
        print_summary(&summary);
        panic!("Phase 0 gate failed — no bitmap members loaded");
    }

    // === Phase 0 gate: dump the single Background member ===
    if let Some((cl, cm, name)) = &gate_target {
        let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm));
        if let Some((bytes, w, h)) = decode_png(&json) {
            let out = format!("{}/{}.png", primary_dir, sanitize(name));
            fs::write(&out, &bytes).expect("write background png");
            // Scratch sibling write for fixed-path debugging.
            let scratch_out = format!("{}/{}.png", SCRATCH_DUMP_ROOT, sanitize(name));
            fs::write(&scratch_out, &bytes).ok();
            summary.push(format!(
                "  ✓ Phase 0 gate: {} {}x{} ({} bytes) → {}",
                name,
                w,
                h,
                bytes.len(),
                out
            ));
        } else {
            summary.push(format!(
                "  ✗ Phase 0 gate: {} found but PNG decode failed: {}",
                name,
                json.lines().next().unwrap_or("(empty)")
            ));
        }
    } else {
        summary.push(format!(
            "  ⚠ Phase 0 gate: no member named '{}'. Gate will be re-evaluated \
             against the all-bitmap dump below.",
            PHASE_0_GATE_MEMBER
        ));
    }

    // === Phase 2a payload: dump every bitmap + emit _members.json ===
    let mut members_meta: Vec<serde_json::Value> = Vec::new();
    let mut dumped_count = 0usize;
    let mut failed_count = 0usize;
    let mut prefix_hits: std::collections::BTreeMap<&'static str, usize> =
        EXPECTED_PREFIXES.iter().map(|p| (*p, 0usize)).collect();
    let mut unexpected_names: Vec<String> = Vec::new();

    for (cl, cm, name) in &targets {
        let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm));
        let png = decode_png(&json);
        if let Some((bytes, _, _)) = &png {
            let out = format!("{}/{}_{}_{}.png", primary_dir, cl, cm, sanitize(name));
            // Scratch sibling write — same filename layout, fixed root.
            let scratch_out = format!("{}/{}_{}_{}.png", SCRATCH_DUMP_ROOT, cl, cm, sanitize(name));
            fs::write(&scratch_out, bytes).ok();
            if fs::write(&out, bytes).is_ok() {
                dumped_count += 1;
            } else {
                failed_count += 1;
            }
        } else {
            failed_count += 1;
        }

        // Categorize by prefix.
        let mut matched = false;
        for prefix in EXPECTED_PREFIXES {
            if name.starts_with(prefix) {
                *prefix_hits.get_mut(prefix).unwrap() += 1;
                matched = true;
                break;
            }
        }
        if !matched && !name.starts_with("member_") {
            unexpected_names.push(name.clone());
        }

        // Capture metadata regardless of PNG decode success.
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
            if !name.is_empty() && !name.starts_with("member_") {
                members_meta.push(serde_json::json!({
                    "name": name,
                    "regX": v.get("reg_x"),
                    "regY": v.get("reg_y"),
                    "bitDepth": v.get("bit_depth"),
                    "useAlpha": v.get("use_alpha"),
                    "width": v.get("width"),
                    "height": v.get("height"),
                    "paletteRef": v.get("palette_ref"),
                }));
            }
        }
    }

    summary.push(format!(
        "  Bitmap dump: {} ok, {} failed (of {} bitmap members)",
        dumped_count, failed_count, bitmap_member_count
    ));

    summary.push("  Prefix coverage (expected from Lingo source):".to_string());
    for (prefix, count) in &prefix_hits {
        let mark = if *count > 0 { "✓" } else { "·" };
        summary.push(format!("    {} {:<22} {} member(s)", mark, prefix, count));
    }

    if !unexpected_names.is_empty() {
        summary.push(format!(
            "  Unexpected member names ({}, first 10 shown):",
            unexpected_names.len()
        ));
        for n in unexpected_names.iter().take(10) {
            summary.push(format!("    ? {}", n));
        }
    }

    // === Write _members.json sidecar ===
    if !members_meta.is_empty() {
        let meta_path = format!("{}/_members.json", primary_dir);
        let meta_json =
            serde_json::to_string_pretty(&members_meta).expect("serialize members meta");
        fs::write(&meta_path, &meta_json).expect("write _members.json");
        // Scratch sibling write.
        let scratch_meta = format!("{}/_members.json", SCRATCH_DUMP_ROOT);
        fs::write(&scratch_meta, &meta_json).ok();
        summary.push(format!(
            "  _members.json: {} entries → {}",
            members_meta.len(),
            meta_path
        ));
    }

    // === Sound dump: emit .wav per named sound member + _sounds.json ===
    // Sounds land under a `sounds/` subdir so they're grouped together
    // and so a downstream consumer can mount the directory at e.g.
    // <client>/games/recycler/sounds/ without re-organizing.
    let sounds_dir = format!("{}/sounds", primary_dir);
    let scratch_sounds_dir = format!("{}/sounds", SCRATCH_DUMP_ROOT);
    if !sound_targets.is_empty() {
        fs::create_dir_all(&sounds_dir).expect("create sounds dir");
        fs::create_dir_all(&scratch_sounds_dir).ok();
    }
    let mut sounds_meta: Vec<serde_json::Value> = Vec::new();
    let mut sound_ok = 0usize;
    let mut sound_fail = 0usize;
    for st in &sound_targets {
        let safe = sanitize(&st.name);
        let primary_wav = format!("{}/{}.wav", sounds_dir, safe);
        let scratch_wav = format!("{}/{}_{}_{}.wav",
            scratch_sounds_dir, st.cast_lib, st.cast_member, safe);
        match fs::write(&primary_wav, &st.wav_bytes) {
            Ok(_) => sound_ok += 1,
            Err(_) => sound_fail += 1,
        }
        // Scratch sibling write — disambiguated by cast pair so re-runs
        // don't collide on bare names.
        fs::write(&scratch_wav, &st.wav_bytes).ok();
        sounds_meta.push(serde_json::json!({
            "name": st.name,
            "filename": format!("{}.wav", safe),
            "castLib": st.cast_lib,
            "castMember": st.cast_member,
            "channels": st.channels,
            "sampleRate": st.sample_rate,
            "bitsPerSample": st.bits_per_sample,
            "sampleCount": st.sample_count,
            "codec": st.codec,
            "wavByteLength": st.wav_bytes.len(),
        }));
    }
    if !sound_targets.is_empty() {
        summary.push(format!(
            "  Sound dump: {} ok, {} failed (of {} named sound members; {} unnamed skipped) → {}",
            sound_ok, sound_fail, sound_targets.len(), sound_unnamed_count, sounds_dir
        ));
        let sounds_meta_path = format!("{}/_sounds.json", sounds_dir);
        let sounds_meta_json = serde_json::to_string_pretty(&sounds_meta)
            .expect("serialize sounds meta");
        fs::write(&sounds_meta_path, &sounds_meta_json)
            .expect("write _sounds.json");
        // Scratch sibling write.
        fs::write(format!("{}/_sounds.json", scratch_sounds_dir), &sounds_meta_json).ok();
        summary.push(format!(
            "  _sounds.json: {} entries → {}",
            sounds_meta.len(), sounds_meta_path
        ));
    }

    print_summary(&summary);

    // Hard-fail if Phase 0 gate didn't pass.
    assert!(
        gate_target.is_some() && dumped_count > 0,
        "Phase 0 gate did not pass — see summary above"
    );
}

fn print_summary(summary: &[String]) {
    println!();
    println!("=== FurniFactory2.dcr extraction summary ===");
    for line in summary {
        println!("{}", line);
    }
    println!();
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
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
