// MCP (Model Context Protocol) query functions for VM debugging
// These functions return JSON strings for use with the MCP server

use std::rc::Rc;

use fxhash::FxHashMap;
use serde::Serialize;

use crate::{director::{
    chunks::score::ScoreFrameChannelData,
    enums::ScriptType,
    file::get_variable_multiplier,
    lingo::{decompiler::handler::decompile_handler, script::ScriptContext as LingoScriptContext},
}, player::datum_formatting::format_concrete_datum_with_depth};

use super::{
    allocator::{DatumAllocatorTrait, ScriptInstanceAllocatorTrait},
    cast_lib::{CastLib, CastMemberRef},
    cast_member::CastMemberType,
    datum_ref::DatumId,
    score::get_channel_number_from_index,
    script::Script,
    DirPlayer,
};

// ============================================================================
// Response types for MCP tools
// ============================================================================

#[derive(Serialize)]
pub struct McpScriptInfo {
    pub cast_lib: i32,
    pub cast_member: i32,
    pub name: String,
    pub script_type: String,
    pub handlers: Vec<String>,
}

#[derive(Serialize)]
pub struct McpScriptList {
    pub scripts: Vec<McpScriptInfo>,
    pub total_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct McpScriptDetails {
    pub cast_lib: i32,
    pub cast_member: i32,
    pub name: String,
    pub script_type: String,
    pub handlers: Vec<McpHandlerInfo>,
    pub properties: Vec<String>,
}

#[derive(Serialize)]
pub struct McpHandlerInfo {
    pub name: String,
    pub arguments: Vec<String>,
    pub locals: Vec<String>,
    pub bytecode_count: usize,
}

#[derive(Serialize)]
pub struct McpBytecodeInstruction {
    pub pos: usize,
    pub opcode: String,
    pub operand: i64,
    pub text: String,
}

#[derive(Serialize)]
pub struct McpDisassemblyResult {
    pub handler_name: String,
    pub arguments: Vec<String>,
    pub bytecode: Vec<McpBytecodeInstruction>,
}

#[derive(Serialize)]
pub struct McpDecompiledLine {
    pub text: String,
    pub indent: u32,
    pub bytecode_indices: Vec<usize>,
}

#[derive(Serialize)]
pub struct McpDecompileResult {
    pub handler_name: String,
    pub arguments: Vec<String>,
    // pub lines: Vec<McpDecompiledLine>,
    pub source: String,
}

#[derive(Serialize)]
pub struct McpScopeInfo {
    pub index: usize,
    pub script_name: String,
    pub cast_lib: i32,
    pub cast_member: i32,
    pub handler_name: String,
    pub bytecode_index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locals: Option<FxHashMap<String, McpDatumValue>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<McpDatumValue>>,
    pub stack_depth: usize,
}

#[derive(Serialize)]
pub struct McpScopeSummary {
    pub index: usize,
    pub script_name: String,
    pub handler_name: String,
    pub bytecode_index: usize,
}

/// Lightweight execution context - single call to get current position
#[derive(Serialize)]
pub struct McpContext {
    pub is_playing: bool,
    pub is_paused: bool,
    pub at_breakpoint: bool,
    pub current_frame: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_handler: Option<McpScopeSummary>,
    pub scope_count: usize,
}

#[derive(Serialize)]
pub struct McpCallStack {
    pub scopes: Vec<McpScopeInfo>,
    pub current_scope_index: Option<usize>,
}

#[derive(Serialize)]
pub struct McpExecutionState {
    pub is_playing: bool,
    pub is_paused: bool,
    pub current_frame: u32,
    pub total_frames: usize,
    pub at_breakpoint: bool,
    pub movie_loaded: bool,
    pub movie_title: String,
    pub stage_width: u32,
    pub stage_height: u32,
}

#[derive(Serialize)]
pub struct McpDatumValue {
    pub datum_id: Option<usize>,
    pub type_name: String,
    pub value: String,
}

#[derive(Serialize)]
pub struct McpGlobalsResult {
    pub globals: FxHashMap<String, McpDatumValue>,
}

#[derive(Serialize)]
pub struct McpLocalsResult {
    pub scope_index: usize,
    pub handler_name: String,
    pub locals: FxHashMap<String, McpDatumValue>,
    pub args: Vec<McpArgInfo>,
}

#[derive(Serialize)]
pub struct McpArgInfo {
    pub name: String,
    pub value: McpDatumValue,
}

#[derive(Serialize)]
pub struct McpDatumInspection {
    pub datum_id: usize,
    pub type_name: String,
    pub value: String,
    pub properties: Option<FxHashMap<String, McpDatumValue>>,
}

#[derive(Serialize)]
pub struct McpCastLibInfo {
    pub number: i32,
    pub name: String,
    pub member_count: usize,
    pub script_count: usize,
}

#[derive(Serialize)]
pub struct McpCastMemberInfo {
    pub cast_lib: i32,
    pub cast_member: i32,
    pub name: String,
    pub member_type: String,
    /// Bitmap-only fields. Zero/false for non-bitmap members.
    /// `reg_x` / `reg_y` are the cast member's registration point; Director
    /// sprites position by regPoint, so downstream renderers need this to
    /// translate `(locH, locV)` from upstream SceneXml into top-left anchors.
    pub reg_x: i16,
    pub reg_y: i16,
    pub bit_depth: u8,
    pub use_alpha: bool,
    pub palette_id: i16,
    pub width: u16,
    pub height: u16,
}

#[derive(Serialize)]
pub struct McpCastMemberDetails {
    pub cast_lib: i32,
    pub cast_member: i32,
    pub name: String,
    pub member_type: String,
    pub script_type: Option<String>,
    pub handlers: Option<Vec<String>>,
}

#[derive(Serialize)]
pub struct McpCastMemberPicture {
    pub cast_lib: i32,
    pub cast_member: i32,
    pub name: String,
    pub width: u16,
    pub height: u16,
    pub bit_depth: u8,
    pub original_bit_depth: u8,
    pub reg_x: i16,
    pub reg_y: i16,
    pub use_alpha: bool,
    pub palette_ref: String,
    /// Length of the underlying RGBA buffer in bytes.
    pub data_len: usize,
    /// First 64 bytes of the bitmap's `data` buffer, hex-encoded — diagnostic.
    pub data_head_hex: String,
    /// Base64-encoded PNG. RGBA pixels resolved via the bitmap's palette.
    pub png_base64: String,
}

#[derive(Serialize)]
pub struct McpFilmLoopFrameMember {
    pub channel: u32,
    pub cast_lib: i32,
    pub cast_member: i32,
    pub name: String,
    /// Director loc_h/loc_v: pixel position in the filmLoop's local
    /// coordinate space. The sprite is anchored at its cast member's
    /// regPoint — the renderer must translate by (-regX, -regY) to get
    /// the top-left draw position.
    pub loc_h: i32,
    pub loc_v: i32,
    /// On-stage display size, resolved with Director's "stretch off →
    /// bitmap natural size" rule already applied. Renderer can use
    /// these directly without consulting member metadata.
    pub width: i32,
    pub height: i32,
    pub rotation: f64,
    pub skew: f64,
    /// Blend opacity, 0..=100 percent (0 = transparent, 100 = opaque).
    /// Already converted from Director's raw byte (D5-D7 direct,
    /// D8+ inverted 0-255).
    pub blend: u8,
    pub ink: u8,
}

#[derive(Serialize)]
pub struct McpFilmLoopFrame {
    pub frame: u32,
    pub duration_ticks: u32,
    pub duration_ms: u32,
    pub members: Vec<McpFilmLoopFrameMember>,
}

#[derive(Serialize)]
pub struct McpFilmLoopSpan {
    pub channel: u32,
    pub start_frame: u32,
    pub end_frame: u32,
}

#[derive(Serialize)]
pub struct McpFilmLoopFrames {
    pub cast_lib: i32,
    pub cast_member: i32,
    pub name: String,
    pub frame_count: u32,
    pub width: u16,
    pub height: u16,
    pub reg_x: i16,
    pub reg_y: i16,
    pub loops: bool,
    pub default_duration_ms: u32,
    pub sprite_spans: Vec<McpFilmLoopSpan>,
    pub frames: Vec<McpFilmLoopFrame>,
}

#[derive(Serialize)]
pub struct McpBreakpointInfo {
    pub script_name: String,
    pub handler_name: String,
    pub bytecode_index: usize,
}

#[derive(Serialize)]
pub struct McpBreakpointList {
    pub breakpoints: Vec<McpBreakpointInfo>,
}

#[derive(Serialize)]
pub struct McpError {
    pub error: String,
}

#[derive(Serialize)]
pub struct McpEvalResult {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datum_id: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ============================================================================
// Helper functions
// ============================================================================

/// Serialize result to JSON, with error fallback
fn to_json<T: Serialize>(result: &T) -> String {
    serde_json::to_string_pretty(result).unwrap_or_else(|e| {
        serde_json::to_string(&McpError { error: e.to_string() }).unwrap()
    })
}

/// Create an error JSON response
fn mcp_error(msg: impl Into<String>) -> String {
    serde_json::to_string(&McpError { error: msg.into() }).unwrap()
}

fn script_type_str(script_type: &ScriptType) -> &'static str {
    match script_type {
        ScriptType::Movie => "movie",
        ScriptType::Parent => "parent",
        ScriptType::Score => "score",
        ScriptType::Member => "member",
        ScriptType::Invalid => "invalid",
        ScriptType::Unknown => "unknown",
    }
}

/// Max length for compact value representation
const COMPACT_VALUE_MAX_LEN: usize = 60;

fn datum_to_mcp_value(player: &DirPlayer, datum_ref: &super::DatumRef) -> McpDatumValue {
    datum_to_mcp_value_with_options(player, datum_ref, false)
}

fn datum_to_mcp_value_compact(player: &DirPlayer, datum_ref: &super::DatumRef) -> McpDatumValue {
    datum_to_mcp_value_with_options(player, datum_ref, true)
}

fn datum_to_mcp_value_with_options(player: &DirPlayer, datum_ref: &super::DatumRef, compact: bool) -> McpDatumValue {
    let datum = player.get_datum(datum_ref);
    let datum_id = match datum_ref {
        super::DatumRef::Void => None,
        super::DatumRef::Ref(id, _) => Some(*id),
    };

    // For compact mode, use short representations for complex types
    let value = if compact {
        format_datum_compact(datum, player, datum_id)
    } else {
        format_concrete_datum_with_depth(datum, player, 0, 0)
    };

    McpDatumValue {
        datum_id,
        type_name: datum.type_str().to_string(),
        value,
    }
}

/// Format datum in compact form for summaries
fn format_datum_compact(datum: &crate::director::lingo::datum::Datum, player: &DirPlayer, datum_id: Option<usize>) -> String {
    use crate::director::lingo::datum::Datum;

    match datum {
        // Simple types - use standard formatting
        Datum::Int(i) => i.to_string(),
        Datum::Float(f) => format!("{}", f),
        Datum::Symbol(s) => format!("#{}", s),
        Datum::Void => "Void".to_string(),
        Datum::Null => "<Null>".to_string(),

        // Strings - truncate if long
        Datum::String(s) => {
            if s.len() > COMPACT_VALUE_MAX_LEN - 2 {
                format!("\"{}...\"", &s[..COMPACT_VALUE_MAX_LEN - 5])
            } else {
                format!("\"{}\"", s)
            }
        },
        Datum::StringChunk(..) => {
            let s = datum.string_value().unwrap_or_default();
            if s.len() > COMPACT_VALUE_MAX_LEN - 2 {
                format!("\"{}...\"", &s[..COMPACT_VALUE_MAX_LEN - 5])
            } else {
                format!("\"{}\"", s)
            }
        },

        // Lists - show count only
        Datum::List(_, items, _) => {
            format!("<list:{}>", items.len())
        },

        // PropLists - show count only
        Datum::PropList(entries, _) => {
            format!("<propList:{}>", entries.len())
        },

        // Script instances - compact form with ID
        Datum::ScriptInstanceRef(instance_ref) => {
            let instance = player.allocator.get_script_instance(instance_ref);
            if let Some(script) = player.movie.cast_manager.get_script_by_ref(&instance.script) {
                format!("<{}:{}>", script.name, instance_ref)
            } else {
                format!("<instance:{}>", instance_ref)
            }
        },

        // Script refs
        Datum::ScriptRef(member_ref) => {
            if let Some(script) = player.movie.cast_manager.get_script_by_ref(member_ref) {
                format!("<script:{}>", script.name)
            } else {
                format!("<script:{},{}>", member_ref.cast_lib, member_ref.cast_member)
            }
        },

        // For other types, use ID-based short form if available
        _ => {
            if let Some(id) = datum_id {
                format!("<{}:{}>", datum.type_str(), id)
            } else {
                format!("<{}>", datum.type_str())
            }
        }
    }
}

fn get_script_info(script: &Script) -> McpScriptInfo {
    McpScriptInfo {
        cast_lib: script.member_ref.cast_lib,
        cast_member: script.member_ref.cast_member,
        name: script.name.clone(),
        script_type: script_type_str(&script.script_type).to_string(),
        handlers: script.handler_names.clone(),
    }
}

/// Helper struct for script context lookups
struct ScriptLookup<'a> {
    script: &'a Rc<Script>,
    cast: &'a CastLib,
    lctx: &'a LingoScriptContext,
    multiplier: u32,
}

