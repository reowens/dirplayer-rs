use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::bitmap::bitmap::PaletteRef;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::{CastMember, CastMemberType};
use vm_rust::player::mcp::{
    collect_palette_table, mcp_get_cast_member_picture, mcp_get_cast_member_picture_with_palette,
    palettes_manifest_json,
};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{DirPlayer, reserve_player_mut, reserve_player_ref};

#[derive(Clone, Copy)]
pub(crate) struct FurnitureBitmapProfile {
    pub(crate) source_cct: &'static str,
    pub(crate) output_subdirectory: &'static str,
    pub(crate) members_sidecar: &'static str,
    pub(crate) dumper_name: &'static str,
    pub(crate) external_palette_source_cct: Option<&'static str>,
    pub(crate) emit_matte_masks: bool,
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
    pub(crate) external_palette_renders: usize,
    pub(crate) matte_masks_written: usize,
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

fn matte_dir(profile: FurnitureBitmapProfile) -> String {
    format!("{}/matte", output_dir(profile))
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

fn external_palette_member(cast_lib: i32, cast_member: i32, palette_cast: i32) -> Option<i32> {
    reserve_player_ref(|player| {
        let cast = player.movie.cast_manager.get_cast(cast_lib as u32).ok()?;
        let bitmap_member = cast
            .members
            .get(&(cast_member as u32))?
            .member_type
            .as_bitmap()?;
        let bitmap = player.bitmap_manager.get_bitmap(bitmap_member.image_ref)?;
        if !bitmap.has_palette() {
            return None;
        }
        let palette_ref = match &bitmap.palette_ref {
            PaletteRef::Member(member_ref) => member_ref,
            _ => return None,
        };
        if palette_ref.cast_lib > 0
            && player
                .movie
                .cast_manager
                .get_cast(palette_ref.cast_lib as u32)
                .ok()
                .and_then(|palette_source| {
                    palette_source
                        .members
                        .get(&(palette_ref.cast_member as u32))
                })
                .is_some()
        {
            return None;
        }
        let external_cast = player
            .movie
            .cast_manager
            .get_cast(palette_cast as u32)
            .ok()?;
        external_cast
            .members
            .get(&(palette_ref.cast_member as u32))?;
        Some(palette_ref.cast_member)
    })
}

fn encode_matte_mask(
    player: &DirPlayer,
    cast_lib: i32,
    cast_member: i32,
) -> Result<Vec<u8>, String> {
    use image::{ImageFormat, RgbaImage};

    let cast = player
        .movie
        .cast_manager
        .get_cast(cast_lib as u32)
        .map_err(|_| format!("cast library {} not found", cast_lib))?;
    let member = cast
        .members
        .get(&(cast_member as u32))
        .ok_or_else(|| format!("cast member {}/{} not found", cast_lib, cast_member))?;
    let bitmap_member = member
        .member_type
        .as_bitmap()
        .ok_or_else(|| format!("cast member {}/{} is not a bitmap", cast_lib, cast_member))?;
    let mut bitmap = player
        .bitmap_manager
        .get_bitmap(bitmap_member.image_ref)
        .ok_or_else(|| {
            format!(
                "bitmap data for cast member {}/{} not loaded",
                cast_lib, cast_member
            )
        })?
        .clone();
    let palettes = player.movie.cast_manager.palettes();
    bitmap.create_matte(&palettes);
    let matte = bitmap.matte.as_ref().ok_or_else(|| {
        format!(
            "matte creation failed for cast member {}/{}",
            cast_lib, cast_member
        )
    })?;
    let mut image = RgbaImage::new(bitmap.width as u32, bitmap.height as u32);
    for y in 0..bitmap.height {
        for x in 0..bitmap.width {
            let alpha = if matte.get_bit(x, y) { 255 } else { 0 };
            image.put_pixel(x as u32, y as u32, image::Rgba([255, 255, 255, alpha]));
        }
    }
    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), ImageFormat::Png)
        .map_err(|error| format!("matte PNG encoding failed ({})", error))?;
    Ok(png)
}

