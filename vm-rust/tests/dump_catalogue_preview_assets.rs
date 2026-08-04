//! Focused extraction of the indexed controls, palette renders, and vector
//! geometry used by `WFpreview script` in `cc_catalogue[1].cct`.
//!
//! Run with scratch output outside Furni's runtime asset tree:
//!   CASTS_ROOT=/abs/path/upstream/cokemusic-casts/client2 \
//!   OUTPUT_ROOT=/tmp/dirplayer-catalogue-preview \
//!   cargo test -p vm-rust --test dump_catalogue_preview_assets -- --nocapture

#![cfg(not(target_arch = "wasm32"))]

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use fxhash::FxHashMap;
use vm_rust::player::cast_lib::{CastLib, CastLibState};
use vm_rust::player::cast_member::CastMemberType;
use vm_rust::player::mcp::{
    collect_palette_table, mcp_get_cast_member_picture, mcp_get_cast_member_picture_with_palette,
    palettes_manifest_json,
};
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;
use vm_rust::player::{reserve_player_mut, reserve_player_ref};

const CCT_NAME: &str = "cc_catalogue[1].cct";
const CAST_LIB: i32 = 1;
const FLOOR_GOLDEN_START: (u8, u8, u8) = (0x99, 0xBB, 0xCC);
const FLOOR_GOLDEN_END: (u8, u8, u8) = (0x33, 0x55, 0x77);

#[derive(Clone, Copy, Debug)]
struct FlshVertex {
    x: f32,
    y: f32,
    handle1_x: f32,
    handle1_y: f32,
    handle2_x: f32,
    handle2_y: f32,
}

#[derive(Debug)]
struct FlshShape {
    stroke_color: (u8, u8, u8),
    fill_color: (u8, u8, u8),
    bg_color: (u8, u8, u8),
    end_color: (u8, u8, u8),
    stroke_width: f32,
    fill_mode: u32,
    closed: bool,
    member_width: u32,
    member_height: u32,
    gradient_type: &'static str,
    fill_scale: f32,
    fill_direction: f32,
    fill_offset: (i32, i32),
    fill_cycles: i32,
    vertices: Vec<FlshVertex>,
}

#[derive(Clone, Copy)]
enum VertexRole {
    Vertex,
    Handle1,
    Handle2,
    NewCurve,
    Unknown,
}

#[derive(Clone, Copy)]
struct BitmapInput {
    role: &'static str,
    member: i32,
    name: &'static str,
    width: u16,
    height: u16,
    indexed: bool,
}

const BITMAP_INPUTS: [BitmapInput; 6] = [
    BitmapInput {
        role: "roomPreviewWall",
        member: 57,
        name: "cat.roomprev.wall",
        width: 108,
        height: 91,
        indexed: false,
    },
    BitmapInput {
        role: "roomPreviewFloor",
        member: 58,
        name: "cat.roomprev.floor",
        width: 108,
        height: 46,
        indexed: false,
    },
    BitmapInput {
        role: "wallControlStyle1Left",
        member: 53,
        name: "cat_left_wall_1_b_0_0_0",
        width: 50,
        height: 86,
        indexed: true,
    },
    BitmapInput {
        role: "wallControlStyle1Right",
        member: 54,
        name: "cat_right_wall_1_b_0_0_0",
        width: 55,
        height: 88,
        indexed: true,
    },
    BitmapInput {
        role: "wallControlStyle2Left",
        member: 63,
        name: "cat_left_wall_2_b_0_0_0",
        width: 50,
        height: 87,
        indexed: true,
    },
    BitmapInput {
        role: "wallControlStyle2Right",
        member: 64,
        name: "cat_right_wall_2_b_0_0_0",
        width: 55,
        height: 88,
        indexed: true,
    },
];

#[derive(Clone, Copy)]
struct WallPalette {
    pattern: &'static str,
    left_member: i32,
    right_member: i32,
}