/// Look up script, cast, and lctx for a member reference
fn get_script_context<'a>(
    player: &'a DirPlayer,
    member_ref: &CastMemberRef,
) -> Result<ScriptLookup<'a>, String> {
    let script = player
        .movie
        .cast_manager
        .get_script_by_ref(member_ref)
        .ok_or_else(|| {
            format!(
                "Script not found at cast_lib={}, cast_member={}",
                member_ref.cast_lib, member_ref.cast_member
            )
        })?;

    let cast = player
        .movie
        .cast_manager
        .get_cast(member_ref.cast_lib as u32)
        .map_err(|_| format!("Cast library {} not found", member_ref.cast_lib))?;

    let lctx = cast
        .lctx
        .as_ref()
        .ok_or_else(|| "Script context not available".to_string())?;

    Ok(ScriptLookup {
        script,
        cast,
        lctx,
        multiplier: get_variable_multiplier(cast.capital_x, cast.dir_version),
    })
}

/// Get handler name from name ID using lctx
fn get_handler_name(lctx: Option<&LingoScriptContext>, name_id: u16) -> String {
    lctx.and_then(|l| l.names.get(name_id as usize))
        .cloned()
        .unwrap_or_else(|| format!("handler_{}", name_id))
}

/// Get argument names from handler using lctx
fn get_argument_names(lctx: Option<&LingoScriptContext>, arg_ids: &[u16]) -> Vec<String> {
    arg_ids
        .iter()
        .map(|&id| {
            lctx.and_then(|l| l.names.get(id as usize))
                .cloned()
                .unwrap_or_else(|| format!("arg_{}", id))
        })
        .collect()
}

