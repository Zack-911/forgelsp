//! WASM-specific API for ForgeLSP.
//!
//! Exposes a high-level JavaScript-callable interface for language features
//! like completion, hover, and diagnostics — without requiring an LSP transport.

use std::sync::Arc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use crate::metadata::MetadataManager;
use crate::parser::ForgeScriptParser;
use crate::utils;
use lsp_types::Position;

use std::sync::OnceLock;

/// Global state for the WASM module.
static MANAGER: OnceLock<Arc<MetadataManager>> = OnceLock::new();

/// Initializes the WASM module with optional configuration JSON.
///
/// Call this before any other API. Accepts a JSON string matching the
/// `forgeconfig.json` schema (or an empty string for defaults).
#[wasm_bindgen]
pub fn init(config_json: &str) -> Result<(), JsValue> {
    utils::init_wasm_logger(utils::LogLevel::Debug);
    utils::forge_log(utils::LogLevel::Info, "ForgeLSP WASM module initializing");

    let fetch_urls = if config_json.is_empty() {
        vec![
            "https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"
                .to_string(),
        ]
    } else {
        utils::parse_forge_config(config_json)
            .map(|config| config.urls)
            .unwrap_or_else(|| {
                vec!["https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"
                    .to_string()]
            })
    };

    let mgr = MetadataManager::new_wasm(fetch_urls)
        .map_err(|e| JsValue::from_str(&format!("Init error: {e}")))?;
    let arc = Arc::new(mgr);
    MANAGER
        .set(arc)
        .map_err(|_| JsValue::from_str("Already initialized"))?;

    utils::forge_log(utils::LogLevel::Info, "ForgeLSP WASM module initialized");
    Ok(())
}

/// Loads metadata from the configured URLs.
///
/// Returns a Promise that resolves when metadata is loaded.
#[wasm_bindgen]
pub fn load_metadata() -> js_sys::Promise {
    future_to_promise(async move {
        let mgr = MANAGER
            .get()
            .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;
        mgr.load_all()
            .await
            .map_err(|e| JsValue::from_str(&format!("Metadata load error: {e}")))?;
        utils::forge_log(
            utils::LogLevel::Info,
            &format!("Loaded {} functions", mgr.function_count()),
        );
        Ok(JsValue::from_str("ok"))
    })
}

/// Adds custom function definitions from a JSON string.
///
/// Accepts a JSON array of custom function objects matching the `CustomFunction` schema.
#[wasm_bindgen]
pub fn add_custom_functions(json: &str) -> Result<JsValue, JsValue> {
    let mgr = MANAGER
        .get()
        .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;

    let custom_funcs: Vec<utils::CustomFunction> = serde_json::from_str(json)
        .map_err(|e| JsValue::from_str(&format!("JSON parse error: {e}")))?;

    let registered = mgr
        .add_custom_functions(custom_funcs)
        .map_err(|e| JsValue::from_str(&format!("Add custom functions error: {e}")))?;

    serde_json::to_string(&registered)
        .map(|s| JsValue::from_str(&s))
        .map_err(|e| JsValue::from_str(&format!("Serialize error: {e}")))
}

/// Parses a ForgeScript document and returns a JSON result.
///
/// Returns JSON with `{ functions, diagnostics }`.
#[wasm_bindgen]
pub fn process_document(text: &str) -> Result<JsValue, JsValue> {
    let mgr = MANAGER
        .get()
        .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;

    let start = utils::Instant::now();
    let parser = ForgeScriptParser::new(mgr.clone(), text);
    let result = parser.parse();

    let output = serde_json::json!({
        "functions": result.functions.iter().map(|f| {
            serde_json::json!({
                "name": f.name,
                "span": [f.span.0, f.span.1],
                "args": f.args,
            })
        }).collect::<Vec<_>>(),
        "diagnostics": result.diagnostics.iter().map(|d| {
            serde_json::json!({
                "start": d.start,
                "end": d.end,
                "message": d.message,
            })
        }).collect::<Vec<_>>(),
    });

    utils::forge_log(
        utils::LogLevel::Debug,
        &format!("Document processed in {}", start.elapsed_display()),
    );

    serde_json::to_string(&output)
        .map(|s| JsValue::from_str(&s))
        .map_err(|e| JsValue::from_str(&format!("Serialize error: {e}")))
}

/// Returns the total number of functions loaded.
#[wasm_bindgen]
pub fn get_function_count() -> Result<u32, JsValue> {
    let mgr = MANAGER
        .get()
        .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;
    Ok(mgr.function_count() as u32)
}