const WALL_PALETTES: [WallPalette; 10] = [
    WallPalette {
        pattern: "tile",
        left_member: 66,
        right_member: 67,
    },
    WallPalette {
        pattern: "graythreedo",
        left_member: 76,
        right_member: 77,
    },
    WallPalette {
        pattern: "threegrid",
        left_member: 70,
        right_member: 71,
    },
    WallPalette {
        pattern: "zalmiag",
        left_member: 84,
        right_member: 85,
    },
    WallPalette {
        pattern: "plottr",
        left_member: 82,
        right_member: 83,
    },
    WallPalette {
        pattern: "disnej",
        left_member: 72,
        right_member: 73,
    },
    WallPalette {
        pattern: "lilacgrandma",
        left_member: 74,
        right_member: 75,
    },
    WallPalette {
        pattern: "plates",
        left_member: 68,
        right_member: 69,
    },
    WallPalette {
        pattern: "bighstripe",
        left_member: 78,
        right_member: 79,
    },
    WallPalette {
        pattern: "dithastriperh",
        left_member: 80,
        right_member: 81,
    },
];

const FLOOR_PALETTES: [(&str, i32); 15] = [
    ("orgun", 98),
    ("karpyt", 100),
    ("indjun", 99),
    ("ditlines", 97),
    ("ditgrid", 96),
    ("basic", 95),
    ("twirlee", 94),
    ("phunk", 93),
    ("wooddither", 92),
    ("oceanliner", 91),
    ("cahya", 90),
    ("purplecarpetswirl", 89),
    ("bluecarpet", 88),
    ("drija", 87),
    ("woodswirl", 86),
];

fn casts_root() -> String {
    std::env::var("CASTS_ROOT").unwrap_or_else(|_| "./casts".to_string())
}