// ============================================================================
// MCP Query Functions
// ============================================================================

/// List scripts in the movie with optional filtering and pagination
///
/// Parameters:
/// - cast_lib: Filter to a specific cast library (None = all)
/// - limit: Maximum number of scripts to return (None = all)
/// - offset: Number of scripts to skip (None = 0)
pub fn mcp_list_scripts(
    player: &DirPlayer,
    cast_lib: Option<i32>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> String {
    let all_scripts: Vec<McpScriptInfo> = player
        .movie
        .cast_manager
        .casts
        .iter()
        .filter(|cast| cast_lib.map_or(true, |lib| cast.number as i32 == lib))
        .flat_map(|cast| cast.scripts.values().map(|s| get_script_info(s)))
        .collect();

    let total_count = all_scripts.len();
    let offset_val = offset.unwrap_or(0);

    let scripts: Vec<McpScriptInfo> = all_scripts
        .into_iter()
        .skip(offset_val)
        .take(limit.unwrap_or(usize::MAX))
        .collect();

    to_json(&McpScriptList {
        scripts,
        total_count,
        offset: if offset_val > 0 { Some(offset_val) } else { None },
        limit,
    })
}

/// Get detailed information about a specific script
pub fn mcp_get_script(player: &DirPlayer, cast_lib: i32, cast_member: i32) -> String {
    let member_ref = CastMemberRef { cast_lib, cast_member };

    let ctx = match get_script_context(player, &member_ref) {
        Ok(c) => c,
        Err(e) => return mcp_error(e),
    };

    let handlers: Vec<McpHandlerInfo> = ctx
        .script
        .handlers
        .iter()
        .map(|(name, handler)| McpHandlerInfo {
            name: name.to_string(),
            arguments: get_argument_names(Some(ctx.lctx), &handler.argument_name_ids),
            locals: handler
                .local_name_ids
                .iter()
                .map(|&id| {
                    ctx.lctx
                        .names
                        .get(id as usize)
                        .cloned()
                        .unwrap_or_else(|| format!("local_{}", id))
                })
                .collect(),
            bytecode_count: handler.bytecode_array.len(),
        })
        .collect();

    to_json(&McpScriptDetails {
        cast_lib: ctx.script.member_ref.cast_lib,
        cast_member: ctx.script.member_ref.cast_member,
        name: ctx.script.name.clone(),
        script_type: script_type_str(&ctx.script.script_type).to_string(),
        handlers,
        properties: ctx.script.properties.borrow().keys().map(|k| k.to_string()).collect(),
    })
}

/// Disassemble a handler (show bytecode)
pub fn mcp_disassemble_handler(
    player: &DirPlayer,
    cast_lib: i32,
    cast_member: i32,
    handler_name: &str,
) -> String {
    let member_ref = CastMemberRef { cast_lib, cast_member };

    let ctx = match get_script_context(player, &member_ref) {
        Ok(c) => c,
        Err(e) => return mcp_error(e),
    };

    let handler = match ctx.script.get_own_handler(&handler_name.to_lowercase()) {
        Some(h) => h,
        None => return mcp_error(format!("Handler '{}' not found in script", handler_name)),
    };

    to_json(&McpDisassemblyResult {
        handler_name: handler_name.to_string(),
        arguments: get_argument_names(Some(ctx.lctx), &handler.argument_name_ids),
        bytecode: handler
            .bytecode_array
            .iter()
            .map(|bc| McpBytecodeInstruction {
                pos: bc.pos,
                opcode: format!("{:?}", bc.opcode),
                operand: bc.obj,
                text: bc.to_bytecode_text(ctx.lctx, &handler, ctx.multiplier),
            })
            .collect(),
    })
}

/// Decompile a handler (show Lingo source)
pub fn mcp_decompile_handler(
    player: &DirPlayer,
    cast_lib: i32,
    cast_member: i32,
    handler_name: &str,
) -> String {
    let member_ref = CastMemberRef { cast_lib, cast_member };

    let ctx = match get_script_context(player, &member_ref) {
        Ok(c) => c,
        Err(e) => return mcp_error(e),
    };

    let handler = match ctx.script.get_own_handler(&handler_name.to_lowercase()) {
        Some(h) => h,
        None => return mcp_error(format!("Handler '{}' not found in script", handler_name)),
    };

    let decompiled = decompile_handler(
        &handler,
        &ctx.script.chunk,
        ctx.lctx,
        ctx.cast.dir_version,
        ctx.multiplier,
    );

    // Build full source with indentation
    let source = decompiled
        .lines
        .iter()
        .map(|line| format!("{}{}", "  ".repeat(line.indent as usize), line.text))
        .collect::<Vec<_>>()
        .join("\n");

    to_json(&McpDecompileResult {
        handler_name: decompiled.name,
        arguments: decompiled.arguments,
        // lines: decompiled
        //     .lines
        //     .iter()
        //     .map(|line| McpDecompiledLine {
        //         text: line.text.clone(),
        //         indent: line.indent,
        //         bytecode_indices: line.bytecode_indices.clone(),
        //     })
        //     .collect(),
        source,
    })
}

/// Get the current call stack
///
/// Parameters:
/// - depth: Maximum number of scopes to return from the top (None = all)
/// - include_locals: Whether to include local variables and arguments (default: false)
pub fn mcp_get_call_stack(player: &DirPlayer, depth: Option<usize>, include_locals: bool) -> String {
    // Filter out placeholder scopes (cast_lib == 0 && cast_member == 0)
    let valid_scopes: Vec<_> = player
        .scopes
        .iter()
        .enumerate()
        .filter(|(_, scope)| scope.script_ref.cast_lib != 0 || scope.script_ref.cast_member != 0)
        .collect();

    let total_count = valid_scopes.len();

    // Apply depth limit from the top of the stack
    let scopes_to_process: Vec<_> = match depth {
        Some(d) if d < total_count => valid_scopes.into_iter().rev().take(d).rev().collect(),
        _ => valid_scopes,
    };

    let scopes: Vec<McpScopeInfo> = scopes_to_process
        .into_iter()
        .map(|(index, scope)| {
            let script_name = player
                .movie
                .cast_manager
                .get_script_by_ref(&scope.script_ref)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "unknown".to_string());

            let lctx = player
                .movie
                .cast_manager
                .get_cast(scope.script_ref.cast_lib as u32)
                .ok()
                .and_then(|c| c.lctx.as_ref());

            McpScopeInfo {
                index,
                script_name,
                cast_lib: scope.script_ref.cast_lib,
                cast_member: scope.script_ref.cast_member,
                handler_name: get_handler_name(lctx, scope.handler_name_id),
                bytecode_index: scope.bytecode_index,
                locals: if include_locals {
                    Some(scope
                        .locals
                        .iter()
                        .map(|(name_id, datum_ref)| {
                            let name = lctx.and_then(|l| l.names.get(*name_id as usize))
                                .cloned()
                                .unwrap_or_else(|| format!("local_{}", name_id));
                            (name, datum_to_mcp_value_compact(player, datum_ref))
                        })
                        .collect())
                } else {
                    None
                },
                args: if include_locals {
                    Some(scope
                        .args
                        .iter()
                        .map(|datum_ref| datum_to_mcp_value_compact(player, datum_ref))
                        .collect())
                } else {
                    None
                },
                stack_depth: scope.stack.len(),
            }
        })
        .collect();

    to_json(&McpCallStack {
        current_scope_index: if scopes.is_empty() { None } else { Some(scopes.len() - 1) },
        scopes,
    })
}

