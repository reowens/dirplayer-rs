//! Native integration test: dump named UI bitmap members from engine
//! cast files (`cc_interface[1].cct`, `cc_room.cct`, `isoengine.cct`,
//! ...) to authoritative PNGs under
//! `packages/client/public/assets/ui/<name>.png`.
//!
//! Sibling of `dump_studio_bitmaps.rs`. Same toolchain (TestPlayer +
//! `mcp_get_cast_member_picture`), but pointed at engine casts that
//! hold UI sprites referenced by name from upstream Lingo (e.g.
//! `cc.thumbicon.up` / `cc.thumbicon.down` consumed by
//! `Cast External ParentScript 32 - VoteIndicator.ls`).
//!
//! Why this exists: engine bitmaps live inside `cc_*.cct` cast libs,
//! not the per-room casts that `dump_cct_bitmaps.rs` walks. Adding
//! them to that loop would conflate engine UI with per-room FG
//! overlays. A focused engine dumper keeps the two pipelines separate
//! and makes it cheap to grab additional UI sprites later (just append
//! to `targets()`).
//!
//! Install:
//!   cp upstream/cokemusic-casts/dirplayer-patches/dump_engine_bitmaps.rs.example \
//!      ~/Development/packages/dirplayer-rs/vm-rust/tests/dump_engine_bitmaps.rs
//!
//! Run:
//!   cargo test -p vm-rust --test dump_engine_bitmaps -- --nocapture
//!
//! Output:
//!   - packages/client/public/assets/ui/<member_name>.png  (one per target)
//!   - packages/client/public/assets/ui/_engine_members.json
//!     (regPoint / width / height / bitDepth / useAlpha per member —
//!     consumed by RoomScene to anchor the sprite at upstream's iVOffset)
//!   - /tmp/dirplayer_dumps/<cct_stem>/<cl>_<cm>_<safe>.png  (scratch)

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

/// Input cct mirror — set CASTS_ROOT to the directory containing the
/// engine cct files (cc_room.cct, isoengine.cct, etc.).
fn casts_root() -> String {
    std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string())
}
/// Output dir for the dumped UI/engine PNGs + _engine_members.json.
fn ui_output_dir() -> String {
    let root = std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string());
    format!("{}/ui", root)
}
const SCRATCH_DUMP_ROOT: &str = "/tmp/dirplayer_dumps";

/// (cct filename relative to CASTS_ROOT, cast member name, output png stem
/// under ui_output_dir()). The output stem matches the upstream member
/// name exactly so consumers can resolve the asset by the same string the
/// Lingo `sMember = "cc.thumbicon.up"` references.
fn targets() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("cc_interface[1].cct", "cc.thumbicon.up",   "cc.thumbicon.up"),
        ("cc_interface[1].cct", "cc.thumbicon.down", "cc.thumbicon.down"),
    ]
}