/// Gets hover information for a function name.
///
/// Returns JSON with function metadata or null if not found.
#[wasm_bindgen]
pub fn get_hover(function_name: &str) -> Result<JsValue, JsValue> {
    let mgr = MANAGER
        .get()
        .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;

    let name = if function_name.starts_with('$') {
        function_name.to_string()
    } else {
        format!("${function_name}")
    };

    match mgr.get(&name) {
        Some(func) => {
            let output = serde_json::json!({
                "name": func.name,
                "description": func.description,
                "brackets": func.brackets,
                "args": func.args.as_ref().map(|args| {
                    args.iter().map(|a| {
                        serde_json::json!({
                            "name": a.name,
                            "description": a.description,
                            "required": a.required,
                            "type": a.arg_type,
                            "rest": a.rest,
                        })
                    }).collect::<Vec<_>>()
                }),
                "output": func.output,
                "examples": func.examples,
            });
            serde_json::to_string(&output)
                .map(|s| JsValue::from_str(&s))
                .map_err(|e| JsValue::from_str(&format!("Serialize error: {e}")))
        }
        None => Ok(JsValue::NULL),
    }
}

/// Gets all function names as a JSON array of strings.
#[wasm_bindgen]
pub fn get_all_function_names() -> Result<JsValue, JsValue> {
    let mgr = MANAGER
        .get()
        .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;

    let names: Vec<String> = mgr.all_functions().iter().map(|f| f.name.clone()).collect();

    serde_json::to_string(&names)
        .map(|s| JsValue::from_str(&s))
        .map_err(|e| JsValue::from_str(&format!("Serialize error: {e}")))
}

/// Converts a byte offset to (line, character) in the given text.
#[wasm_bindgen]
pub fn offset_to_position(text: &str, offset: usize) -> JsValue {
    let (line, col) = utils::offset_to_position_raw(text, offset);
    let arr = js_sys::Array::new();
    arr.push(&JsValue::from(line));
    arr.push(&JsValue::from(col));
    arr.into()
}

/// Converts (line, character) to a byte offset in the given text.
#[wasm_bindgen]
pub fn position_to_offset(text: &str, line: u32, character: u32) -> JsValue {
    match utils::position_to_offset_raw(text, line, character) {
        Some(offset) => JsValue::from(offset as u32),
        None => JsValue::NULL,
    }
}

/// Extracts semantic tokens for the given ForgeScript text.
///
/// Returns a JSON string containing relative LSP semantic tokens.
#[wasm_bindgen]
pub fn get_semantic_tokens(text: &str, use_colors: bool) -> Result<JsValue, JsValue> {
    let mgr = MANAGER
        .get()
        .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;

    let tokens = crate::semantic::extract_semantic_tokens_with_colors(text, use_colors, mgr);

    let data: Vec<u32> = tokens
        .iter()
        .flat_map(|t| {
            vec![
                t.delta_line,
                t.delta_start,
                t.length,
                t.token_type,
                t.token_modifiers_bitset,
            ]
        })
        .collect();

    serde_json::to_string(&data)
        .map(|s| JsValue::from_str(&s))
        .map_err(|e| JsValue::from_str(&format!("Serialize error: {e}")))
}

/// Extracts VS Code-specific highlight ranges for the given text.
///
/// Returns a JSON string containing `Vec<(start, end, color)>`.
#[wasm_bindgen]
pub fn get_highlight_ranges(text: &str) -> Result<JsValue, JsValue> {
    let mgr = MANAGER
        .get()
        .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;

    // Use default colors or empty since we don't have the server's function_colors here.
    // However, the caller should ideally provide colors.
    // For now, let's use a default set if none provided.
    let default_colors = vec![
        "#f5c2e7".to_string(),
        "#cba6f7".to_string(),
        "#f38ba8".to_string(),
        "#fab387".to_string(),
        "#f9e2af".to_string(),
        "#a6e3a1".to_string(),
        "#94e2d5".to_string(),
        "#89dceb".to_string(),
        "#89b4fa".to_string(),
        "#b4befe".to_string(),
    ];

    let highlights = crate::semantic::extract_highlight_ranges(text, &default_colors, true, mgr);

    serde_json::to_string(&highlights)
        .map(|s| JsValue::from_str(&s))
        .map_err(|e| JsValue::from_str(&format!("Serialize error: {e}")))
}

/// Extracts autocomplete results for the given ForgeScript text and position.
#[wasm_bindgen]
pub fn get_completions(text: &str, line: u32, character: u32) -> Result<JsValue, JsValue> {
    let mgr = MANAGER
        .get()
        .ok_or_else(|| JsValue::from_str("Not initialized — call init() first"))?;

    let pos = Position::new(line, character);
    let completions = crate::completion::get_completions(text, pos, mgr);

    serde_json::to_string(&completions)
        .map(|s| JsValue::from_str(&s))
        .map_err(|e| JsValue::from_str(&format!("Serialize error: {e}")))
}