/// Get lightweight execution context - current position only
pub fn mcp_get_context(player: &DirPlayer) -> String {
    // Filter out placeholder scopes
    let valid_scopes: Vec<_> = player
        .scopes
        .iter()
        .enumerate()
        .filter(|(_, scope)| scope.script_ref.cast_lib != 0 || scope.script_ref.cast_member != 0)
        .collect();

    let current_handler = valid_scopes.last().map(|(index, scope)| {
        let script_name = player
            .movie
            .cast_manager
            .get_script_by_ref(&scope.script_ref)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "unknown".to_string());

        let lctx = player
            .movie
            .cast_manager
            .get_cast(scope.script_ref.cast_lib as u32)
            .ok()
            .and_then(|c| c.lctx.as_ref());

        McpScopeSummary {
            index: *index,
            script_name,
            handler_name: get_handler_name(lctx, scope.handler_name_id),
            bytecode_index: scope.bytecode_index,
        }
    });

    to_json(&McpContext {
        is_playing: player.is_playing,
        is_paused: player.is_script_paused,
        at_breakpoint: player.current_breakpoint.is_some(),
        current_frame: player.movie.current_frame,
        current_handler,
        scope_count: valid_scopes.len(),
    })
}

/// Get execution state
pub fn mcp_get_execution_state(player: &DirPlayer) -> String {
    to_json(&McpExecutionState {
        is_playing: player.is_playing,
        is_paused: player.is_script_paused,
        current_frame: player.movie.current_frame,
        total_frames: player
            .movie
            .score
            .sprite_spans
            .iter()
            .map(|span| span.end_frame as usize)
            .max()
            .unwrap_or(0),
        at_breakpoint: player.current_breakpoint.is_some(),
        movie_loaded: !player.movie.score.sprite_spans.is_empty(),
        movie_title: player.title.clone(),
        stage_width: player.stage_size.0,
        stage_height: player.stage_size.1,
    })
}

/// Get all global variables
pub fn mcp_get_globals(player: &DirPlayer) -> String {
    to_json(&McpGlobalsResult {
        globals: player
            .globals
            .iter()
            .map(|(name, datum_ref)| (name.clone(), datum_to_mcp_value(player, datum_ref)))
            .collect(),
    })
}

/// Get locals for a specific scope
pub fn mcp_get_locals(player: &DirPlayer, scope_index: Option<usize>) -> String {
    let index = scope_index.unwrap_or_else(|| player.scopes.len().saturating_sub(1));

    let scope = match player.scopes.get(index) {
        Some(s) => s,
        None => return mcp_error(format!("Scope index {} not found", index)),
    };

    let lctx = player
        .movie
        .cast_manager
        .get_cast(scope.script_ref.cast_lib as u32)
        .ok()
        .and_then(|c| c.lctx.as_ref());

    // Get argument names from handler definition
    let arg_names: Vec<String> = player
        .movie
        .cast_manager
        .get_script_by_ref(&scope.script_ref)
        .and_then(|script| script.get_own_handler_by_name_id(scope.handler_name_id))
        .map(|handler| get_argument_names(lctx, &handler.argument_name_ids))
        .unwrap_or_default();

    to_json(&McpLocalsResult {
        scope_index: index,
        handler_name: get_handler_name(lctx, scope.handler_name_id),
        locals: scope
            .locals
            .iter()
            .map(|(name_id, datum_ref)| {
                let name = lctx.and_then(|l| l.names.get(*name_id as usize))
                    .cloned()
                    .unwrap_or_else(|| format!("local_{}", name_id));
                (name, datum_to_mcp_value(player, datum_ref))
            })
            .collect(),
        args: scope
            .args
            .iter()
            .enumerate()
            .map(|(i, datum_ref)| McpArgInfo {
                name: arg_names.get(i).cloned().unwrap_or_else(|| format!("arg{}", i)),
                value: datum_to_mcp_value(player, datum_ref),
            })
            .collect(),
    })
}