pub(crate) async fn dump_furniture_profile(
    profile: FurnitureBitmapProfile,
) -> FurnitureBitmapDumpReport {
    fs::create_dir_all(output_dir(profile)).expect("create furniture assets dir");
    fs::create_dir_all(png_dir(profile)).expect("create furniture data dir");
    if profile.emit_matte_masks {
        fs::create_dir_all(matte_dir(profile)).expect("create furniture matte dir");
    }

    let cct_path = format!("{}/{}", casts_root(), profile.source_cct);
    if !PathBuf::from(&cct_path).exists() {
        panic!("cct not found at {} - set CASTS_ROOT", cct_path);
    }

    let mut player = TestPlayer::new();
    let external_palettes: Vec<(u32, CastMember)> =
        if let Some(source_cct) = profile.external_palette_source_cct {
            let source_path = format!("{}/{}", casts_root(), source_cct);
            if !PathBuf::from(&source_path).exists() {
                panic!(
                    "palette source cct not found at {} - set CASTS_ROOT",
                    source_path
                );
            }
            player.load_movie(&source_path).await;
            synthesize_casts_from_loaded_movie();
            reserve_player_ref(|player| {
                player
                    .movie
                    .cast_manager
                    .casts
                    .iter()
                    .flat_map(|cast| cast.members.iter())
                    .filter_map(|(member_num, member)| {
                        matches!(member.member_type, CastMemberType::Palette(_))
                            .then(|| (*member_num, member.clone()))
                    })
                    .collect()
            })
        } else {
            Vec::new()
        };
    player.load_movie(&cct_path).await;
    synthesize_casts_from_loaded_movie();
    let source_cast_count = reserve_player_ref(|player| player.movie.cast_manager.casts.len());
    let external_palette_cast = if external_palettes.is_empty() {
        None
    } else {
        Some(reserve_player_mut(|player| {
            let cast_number = player
                .movie
                .cast_manager
                .casts
                .iter()
                .map(|cast| cast.number)
                .max()
                .unwrap_or(0)
                + 1;
            let mut palette_cast = CastLib {
                name: "__external_palettes".to_string(),
                file_name: profile.external_palette_source_cct.unwrap().to_string(),
                number: cast_number,
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
            player.movie.cast_manager.casts.push(palette_cast);
            cast_number as i32
        }))
    };

    let mut all_named: Vec<(i32, i32, String)> = Vec::new();
    let mut total_bitmaps = 0usize;
    let mut unnamed_bitmaps = 0usize;
    let mut palette_members = 0usize;
    let mut empty_bitmaps: Vec<String> = Vec::new();

    reserve_player_ref(|player| {
        for cast in player
            .movie
            .cast_manager
            .casts
            .iter()
            .take(source_cast_count)
        {
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
    let mut external_palette_renders = 0usize;
    let mut matte_masks_written = 0usize;

    for (cast_lib, cast_member, name) in &targets {
        let external_palette = external_palette_cast.and_then(|palette_cast| {
            external_palette_member(*cast_lib, *cast_member, palette_cast)
        });
        let json = match (external_palette_cast, external_palette) {
            (Some(palette_cast), Some(palette_member)) => {
                external_palette_renders += 1;
                reserve_player_ref(|player| {
                    mcp_get_cast_member_picture_with_palette(
                        player,
                        *cast_lib,
                        *cast_member,
                        palette_cast,
                        palette_member,
                    )
                })
            }
            _ => reserve_player_ref(|player| {
                mcp_get_cast_member_picture(player, *cast_lib, *cast_member)
            }),
        };
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

        if profile.emit_matte_masks {
            let matte =
                reserve_player_ref(|player| encode_matte_mask(player, *cast_lib, *cast_member));
            match matte {
                Ok(bytes) => {
                    fs::write(format!("{}/{}", matte_dir(profile), filename), bytes)
                        .expect("write furniture matte PNG");
                    matte_masks_written += 1;
                }
                Err(error) => {
                    decode_failures.push(format!("{}: {}", name, error));
                    continue;
                }
            }
        }

        collect_palette_table(&mut palette_tables, &parsed);
        let mut member_meta = serde_json::json!({
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
        });
        if let (Some(palette_member), Some(source_cct), Some(object)) = (
            external_palette,
            profile.external_palette_source_cct,
            member_meta.as_object_mut(),
        ) {
            object.insert(
                "externalPalette".into(),
                serde_json::json!({
                    "sourceCct": source_cct,
                    "castMember": palette_member,
                }),
            );
        }
        if profile.emit_matte_masks {
            member_meta
                .as_object_mut()
                .expect("furniture member metadata object")
                .insert("matteFilename".into(), serde_json::json!(filename));
        }
        members_meta.push(member_meta);
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
        "  emitted {} member entries, {} PNGs, {} matte masks -> {} ({} decode failures)",
        members_meta.len(),
        pngs_written,
        matte_masks_written,
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
        external_palette_renders,
        matte_masks_written,
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