#[test]
fn dump_engine_cct_bitmaps() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    fs::create_dir_all(ui_output_dir()).expect("create ui assets dir");
    fs::create_dir_all(SCRATCH_DUMP_ROOT).expect("create scratch dir");

    let mut player = TestPlayer::new();
    let mut summary: Vec<String> = Vec::new();
    let mut members_meta: Vec<serde_json::Value> = Vec::new();

    // Group targets by cct — one load per file.
    use std::collections::BTreeMap;
    let mut by_cct: BTreeMap<&str, Vec<(&str, &str)>> = BTreeMap::new();
    for (cct, name, stem) in targets() {
        by_cct.entry(cct).or_default().push((name, stem));
    }

    for (cct_name, names) in &by_cct {
        let cct_path = format!("{}/{}", casts_root(), cct_name);
        if !PathBuf::from(&cct_path).exists() {
            summary.push(format!("  ✗ {}: cct not found", cct_name));
            continue;
        }

        let stem = PathBuf::from(cct_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("engine")
            .to_string();
        let scratch_dir = PathBuf::from(format!("{}/{}", SCRATCH_DUMP_ROOT, stem));
        fs::create_dir_all(&scratch_dir).ok();

        // Load + synthesize cast (mirrors dump_studio_bitmaps.rs).
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

        // Index every bitmap by name.
        let mut name_to_id: std::collections::HashMap<String, (i32, i32)> =
            std::collections::HashMap::new();
        let mut all_bitmaps: Vec<(i32, i32, String)> = Vec::new();
        reserve_player_ref(|player| {
            for cast in player.movie.cast_manager.casts.iter() {
                for (member_num, member) in cast.members.iter() {
                    if matches!(member.member_type, CastMemberType::Bitmap(_)) {
                        let cl = cast.number as i32;
                        let cm = *member_num as i32;
                        let display = if member.name.is_empty() {
                            format!("member_{}", member_num)
                        } else {
                            member.name.clone()
                        };
                        if !member.name.is_empty() {
                            // First-occurrence wins (matches Director's
                            // lowest-cm name resolution).
                            name_to_id
                                .entry(member.name.clone())
                                .or_insert((cl, cm));
                        }
                        all_bitmaps.push((cl, cm, display));
                    }
                }
            }
        });

        summary.push(format!(
            "  loaded {} → {} bitmap members indexed",
            cct_name,
            all_bitmaps.len()
        ));

        // Resolve each requested target.
        for (member_name, file_stem) in names {
            let Some(&(cl, cm)) = name_to_id.get(*member_name) else {
                summary.push(format!(
                    "  ✗ {:<26} ({}) not found in {}",
                    member_name, file_stem, cct_name
                ));
                continue;
            };

            let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, cl, cm));
            let parsed: serde_json::Value = match serde_json::from_str(&json) {
                Ok(v) => v,
                Err(e) => {
                    summary.push(format!(
                        "  ✗ {:<26} json parse failed: {}",
                        member_name, e
                    ));
                    continue;
                }
            };
            let Some((bytes, w, h)) = decode_png(&json) else {
                let first_line = json.lines().next().unwrap_or("(empty)");
                summary.push(format!(
                    "  ✗ {:<26} png decode failed: {}",
                    member_name, first_line
                ));
                continue;
            };

            let out_path = format!("{}/{}.png", ui_output_dir(), file_stem);
            fs::write(&out_path, &bytes).expect("write engine ui png");

            members_meta.push(serde_json::json!({
                "name": member_name,
                "filename": format!("{}.png", file_stem),
                "sourceCct": cct_name,
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

            summary.push(format!(
                "  ✓ {:<26} → {}.png  {}x{} ({} bytes) regPoint=({:?},{:?}) bd={:?} alpha={:?}",
                member_name,
                file_stem,
                w,
                h,
                bytes.len(),
                parsed.get("reg_x"),
                parsed.get("reg_y"),
                parsed.get("bit_depth"),
                parsed.get("use_alpha"),
            ));
        }

        // Scratch dump every bitmap in this cct for inspection.
        for (cl, cm, name) in &all_bitmaps {
            let json = reserve_player_ref(|player| mcp_get_cast_member_picture(player, *cl, *cm));
            if let Some((bytes, _, _)) = decode_png(&json) {
                let safe = name
                    .chars()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' {
                            c
                        } else {
                            '_'
                        }
                    })
                    .collect::<String>();
                let out = scratch_dir.join(format!("{}_{}_{}.png", cl, cm, safe));
                fs::write(out, &bytes).ok();
            }
        }
        summary.push(format!(
            "    scratch: {} bitmaps → {}",
            all_bitmaps.len(),
            scratch_dir.display()
        ));
    }

    // Sidecar manifest. The runtime VoteIndicator port uses regPoint to
    // anchor the sprite at upstream's `iVOffset = 0` baseline (Director
    // bitmaps render with top-left at sprite locH/locV minus regPoint).
    if !members_meta.is_empty() {
        let meta_path = format!("{}/_engine_members.json", ui_output_dir());
        let meta_json =
            serde_json::to_string_pretty(&members_meta).expect("serialize _engine_members.json");
        fs::write(&meta_path, meta_json).expect("write _engine_members.json");
        summary.push(format!(
            "    _engine_members.json: {} entries → {}",
            members_meta.len(),
            meta_path
        ));
    }

    println!();
    println!("=== Engine ui extraction summary ===");
    for line in &summary {
        println!("{}", line);
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