/// Inspect a datum by ID
pub fn mcp_inspect_datum(player: &DirPlayer, datum_id: DatumId) -> String {
    let datum_ref = match player.allocator.get_datum_ref(datum_id) {
        Some(r) => r,
        None => return mcp_error(format!("Datum with ID {} not found", datum_id)),
    };

    let datum = player.get_datum(&datum_ref);

    // For script instances and prop lists, include properties
    let properties = match datum {
        crate::director::lingo::datum::Datum::ScriptInstanceRef(instance_ref) => Some(
            player
                .allocator
                .get_script_instance(instance_ref)
                .properties
                .iter()
                .map(|(name, datum_ref)| (name.to_string(), datum_to_mcp_value(player, datum_ref)))
                .collect(),
        ),
        crate::director::lingo::datum::Datum::PropList(entries, _) => Some(
            entries
                .iter()
                .map(|(k, v)| {
                    (format_concrete_datum_with_depth(player.get_datum(k), player, 0, 0), datum_to_mcp_value(player, v))
                })
                .collect(),
        ),
        _ => None,
    };

    to_json(&McpDatumInspection {
        datum_id,
        type_name: datum.type_str().to_string(),
        value: format_concrete_datum_with_depth(datum, player, 0, 0),
        properties,
    })
}

/// List all cast libraries
pub fn mcp_list_cast_libs(player: &DirPlayer) -> String {
    let libs: Vec<McpCastLibInfo> = player
        .movie
        .cast_manager
        .casts
        .iter()
        .map(|cast| McpCastLibInfo {
            number: cast.number as i32,
            name: cast.name.clone(),
            member_count: cast.members.len(),
            script_count: cast.scripts.len(),
        })
        .collect();

    to_json(&libs)
}

/// List cast members
pub fn mcp_list_cast_members(player: &DirPlayer, cast_lib: Option<i32>) -> String {
    let members: Vec<McpCastMemberInfo> = player
        .movie
        .cast_manager
        .casts
        .iter()
        .filter(|cast| cast_lib.map_or(true, |lib| cast.number as i32 == lib))
        .flat_map(|cast| {
            cast.members.iter().map(move |(&member_num, member)| {
                let bm = member.member_type.as_bitmap();
                // Honor BitmapInfo.center_reg_point: when true, Director's
                // runtime substitutes (width/2, height/2) for the stored
                // reg_point at composite (see player/score.rs:4230). Resolve
                // the effective regPoint here so downstream consumers don't
                // need to know about the flag.
                let (reg_x, reg_y) = bm.map(|b| {
                    if b.info.center_reg_point && b.info.width > 0 && b.info.height > 0 {
                        ((b.info.width / 2) as i16, (b.info.height / 2) as i16)
                    } else {
                        b.reg_point
                    }
                }).unwrap_or((0, 0));
                McpCastMemberInfo {
                    cast_lib: cast.number as i32,
                    cast_member: member_num as i32,
                    name: member.name.clone(),
                    member_type: member.member_type.type_string().to_string(),
                    reg_x,
                    reg_y,
                    bit_depth: bm.map(|b| b.info.bit_depth).unwrap_or(0),
                    use_alpha: bm.map(|b| b.info.use_alpha).unwrap_or(false),
                    palette_id: bm.map(|b| b.info.palette_id).unwrap_or(0),
                    width: bm.map(|b| b.info.width).unwrap_or(0),
                    height: bm.map(|b| b.info.height).unwrap_or(0),
                }
            })
        })
        .collect();

    to_json(&members)
}

/// Inspect a cast member
pub fn mcp_inspect_cast_member(player: &DirPlayer, cast_lib: i32, cast_member: i32) -> String {
    let member_ref = CastMemberRef { cast_lib, cast_member };

    let cast = match player.movie.cast_manager.get_cast(cast_lib as u32) {
        Ok(c) => c,
        Err(_) => return mcp_error(format!("Cast library {} not found", cast_lib)),
    };

    let member = match cast.members.get(&(cast_member as u32)) {
        Some(m) => m,
        None => return mcp_error(format!("Cast member {} not found in cast library {}", cast_member, cast_lib)),
    };

    // For script members, include script type and handlers
    let (script_type, handlers) = player
        .movie
        .cast_manager
        .get_script_by_ref(&member_ref)
        .map(|script| {
            (
                Some(script_type_str(&script.script_type).to_string()),
                Some(script.handler_names.clone()),
            )
        })
        .unwrap_or((None, None));

    to_json(&McpCastMemberDetails {
        cast_lib,
        cast_member,
        name: member.name.clone(),
        member_type: member.member_type.type_string().to_string(),
        script_type,
        handlers,
    })
}