fn output_root() -> String {
    let root = std::env::var("OUTPUT_ROOT").unwrap_or_else(|_| "./out".to_string());
    format!("{}/catalogue-preview", root)
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

#[test]
fn dump_catalogue_preview_assets() {
    async_std::task::block_on(dump_inner());
}

async fn dump_inner() {
    let source = format!("{}/{}", casts_root(), CCT_NAME);
    assert!(
        PathBuf::from(&source).exists(),
        "cct not found at {}",
        source
    );

    let root = output_root();
    for directory in [
        "inputs",
        "wall-renders",
        "floor-renders",
        "floor-shape",
        "diagnostics",
    ] {
        fs::create_dir_all(format!("{}/{}", root, directory)).expect("create output directory");
    }

    let mut player = TestPlayer::new();
    player.load_movie(&source).await;
    synthesize_casts_from_loaded_movie();

    reserve_player_ref(|player| {
        let cast = player
            .movie
            .cast_manager
            .get_cast(CAST_LIB as u32)
            .expect("catalogue cast");
        assert_eq!(cast.name, "External");
    });

    let mut palette_tables: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut input_manifest = Vec::new();
    for spec in BITMAP_INPUTS {
        input_manifest.push(export_bitmap_input(&root, spec, &mut palette_tables));
    }

    let mut wall_renders = Vec::new();
    for palette in WALL_PALETTES {
        wall_renders.push(export_palette_render(
            &root,
            "wall",
            Some("left"),
            palette.pattern,
            55,
            "cat_left_wall_1_a_0_0_0",
            51,
            87,
            palette.left_member,
            &format!("cat_left_wall_{}", palette.pattern),
            "wall-renders",
            &mut palette_tables,
        ));
        wall_renders.push(export_palette_render(
            &root,
            "wall",
            Some("right"),
            palette.pattern,
            56,
            "cat_right_wall_1_a_0_0_0",
            55,
            88,
            palette.right_member,
            &format!("cat_right_wall_{}", palette.pattern),
            "wall-renders",
            &mut palette_tables,
        ));
    }

    let mut floor_renders = Vec::new();
    for (pattern, palette_member) in FLOOR_PALETTES {
        floor_renders.push(export_palette_render(
            &root,
            "floor",
            None,
            pattern,
            59,
            "studiofloor_1_preview",
            106,
            44,
            palette_member,
            &format!("cat_floor_{}", pattern),
            "floor-renders",
            &mut palette_tables,
        ));
    }

    let floor_shape = export_floor_shape(&root);
    let diagnostic = export_diagnostic_bitmap(&root, 61, "floor_shape_bitmap", 106, 46);

    assert_eq!(input_manifest.len(), 6);
    assert_eq!(wall_renders.len(), 20);
    assert_eq!(floor_renders.len(), 15);
    assert_eq!(palette_tables.len(), 35);

    let manifest = serde_json::json!({
        "version": 1,
        "sourceCct": CCT_NAME,
        "standaloneCastName": "External",
        "engineContextCastName": "Catalogue",
        "composition": {
            "width": 105,
            "height": 105,
            "rightWallOffset": [50, 0],
            "floorOffset": [0, 61],
        },
        "inputEncoding": {
            "indexed": "One decoded Director palette index per byte, row-major, width*height bytes. Values are not reconstructed from RGBA.",
            "rgba": "Decoded RGBA8, row-major, width*height*4 bytes; matches the exported PNG pixels.",
        },
        "bitmapInputs": input_manifest,
        "wallRenders": wall_renders,
        "floorRenders": floor_renders,
        "floorShape": floor_shape,
        "diagnostics": [diagnostic],
    });
    fs::write(
        format!("{}/_catalogue_preview_assets.json", root),
        serde_json::to_string_pretty(&manifest).expect("serialize catalogue preview manifest"),
    )
    .expect("write catalogue preview manifest");
    fs::write(
        format!("{}/_palettes.json", root),
        palettes_manifest_json("dump_catalogue_preview_assets", &palette_tables),
    )
    .expect("write catalogue preview palettes");

    println!();
    println!("=== Catalogue preview extraction summary ===");
    println!("  2 RGBA room masks + 4 indexed wall controls");
    println!("  20 wall palette renders");
    println!("  15 floor palette renders");
    println!("  floor_shape_preview geometry + raw FLSH payload");
    println!("  {} resolved palette tables", palette_tables.len());
    println!("  output -> {}", root);
}

fn export_bitmap_input(
    root: &str,
    spec: BitmapInput,
    palette_tables: &mut BTreeMap<String, serde_json::Value>,
) -> serde_json::Value {
    let (name, width, height, bit_depth, original_bit_depth, use_alpha, palette_ref, data, json) =
        reserve_player_ref(|player| {
            let cast = player
                .movie
                .cast_manager
                .get_cast(CAST_LIB as u32)
                .expect("catalogue cast");
            let member = cast
                .members
                .get(&(spec.member as u32))
                .expect("indexed member");
            assert_eq!(member.name, spec.name);
            let bitmap_member = member.member_type.as_bitmap().expect("indexed member type");
            let bitmap = player
                .bitmap_manager
                .get_bitmap(bitmap_member.image_ref)
                .expect("indexed bitmap data");
            (
                member.name.clone(),
                bitmap.width,
                bitmap.height,
                bitmap.bit_depth,
                bitmap.original_bit_depth,
                bitmap.use_alpha,
                format!("{:?}", bitmap.palette_ref),
                bitmap.data.clone(),
                mcp_get_cast_member_picture(player, CAST_LIB, spec.member),
            )
        });

    assert_eq!((width, height), (spec.width, spec.height));
    let picture = parse_picture(&json, spec.name, width, height);

    let stem = file_stem(spec.name);
    let png_file = format!("inputs/{}.png", stem);
    let png = decode_picture_png(&picture);
    fs::write(format!("{}/{}", root, png_file), &png).expect("write input preview png");

    let (encoding, data_file) = if spec.indexed {
        assert_eq!(bit_depth, 8, "{} current bit depth", spec.name);
        assert_eq!(original_bit_depth, 8, "{} original bit depth", spec.name);
        assert_eq!(data.len(), width as usize * height as usize);
        assert_index_roundtrip(&picture, &data, width, height);
        collect_palette_table(palette_tables, &picture);
        let file = format!("inputs/{}.indices.bin", stem);
        fs::write(format!("{}/{}", root, file), &data).expect("write index plane");
        ("indexed8", file)
    } else {
        assert_eq!(bit_depth, 32, "{} decoded bit depth", spec.name);
        assert_eq!(original_bit_depth, 16, "{} original bit depth", spec.name);
        assert_eq!(data.len(), width as usize * height as usize * 4);
        let rgba = image::load_from_memory(&png)
            .expect("decode RGBA input png")
            .to_rgba8();
        assert_eq!(rgba.dimensions(), (width as u32, height as u32));
        let file = format!("inputs/{}.rgba.bin", stem);
        fs::write(format!("{}/{}", root, file), rgba.as_raw()).expect("write RGBA plane");
        ("rgba8", file)
    };

    serde_json::json!({
        "role": spec.role,
        "castMember": spec.member,
        "name": name,
        "width": width,
        "height": height,
        "bitDepth": bit_depth,
        "originalBitDepth": original_bit_depth,
        "useAlpha": use_alpha,
        "paletteRef": palette_ref,
        "encoding": encoding,
        "dataFile": data_file,
        "previewFile": png_file,
    })
}

#[allow(clippy::too_many_arguments)]
fn export_palette_render(
    root: &str,
    surface: &str,
    direction: Option<&str>,
    pattern: &str,
    source_member: i32,
    source_name: &str,
    width: u16,
    height: u16,
    palette_member: i32,
    palette_name: &str,
    output_directory: &str,
    palette_tables: &mut BTreeMap<String, serde_json::Value>,
) -> serde_json::Value {
    reserve_player_ref(|player| {
        let cast = player
            .movie
            .cast_manager
            .get_cast(CAST_LIB as u32)
            .expect("catalogue cast");
        let source = cast
            .members
            .get(&(source_member as u32))
            .expect("render source");
        assert_eq!(source.name, source_name);
        assert!(matches!(source.member_type, CastMemberType::Bitmap(_)));
        let palette = cast
            .members
            .get(&(palette_member as u32))
            .expect("render palette");
        assert_eq!(palette.name, palette_name);
        assert!(matches!(palette.member_type, CastMemberType::Palette(_)));
    });

    let json = reserve_player_ref(|player| {
        mcp_get_cast_member_picture_with_palette(
            player,
            CAST_LIB,
            source_member,
            CAST_LIB,
            palette_member,
        )
    });
    let picture = parse_picture(&json, source_name, width, height);
    let palette = picture.get("palette").expect("render palette table");
    assert_eq!(
        palette.get("kind").and_then(|value| value.as_str()),
        Some("member")
    );
    assert_eq!(
        palette.get("castMember").and_then(|value| value.as_i64()),
        Some(palette_member as i64)
    );
    assert_eq!(
        palette.get("memberName").and_then(|value| value.as_str()),
        Some(palette_name)
    );
    assert_eq!(
        palette.get("resolution").and_then(|value| value.as_str()),
        Some("exact")
    );
    assert_eq!(
        palette
            .get("entries")
            .and_then(|value| value.as_array())
            .map(Vec::len),
        Some(256)
    );
    collect_palette_table(palette_tables, &picture);

    let file = match direction {
        Some(direction) => format!("{}/{}_{}.png", output_directory, pattern, direction),
        None => format!("{}/{}.png", output_directory, pattern),
    };
    fs::write(format!("{}/{}", root, file), decode_picture_png(&picture))
        .expect("write palette render");

    serde_json::json!({
        "surface": surface,
        "direction": direction,
        "pattern": pattern,
        "sourceMember": source_member,
        "sourceName": source_name,
        "paletteMember": palette_member,
        "paletteName": palette_name,
        "paletteRef": picture.get("palette_ref"),
        "width": width,
        "height": height,
        "file": file,
    })
}

fn export_floor_shape(root: &str) -> serde_json::Value {
    let (name, raw) = reserve_player_ref(|player| {
        let cast = player
            .movie
            .cast_manager
            .get_cast(CAST_LIB as u32)
            .expect("catalogue cast");
        let member = cast.members.get(&60).expect("floor shape member");
        assert_eq!(member.name, "floor_shape_preview");
        let CastMemberType::VectorShape(_) = &member.member_type else {
            panic!("floor_shape_preview is not a vectorShape");
        };
        let raw = player
            .movie
            .file
            .as_ref()
            .and_then(|file| file.casts.first())
            .and_then(|cast| cast.members.get(&60))
            .map(|member| member.chunk.specific_data_raw.clone())
            .expect("floor shape raw specific data");
        (member.name.clone(), raw)
    });

    let flsh = flsh_payload(&raw);
    let shape = parse_flsh_shape(flsh);
    assert_eq!(shape.member_width, 106);
    assert_eq!(shape.member_height, 46);
    assert_eq!(shape.fill_mode, 2);
    assert!(shape.closed);
    assert_eq!(shape.gradient_type, "radial");
    assert_eq!(shape.fill_scale, 210.0);
    assert_eq!(shape.fill_direction, 288.0);
    assert_eq!(shape.fill_offset, (-80, 80));
    assert_eq!(shape.fill_cycles, 1);
    assert_eq!(shape.fill_color, (0xFF, 0x99, 0xFF));
    assert_eq!(shape.end_color, (0xBB, 0x33, 0xAA));
    assert!(shape.vertices.len() >= 3);
    assert!(shape.vertices.iter().all(|vertex| {
        vertex.handle1_x == 0.0
            && vertex.handle1_y == 0.0
            && vertex.handle2_x == 0.0
            && vertex.handle2_y == 0.0
    }));

    let raw_file = "floor-shape/floor_shape_preview.flsh.bin";
    fs::write(format!("{}/{}", root, raw_file), flsh).expect("write floor shape FLSH");

    let reference = render_floor_gradient(&shape, FLOOR_GOLDEN_START, FLOOR_GOLDEN_END);
    let golden_json =
        reserve_player_ref(|player| mcp_get_cast_member_picture(player, CAST_LIB, 61));
    let golden_picture = parse_picture(&golden_json, "floor_shape_bitmap", 106, 46);
    let golden = image::load_from_memory(&decode_picture_png(&golden_picture))
        .expect("decode floor-shape Director golden")
        .to_rgba8();
    let comparison = compare_floor_gradient(&shape, &reference, &golden);

    let reference_file = "floor-shape/floor_shape_reference_orgun_2.png";
    let mut reference_png = Vec::new();
    image::DynamicImage::ImageRgba8(reference)
        .write_to(
            &mut std::io::Cursor::new(&mut reference_png),
            image::ImageFormat::Png,
        )
        .expect("encode floor-shape reference PNG");
    fs::write(format!("{}/{}", root, reference_file), reference_png)
        .expect("write floor-shape reference PNG");

    let geometry_file = "floor-shape/floor_shape_preview.json";
    let geometry = serde_json::json!({
        "castMember": 60,
        "name": name,
        "width": shape.member_width,
        "height": shape.member_height,
        "strokeColor": rgb(shape.stroke_color),
        "fillColor": rgb(shape.fill_color),
        "backgroundColor": rgb(shape.bg_color),
        "endColor": rgb(shape.end_color),
        "strokeWidth": shape.stroke_width,
        "fillMode": shape.fill_mode,
        "gradientType": shape.gradient_type,
        "fillScale": shape.fill_scale,
        "fillDirection": shape.fill_direction,
        "fillOffset": [shape.fill_offset.0, shape.fill_offset.1],
        "fillCycles": shape.fill_cycles,
        "closed": shape.closed,
        "vertices": shape.vertices.iter().map(|vertex| serde_json::json!({
            "x": vertex.x,
            "y": vertex.y,
            "outgoing": [vertex.handle1_x, vertex.handle1_y],
            "incoming": [vertex.handle2_x, vertex.handle2_y],
        })).collect::<Vec<_>>(),
        "rawFlshFile": raw_file,
        "rawFlshBytes": flsh.len(),
        "referenceCompositor": {
            "algorithm": "Director radial gradient: center + fillOffset, radius half-diagonal * fillScale / 100, rounded RGB interpolation.",
            "file": reference_file,
            "startColor": rgb(FLOOR_GOLDEN_START),
            "endColor": rgb(FLOOR_GOLDEN_END),
            "colorSource": "Second cat_floorpattern_orgun record.",
            "goldenMember": 61,
            "goldenName": "floor_shape_bitmap",
            "comparison": comparison,
        },
    });
    fs::write(
        format!("{}/{}", root, geometry_file),
        serde_json::to_string_pretty(&geometry).expect("serialize floor shape geometry"),
    )
    .expect("write floor shape geometry");

    serde_json::json!({
        "castMember": 60,
        "name": "floor_shape_preview",
        "type": "vectorShape",
        "width": shape.member_width,
        "height": shape.member_height,
        "geometryFile": geometry_file,
        "rawFlshFile": raw_file,
        "referenceFile": reference_file,
    })
}

fn export_diagnostic_bitmap(
    root: &str,
    member: i32,
    name: &str,
    width: u16,
    height: u16,
) -> serde_json::Value {
    let json = reserve_player_ref(|player| {
        let cast = player
            .movie
            .cast_manager
            .get_cast(CAST_LIB as u32)
            .expect("catalogue cast");
        let diagnostic = cast
            .members
            .get(&(member as u32))
            .expect("diagnostic member");
        assert_eq!(diagnostic.name, name);
        assert!(matches!(diagnostic.member_type, CastMemberType::Bitmap(_)));
        mcp_get_cast_member_picture(player, CAST_LIB, member)
    });
    let picture = parse_picture(&json, name, width, height);
    let file = format!("diagnostics/{}.png", name);
    fs::write(format!("{}/{}", root, file), decode_picture_png(&picture))
        .expect("write diagnostic bitmap");
    serde_json::json!({
        "castMember": member,
        "name": name,
        "file": file,
        "status": "Not referenced by WFpreview; Director-authored golden for floor_shape_preview radial-gradient comparison.",
    })
}

fn parse_picture(json: &str, expected_name: &str, width: u16, height: u16) -> serde_json::Value {
    let picture: serde_json::Value = serde_json::from_str(json).expect("parse MCP picture JSON");
    if let Some(error) = picture.get("error").and_then(|value| value.as_str()) {
        panic!("{} export failed: {}", expected_name, error);
    }
    assert_eq!(
        picture.get("name").and_then(|value| value.as_str()),
        Some(expected_name)
    );
    assert_eq!(
        picture.get("width").and_then(|value| value.as_u64()),
        Some(width as u64)
    );
    assert_eq!(
        picture.get("height").and_then(|value| value.as_u64()),
        Some(height as u64)
    );
    picture
}

fn decode_picture_png(picture: &serde_json::Value) -> Vec<u8> {
    use base64::Engine;
    let encoded = picture
        .get("png_base64")
        .and_then(|value| value.as_str())
        .expect("picture png_base64");
    base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .expect("decode picture png")
}

fn assert_index_roundtrip(picture: &serde_json::Value, indices: &[u8], width: u16, height: u16) {
    let palette = picture
        .get("palette")
        .and_then(|value| value.get("entries"))
        .and_then(|value| value.as_array())
        .expect("indexed palette entries");
    assert_eq!(palette.len(), 256);
    let decoded = image::load_from_memory(&decode_picture_png(picture))
        .expect("decode indexed preview png")
        .to_rgba8();
    assert_eq!(decoded.dimensions(), (width as u32, height as u32));

    let mut rgb = HashMap::new();
    for index in indices
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
    {
        let value = palette[index as usize].as_str().expect("palette RGB");
        rgb.insert(index, parse_rgb(value));
    }
    for (offset, index) in indices.iter().enumerate() {
        let x = (offset % width as usize) as u32;
        let y = (offset / width as usize) as u32;
        let pixel = decoded.get_pixel(x, y).0;
        assert_eq!(
            &pixel[..3],
            &rgb[index][..],
            "palette round-trip mismatch at {},{}",
            x,
            y
        );
    }
}

fn parse_rgb(value: &str) -> [u8; 3] {
    assert_eq!(value.len(), 6);
    [
        u8::from_str_radix(&value[0..2], 16).expect("red hex"),
        u8::from_str_radix(&value[2..4], 16).expect("green hex"),
        u8::from_str_radix(&value[4..6], 16).expect("blue hex"),
    ]
}

fn rgb(value: (u8, u8, u8)) -> String {
    format!("#{:02X}{:02X}{:02X}", value.0, value.1, value.2)
}

fn file_stem(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn parse_flsh_shape(data: &[u8]) -> FlshShape {
    let read_u32 = |offset: usize| -> u32 {
        let bytes: [u8; 4] = data
            .get(offset..offset + 4)
            .expect("FLSH u32")
            .try_into()
            .expect("FLSH u32 bytes");
        u32::from_be_bytes(bytes)
    };
    let read_f32 = |offset: usize| f32::from_bits(read_u32(offset));
    let parse_color = |offset: usize| {
        (
            read_u32(offset + 4) as u8,
            read_u32(offset + 8) as u8,
            read_u32(offset + 12) as u8,
        )
    };

    // The inline list count is authoritative; later entries reference the
    // symbol roles named by the first vertex instead of repeating strings.
    let entry_count = read_u32(228) as usize;
    let mut position = 232;
    let mut symbol_roles = Vec::new();
    let mut vertices = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        let entry_type = read_u32(position);
        let item_count = read_u32(position + 4) as usize;
        position += 8;
        let mut vertex = (0, 0);
        let mut handle1 = (0, 0);
        let mut handle2 = (0, 0);
        let mut new_curve = entry_type == 0x07;
        for _ in 0..item_count {
            let (role, point) = parse_flsh_point(data, &mut position, &mut symbol_roles);
            match role {
                VertexRole::Vertex => vertex = point,
                VertexRole::Handle1 => handle1 = point,
                VertexRole::Handle2 => handle2 = point,
                VertexRole::NewCurve => new_curve = true,
                VertexRole::Unknown => {}
            }
        }
        if !new_curve {
            vertices.push(FlshVertex {
                x: vertex.0 as f32,
                y: vertex.1 as f32,
                handle1_x: handle1.0 as f32,
                handle1_y: handle1.1 as f32,
                handle2_x: handle2.0 as f32,
                handle2_y: handle2.1 as f32,
            });
        }
    }

    FlshShape {
        stroke_color: parse_color(160),
        fill_color: parse_color(176),
        bg_color: parse_color(192),
        end_color: parse_color(208),
        stroke_width: read_f32(0x80),
        fill_mode: read_u32(0x84),
        closed: read_u32(0x7C) != 0,
        member_width: read_u32(0x24),
        member_height: read_u32(0x20),
        gradient_type: match read_u32(0x88) {
            0 => "linear",
            1 => "radial",
            value => panic!("unsupported gradient type {value}"),
        },
        fill_scale: read_f32(0x8C),
        fill_direction: read_f32(0x90),
        fill_offset: (read_u32(0x98) as i32, read_u32(0x94) as i32),
        fill_cycles: read_u32(0x9C) as i32,
        vertices,
    }
}

fn parse_flsh_point(
    data: &[u8],
    position: &mut usize,
    symbol_roles: &mut Vec<VertexRole>,
) -> (VertexRole, (i32, i32)) {
    let read_u32 = |offset: usize| -> u32 {
        u32::from_be_bytes(data[offset..offset + 4].try_into().expect("FLSH point u32"))
    };
    let read_i32 = |offset: usize| -> i32 {
        i32::from_be_bytes(data[offset..offset + 4].try_into().expect("FLSH point i32"))
    };

    *position += 4;
    let discriminator = read_u32(*position);
    *position += 4;
    let role = if discriminator & 0x8000_0000 != 0 {
        symbol_roles
            .get((discriminator & 0x7FFF_FFFF) as usize)
            .copied()
            .unwrap_or(VertexRole::Unknown)
    } else {
        let length = discriminator as usize;
        let name =
            std::str::from_utf8(&data[*position..*position + length]).expect("FLSH point key");
        *position += length;
        let role = match name {
            "vertex" => VertexRole::Vertex,
            "handle1" => VertexRole::Handle1,
            "handle2" => VertexRole::Handle2,
            "newCurve" => VertexRole::NewCurve,
            _ => VertexRole::Unknown,
        };
        symbol_roles.push(role);
        role
    };
    if matches!(role, VertexRole::NewCurve) {
        return (role, (0, 0));
    }
    *position += 4;
    let y = read_i32(*position);
    *position += 4;
    let x = read_i32(*position);
    *position += 4;
    (role, (x, y))
}

fn floor_polygon(shape: &FlshShape) -> Vec<(f32, f32)> {
    let left = shape
        .vertices
        .iter()
        .map(|vertex| vertex.x)
        .fold(f32::MAX, f32::min);
    let top = shape
        .vertices
        .iter()
        .map(|vertex| vertex.y)
        .fold(f32::MAX, f32::min);
    shape
        .vertices
        .iter()
        .map(|vertex| (vertex.x - left, vertex.y - top))
        .collect()
}

fn point_in_polygon(polygon: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let mut previous = polygon.len() - 1;
    for current in 0..polygon.len() {
        let (current_x, current_y) = polygon[current];
        let (previous_x, previous_y) = polygon[previous];
        if (current_y > y) != (previous_y > y)
            && x < (previous_x - current_x) * (y - current_y) / (previous_y - current_y + 1e-9)
                + current_x
        {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

fn render_floor_gradient(
    shape: &FlshShape,
    start: (u8, u8, u8),
    end: (u8, u8, u8),
) -> image::RgbaImage {
    let width = shape.member_width;
    let height = shape.member_height;
    let polygon = floor_polygon(shape);
    let origin = (
        width as f32 / 2.0 + shape.fill_offset.0 as f32,
        height as f32 / 2.0 + shape.fill_offset.1 as f32,
    );
    let radius =
        ((width as f32).powi(2) + (height as f32).powi(2)).sqrt() / 2.0 * shape.fill_scale / 100.0;
    let interpolate = |from: u8, to: u8, t: f32| {
        (from as f32 * (1.0 - t) + to as f32 * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };

    image::RgbaImage::from_fn(width, height, |x, y| {
        if !point_in_polygon(&polygon, x as f32 + 0.5, y as f32 + 0.5) {
            return image::Rgba([255, 255, 255, 255]);
        }
        let dx = x as f32 + 0.5 - origin.0;
        let dy = y as f32 + 0.5 - origin.1;
        // fillDirection does not rotate a circular radial gradient.
        let t = ((dx * dx + dy * dy).sqrt() / radius).clamp(0.0, 1.0);
        image::Rgba([
            interpolate(start.0, end.0, t),
            interpolate(start.1, end.1, t),
            interpolate(start.2, end.2, t),
            255,
        ])
    })
}

fn compare_floor_gradient(
    shape: &FlshShape,
    reference: &image::RgbaImage,
    golden: &image::RgbaImage,
) -> serde_json::Value {
    assert_eq!(reference.dimensions(), golden.dimensions());
    let polygon = floor_polygon(shape);
    let mut samples = 0u64;
    let mut error_sum = 0u64;
    let mut max_error = 0u8;
    for y in 1..shape.member_height - 1 {
        for x in 1..shape.member_width - 1 {
            let interior = [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)]
                .iter()
                .all(|(dx, dy)| {
                    point_in_polygon(
                        &polygon,
                        x as f32 + *dx as f32 + 0.5,
                        y as f32 + *dy as f32 + 0.5,
                    )
                });
            if !interior {
                continue;
            }
            let actual = reference.get_pixel(x, y).0;
            let expected = golden.get_pixel(x, y).0;
            for channel in 0..3 {
                let error = actual[channel].abs_diff(expected[channel]);
                max_error = max_error.max(error);
                error_sum += error as u64;
                samples += 1;
            }
        }
    }
    let mean_error = error_sum as f64 / samples as f64;
    assert!(
        samples > 8_000,
        "insufficient floor-gradient golden samples"
    );
    assert!(
        max_error <= 40,
        "floor-gradient max error {max_error}, mean error {mean_error}, samples {samples}"
    );
    assert!(mean_error <= 8.0, "floor-gradient mean error {mean_error}");
    serde_json::json!({
        "sampledChannels": samples,
        "maxChannelError": max_error,
        "meanChannelError": mean_error,
        "maxChannelErrorGate": 40,
        "meanChannelErrorGate": 8.0,
    })
}

fn flsh_payload(raw: &[u8]) -> &[u8] {
    assert!(raw.len() >= 15, "vectorShape specific data too short");
    let string_length = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
    let flsh_start = 4 + string_length;
    assert_eq!(&raw[4..flsh_start], b"vectorShape");
    assert!(raw.len() >= flsh_start + 8);
    assert_eq!(&raw[flsh_start + 4..flsh_start + 8], b"FLSH");
    &raw[flsh_start + 8..]
}