/// Render a Bitmap cast member to PNG (palette-resolved RGBA) and return as
/// base64. Lets external tools dump per-room overlays without needing access
/// to the raw cast file format.
pub fn mcp_get_cast_member_picture(
    player: &DirPlayer,
    cast_lib: i32,
    cast_member: i32,
) -> String {
    use base64::Engine;
    use image::{ImageFormat, RgbaImage};

    let cast = match player.movie.cast_manager.get_cast(cast_lib as u32) {
        Ok(c) => c,
        Err(_) => return mcp_error(format!("Cast library {} not found", cast_lib)),
    };
    let member = match cast.members.get(&(cast_member as u32)) {
        Some(m) => m,
        None => return mcp_error(format!(
            "Cast member {} not found in cast library {}",
            cast_member, cast_lib
        )),
    };
    let bitmap_member = match member.member_type.as_bitmap() {
        Some(b) => b,
        None => return mcp_error(format!(
            "Cast member {}/{} is not a Bitmap (type: {})",
            cast_lib,
            cast_member,
            member.member_type.type_string()
        )),
    };
    let bitmap = match player.bitmap_manager.get_bitmap(bitmap_member.image_ref) {
        Some(b) => b,
        None => return mcp_error(format!(
            "Bitmap data for cast member {}/{} not loaded",
            cast_lib, cast_member
        )),
    };

    let palettes = player.movie.cast_manager.palettes();
    let width = bitmap.width;
    let height = bitmap.height;
    // Director's 32bpp bitmaps store ARGB but the alpha channel is only
    // semantically valid when `use_alpha` is true. For opaque 32bpp images
    // (eg. room backgrounds with `useAlpha=false` in Lingo), the stored
    // alpha bytes are zero and have to be forced to 0xFF or the dumped PNG
    // is fully transparent. 8bpp indexed bitmaps already return a=0xFF
    // unconditionally upstream, so this only affects the 32bpp path.
    let force_opaque_32bpp = bitmap.bit_depth == 32 && !bitmap.use_alpha;
    let mut img = RgbaImage::new(width as u32, height as u32);
    for y in 0..height {
        for x in 0..width {
            let (r, g, b, a) = bitmap.get_pixel_color_with_alpha(&palettes, x, y);
            let mut a = a;
            // 32bpp force-opaque: only flip alpha=>255 when the pixel has
            // non-zero RGB. Room backgrounds (tokyo_bg etc.) have visible
            // RGB content + a true-black (0,0,0,0) diamond margin that should
            // stay transparent for the room cutout. Effect overlays
            // (tokyo_bear_headlight, tokyo_pitlight) are 32bpp non-use-alpha
            // bitmaps that are mostly all-zero and intended to render
            // transparent at composite time; the pixel-aware check keeps
            // those zeros transparent while making real content opaque.
            if force_opaque_32bpp && (r != 0 || g != 0 || b != 0) {
                a = 0xFF;
            }
            // NB: 8bpp ink=8 (Background Transparent) chroma-keying is
            // deliberately NOT applied here. Ink mode is a per-sprite
            // property in Director (lives on the SceneXml element, not
            // the bitmap), so different scenes can use the same 8bpp
            // bitmap with different ink modes. Caller (Phaser sprite at
            // composite time) keys on its sprite's ink + the bitmap's
            // palette[0] background color.
            img.put_pixel(x as u32, y as u32, image::Rgba([r, g, b, a]));
        }
    }

    let mut png_bytes: Vec<u8> = Vec::new();
    if let Err(e) = img.write_to(&mut std::io::Cursor::new(&mut png_bytes), ImageFormat::Png) {
        return mcp_error(format!("PNG encoding failed: {}", e));
    }
    let png_base64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);

    let head_len = bitmap.data.len().min(64);
    let data_head_hex = bitmap.data[..head_len]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect::<String>();

    // Honor BitmapInfo.center_reg_point: at runtime, Director substitutes
    // (width/2, height/2) for the stored reg_point when this flag is set
    // (player/score.rs:4230). Resolve the effective regPoint here so
    // downstream consumers don't need to know about the flag.
    let (effective_reg_x, effective_reg_y) = if bitmap_member.info.center_reg_point
        && bitmap_member.info.width > 0 && bitmap_member.info.height > 0
    {
        ((bitmap_member.info.width / 2) as i16, (bitmap_member.info.height / 2) as i16)
    } else {
        bitmap_member.reg_point
    };

    to_json(&McpCastMemberPicture {
        cast_lib,
        cast_member,
        name: member.name.clone(),
        width,
        height,
        bit_depth: bitmap.bit_depth,
        original_bit_depth: bitmap.original_bit_depth,
        reg_x: effective_reg_x,
        reg_y: effective_reg_y,
        use_alpha: bitmap.use_alpha,
        palette_ref: format!("{:?}", bitmap.palette_ref),
        data_len: bitmap.data.len(),
        data_head_hex,
        png_base64,
    })
}

/// Convert Director's raw blend byte to a 0..=100 opacity percentage.
/// Matches score.rs:182 `convert_raw_blend` semantics — D8+ uses an inverted
/// 0-255 scale, D5-D7 stores the percentage directly with 0 meaning opaque.
fn raw_blend_to_percent(raw: u8, dir_version: u16) -> u8 {
    if dir_version >= 700 {
        if raw == 0 {
            100
        } else if raw == 255 {
            0
        } else {
            (((255.0 - raw as f32) * 100.0 / 255.0) as i32).clamp(0, 100) as u8
        }
    } else if raw == 0 {
        100
    } else {
        raw.min(100)
    }
}

/// Resolve effective tempo for a 1-based frame.
/// `tempo_data` is `(frame_idx_zero_based, TempoChannelData)` ascending.
/// Returns `(duration_ticks, duration_ms)` with default 30 fps when no
/// tempo entries apply yet.
fn resolve_film_loop_frame_tempo(
    tempo_data: &[(u32, crate::director::chunks::score::TempoChannelData)],
    frame_num_1based: u32,
) -> (u32, u32) {
    let mut latest = None;
    for (frame_idx, td) in tempo_data.iter() {
        if frame_idx + 1 <= frame_num_1based {
            latest = Some(td);
        } else {
            break;
        }
    }
    let default_fps: u32 = 30;
    let (ticks, ms) = match latest {
        // tempo == 246: tempoCuePoint holds FPS (D6+).
        Some(td) if td.tempo == 246 && td.tempo_cue_point > 0 => {
            let fps = td.tempo_cue_point as u32;
            (60u32.checked_div(fps).unwrap_or(2), 1000u32 / fps.max(1))
        }
        // tempo == 247: tempoCuePoint holds delay in ticks (1 tick = 1/60 s).
        Some(td) if td.tempo == 247 && td.tempo_cue_point > 0 => {
            let ticks = td.tempo_cue_point as u32;
            (ticks, ticks * 1000 / 60)
        }
        // Pre-D6 movies (and some D6+ casts) store FPS directly in the
        // tempo byte for values 1..=120.
        Some(td) if td.tempo > 0 && td.tempo <= 120 => {
            let fps = td.tempo as u32;
            (60u32.checked_div(fps).unwrap_or(2), 1000u32 / fps.max(1))
        }
        _ => (60 / default_fps, 1000 / default_fps),
    };
    (ticks, ms)
}

/// Walk a FilmLoop's score and return per-frame member references + tempo.
/// FilmLoops in Director are sub-scores with their own sprite channels and
/// tempo channel. The runtime advances `current_frame` 1..=frame_count and
/// renders whichever sprites are live in each frame's channels (resolved
/// via delta encoding over `channel_initialization_data`). This handler
/// returns the same per-frame resolution as a flat manifest so external
/// renderers (Furni's Phaser) can play back filmLoops without re-parsing
/// the cast file.
pub fn mcp_get_film_loop_frames(
    player: &DirPlayer,
    cast_lib: i32,
    cast_member: i32,
) -> String {
    let cast = match player.movie.cast_manager.get_cast(cast_lib as u32) {
        Ok(c) => c,
        Err(_) => return mcp_error(format!("Cast library {} not found", cast_lib)),
    };
    let member = match cast.members.get(&(cast_member as u32)) {
        Some(m) => m,
        None => return mcp_error(format!(
            "Cast member {} not found in cast library {}",
            cast_member, cast_lib
        )),
    };
    let film_loop = match &member.member_type {
        CastMemberType::FilmLoop(fl) => fl,
        _ => return mcp_error(format!(
            "Cast member {}/{} is not a FilmLoop (type: {})",
            cast_lib,
            cast_member,
            member.member_type.type_string()
        )),
    };

    // Sort init data defensively by (frame_idx, channel_idx). The reader emits
    // it in frame-ascending order, but if that ever changes, the per-span
    // resolution below would silently produce wrong frames.
    let mut init_data: Vec<(u32, u16, ScoreFrameChannelData)> =
        film_loop.score.channel_initialization_data.clone();
    init_data.sort_by_key(|(frame_idx, channel_idx, _)| (*frame_idx, *channel_idx));

    let mut tempo_data: Vec<(u32, crate::director::chunks::score::TempoChannelData)> =
        film_loop.score.tempo_channel_data.clone();
    tempo_data.sort_by_key(|(frame_idx, _)| *frame_idx);

    // Build the authoritative per-channel sprite spans from
    // `score_chunk.frame_intervals` (D6+). Each (primary, _secondary) pair
    // describes one sprite span: primary.{start_frame, end_frame, channel_index}
    // mark when a sprite is alive on a channel; the secondary entry holds the
    // span's behavior script ref, NOT the sprite's member (per score.rs:2562
    // — the runtime uses delta-merged channel_initialization_data within the
    // span range to pick the actual cast member).
    //
    // For pre-D6 / fallback movies with empty frame_intervals, fall back to
    // the score's already-derived sprite_spans (built from
    // generate_sprite_spans_from_channel_data).
    #[derive(Clone, Copy)]
    struct LoopSpan {
        channel: u32,
        start_frame: u32,
        end_frame: u32,
    }
    let spans: Vec<LoopSpan> = if !film_loop.score_chunk.frame_intervals.is_empty() {
        film_loop
            .score_chunk
            .frame_intervals
            .iter()
            .filter_map(|(primary, _)| {
                // Skip frame-script (channel 0) and the 5 reserved effect
                // channels (1-5 raw → channel_number 0).
                if primary.channel_index <= 5 {
                    return None;
                }
                let channel = get_channel_number_from_index(primary.channel_index);
                if channel == 0 {
                    return None;
                }
                Some(LoopSpan {
                    channel,
                    start_frame: primary.start_frame,
                    end_frame: primary.end_frame,
                })
            })
            .collect()
    } else {
        film_loop
            .score
            .sprite_spans
            .iter()
            .filter(|s| s.channel_number >= 1)
            .map(|s| LoopSpan {
                channel: s.channel_number,
                start_frame: s.start_frame,
                end_frame: s.end_frame,
            })
            .collect()
    };

    // Determine frame count. Prefer score.frame_count; fall back to maxes.
    let max_init_frame = init_data.iter().map(|(f, _, _)| *f + 1).max().unwrap_or(0);
    let max_tempo_frame = tempo_data.iter().map(|(f, _)| *f + 1).max().unwrap_or(0);
    let max_span_frame = spans.iter().map(|s| s.end_frame).max().unwrap_or(0);
    let computed_count = max_init_frame
        .max(max_tempo_frame)
        .max(max_span_frame)
        .max(1);
    let frame_count = film_loop.score.frame_count.unwrap_or(computed_count);

    let filmloop_cast_lib = cast_lib;
    let dir_version = player.movie.dir_version;

    // Walk init_data entries whose 1-based frame is within [span.start_frame,
    // frame_num] AND whose channel matches the span. Build a delta-merged
    // `ScoreFrameChannelData` snapshot scoped to the span — matches the
    // runtime sequence (sprite enters at start_frame with init values, deltas
    // mutate it within the span, leaves at end_frame so the next span on
    // the same channel starts fresh).
    //
    // For tween-driven properties (pos/size/rotation/skew/blend), the score
    // chunk's keyframes_cache provides absolute values per frame and is
    // consulted as an override after the delta merge — that path captures
    // anything Director resolved as a tween rather than a raw delta.
    let resolve_span_data = |span: &LoopSpan, frame_num: u32| -> Option<ScoreFrameChannelData> {
        let mut current: Option<ScoreFrameChannelData> = None;
        for (frame_idx, channel_idx, data) in init_data.iter() {
            let f_1based = frame_idx + 1;
            if f_1based < span.start_frame {
                continue;
            }
            if f_1based > frame_num {
                break;
            }
            if get_channel_number_from_index(*channel_idx as u32) != span.channel {
                continue;
            }
            // Director uses dense per-frame writes for tweened sprites in
            // D5+, with sentinel zeros for "no change" on a few fields:
            //   - cast_member == 0 → keep prior member
            //   - width/height == 0 → keep prior size
            // Other fields (pos, rotation, skew, blend, ink, stretch) are
            // written every frame and overwrite directly.
            match current.as_mut() {
                None => current = Some(data.clone()),
                Some(cur) => {
                    if data.cast_member != 0 {
                        cur.cast_lib = data.cast_lib;
                        cur.cast_member = data.cast_member;
                    }
                    cur.pos_x = data.pos_x;
                    cur.pos_y = data.pos_y;
                    if data.width != 0 {
                        cur.width = data.width;
                    }
                    if data.height != 0 {
                        cur.height = data.height;
                    }
                    cur.rotation = data.rotation;
                    cur.skew = data.skew;
                    cur.blend = data.blend;
                    cur.ink = data.ink;
                    cur.stretch = data.stretch;
                }
            }
        }
        current
    };

    let mut frames: Vec<McpFilmLoopFrame> = Vec::with_capacity(frame_count as usize);
    for frame_num in 1..=frame_count {
        let mut active_spans: Vec<&LoopSpan> = spans
            .iter()
            .filter(|s| s.start_frame <= frame_num && frame_num <= s.end_frame)
            .collect();
        active_spans.sort_by_key(|s| s.channel);

        let mut members: Vec<McpFilmLoopFrameMember> = Vec::new();
        for span in active_spans {
            let data = match resolve_span_data(span, frame_num) {
                Some(d) if d.cast_member != 0 => d,
                _ => continue, // Span has no resolvable member yet — skip.
            };
            // Resolve cast_lib: 65535 ("relative to parent cast") and 0 both
            // fall back to the filmloop's own cast (matches score.rs:920-926
            // plus the filmloop comment at score.rs:235).
            let resolved_cast_lib = if data.cast_lib == 65535 || data.cast_lib == 0 {
                filmloop_cast_lib
            } else {
                data.cast_lib as i32
            };
            let member_ref = CastMemberRef {
                cast_lib: resolved_cast_lib,
                cast_member: data.cast_member as i32,
            };
            let resolved_member = player.movie.cast_manager.find_member_by_ref(&member_ref);
            let name = resolved_member
                .map(|m| m.name.clone())
                .unwrap_or_default();

            // Start from the delta-merged init values, then let
            // keyframes_cache override per-frame transforms.
            // pos_x/pos_y are in source filmLoop coords; subtract
            // initial_rect.left/top so emitted loc_h/loc_v are in
            // filmloop_bitmap-local coords (matches the offset
            // render_score_to_bitmap_with_offset applies at
            // rendering.rs:2225). Downstream renderers can place
            // children directly without re-applying the offset.
            let mut loc_h = data.pos_x as i32 - film_loop.initial_rect.left;
            let mut loc_v = data.pos_y as i32 - film_loop.initial_rect.top;
            let mut width = data.width as i32;
            let mut height = data.height as i32;
            let mut rotation = data.rotation;
            let mut skew = data.skew;
            let mut blend_pct = raw_blend_to_percent(data.blend, dir_version);

            if let Some(kf) = film_loop.score.keyframes_cache.get(&(span.channel as u16)) {
                if let Some(path) = kf.path.as_ref() {
                    if let Some((x, y)) = path.get_position_at_frame(frame_num) {
                        // Same offset normalization as the delta-merged
                        // pos above — keep emitted loc in bitmap-local space.
                        loc_h = x as i32 - film_loop.initial_rect.left;
                        loc_v = y as i32 - film_loop.initial_rect.top;
                    }
                }
                if let Some(size_kf) = kf.size.as_ref() {
                    if let Some((w, h)) = size_kf.get_size_at_frame(frame_num) {
                        width = w as i32;
                        height = h as i32;
                    }
                }
                if let Some(rot_kf) = kf.rotation.as_ref() {
                    if let Some(r) = rot_kf.get_rotation_at_frame(frame_num) {
                        rotation = r;
                    }
                }
                if let Some(skew_kf) = kf.skew.as_ref() {
                    if let Some(s) = skew_kf.get_skew_at_frame(frame_num) {
                        skew = s;
                    }
                }
                if let Some(blend_kf) = kf.blend.as_ref() {
                    if let Some(b) = blend_kf.get_blend_at_frame(frame_num) {
                        blend_pct = b;
                    }
                }
            }

            // Apply Director's "stretch off → bitmap natural size" rule
            // (score.rs:4184-4196) so the renderer doesn't have to look up
            // bitmap meta. Only kicks in for Bitmap members; non-bitmap
            // members fall through with whatever size was tweened.
            if !data.stretch {
                if let Some(member) = resolved_member {
                    if let CastMemberType::Bitmap(bmp) = &member.member_type {
                        if bmp.info.width > 0 && bmp.info.height > 0 {
                            width = bmp.info.width as i32;
                            height = bmp.info.height as i32;
                        }
                    }
                }
            }

            members.push(McpFilmLoopFrameMember {
                channel: span.channel,
                cast_lib: resolved_cast_lib,
                cast_member: data.cast_member as i32,
                name,
                loc_h,
                loc_v,
                width,
                height,
                rotation,
                skew,
                blend: blend_pct,
                ink: data.ink,
            });
        }

        let (duration_ticks, duration_ms) =
            resolve_film_loop_frame_tempo(&tempo_data, frame_num);
        frames.push(McpFilmLoopFrame {
            frame: frame_num,
            duration_ticks,
            duration_ms,
            members,
        });
    }

    let default_duration_ms = frames.first().map(|f| f.duration_ms).unwrap_or(33);

    let sprite_spans: Vec<McpFilmLoopSpan> = film_loop
        .score
        .sprite_spans
        .iter()
        .map(|s| McpFilmLoopSpan {
            channel: s.channel_number,
            start_frame: s.start_frame,
            end_frame: s.end_frame,
        })
        .collect();

    // Manifest dimensions come from `initial_rect` (the bounding box of all
    // child sprites across all frames) — that's what Director uses as the
    // filmloop_bitmap natural size and the on-stage rendering size, NOT
    // the FilmLoopInfo's stored width/height. See cast_member.rs:768
    // `compute_filmloop_initial_rect` and rendering.rs:2188-2189
    // (`let width = initial_rect.width()`).
    //
    // Director filmLoops ALWAYS use center registration: the loc_h/loc_v
    // anchor on the parent .room sprite is treated as the filmLoop's
    // CENTER (score.rs:4392-4394 hardcodes `reg_x = use_width/2`). With
    // bitmap-local child loc_h/loc_v (offset-normalized above) and a
    // centered regPoint, downstream renderers can place each child as
    // `(parent_loc - reg + child_loc - child_reg)`.
    let nat_width = film_loop.initial_rect.width().max(1) as u16;
    let nat_height = film_loop.initial_rect.height().max(1) as u16;
    let reg_x = (nat_width / 2) as i16;
    let reg_y = (nat_height / 2) as i16;

    to_json(&McpFilmLoopFrames {
        cast_lib,
        cast_member,
        name: member.name.clone(),
        frame_count,
        width: nat_width,
        height: nat_height,
        reg_x,
        reg_y,
        loops: film_loop.info.loops != 0,
        default_duration_ms,
        sprite_spans,
        frames,
    })
}

/// List all breakpoints
pub fn mcp_list_breakpoints(player: &DirPlayer) -> String {
    to_json(&McpBreakpointList {
        breakpoints: player
            .breakpoint_manager
            .breakpoints
            .iter()
            .map(|bp| McpBreakpointInfo {
                script_name: bp.script_name.clone(),
                handler_name: bp.handler_name.clone(),
                bytecode_index: bp.bytecode_index,
            })
            .collect(),
    })
}

/// Format eval result as JSON
pub fn mcp_format_eval_result(
    player: &DirPlayer,
    result: Result<super::DatumRef, super::ScriptError>,
) -> String {
    match result {
        Ok(datum_ref) => {
            let datum = player.get_datum(&datum_ref);
            let datum_id = match &datum_ref {
                super::DatumRef::Void => None,
                super::DatumRef::Ref(id, _) => Some(*id),
            };
            to_json(&McpEvalResult {
                success: true,
                result_type: Some(datum.type_str().to_string()),
                result_value: Some(format_concrete_datum_with_depth(datum, player, 0, 0)),
                datum_id,
                error: None,
            })
        }
        Err(err) => {
            to_json(&McpEvalResult {
                success: false,
                result_type: None,
                result_value: None,
                datum_id: None,
                error: Some(err.message),
            })
        }
    }
}
