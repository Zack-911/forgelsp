//! Utility functions for ForgeLSP.
//!
//! Includes logic for configuration loading, GitHub URL resolution,
//! asynchronous logging, and ForgeScript-specific string manipulation.

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use lsp_types::*;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
#[cfg(not(target_arch = "wasm32"))]
use std::io::Write;
use std::sync::{LazyLock, OnceLock};

pub(crate) static SIGNATURE_FUNC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*$").expect("Server: regex failure"));

/// Available log levels for ForgeLSP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace = 0,
    Debug = 1,
    Info = 2,
    Warn = 3,
    Error = 4,
}

// ── Native Logger ───────────────────────────────────────────────────────────

/// Specialized logger for ForgeLSP that writes to console and a file.
#[cfg(not(target_arch = "wasm32"))]
pub struct ForgeLogger {
    pub level: LogLevel,
    pub log_path: PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
static LOGGER: OnceLock<ForgeLogger> = OnceLock::new();

/// Initializes the global logger, clearing any existing log file.
#[cfg(not(target_arch = "wasm32"))]
pub fn init_logger(workspace_root: PathBuf, level: LogLevel) -> anyhow::Result<()> {
    let vscode_dir = workspace_root.join(".vscode");
    if !vscode_dir.exists() {
        let _ = fs::create_dir_all(&vscode_dir);
    }
    let log_path = vscode_dir.join("forgelsp.log");

    // Clear/Create the log file on start
    let _ = fs::File::create(&log_path);

    LOGGER
        .set(ForgeLogger { level, log_path })
        .map_err(|_| anyhow::anyhow!("Logger already initialized"))?;

    Ok(())
}

/// Logs a message if it meets the configured log level (native: stderr + file).
#[cfg(not(target_arch = "wasm32"))]
pub fn forge_log(level: LogLevel, msg: &str) {
    if let Some(logger) = LOGGER.get() {
        if level >= logger.level {
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let level_str = format!("{:?}", level).to_uppercase();
            let log_line = format!("[{}] [{}] {}\n", timestamp, level_str, msg);

            // Print to stderr (LSP standard for console logs)
            eprint!("{}", log_line);

            // Write to file
            if let Ok(mut file) = fs::OpenOptions::new().append(true).open(&logger.log_path) {
                let _ = file.write_all(log_line.as_bytes());
            }
        }
    }
}

// ── WASM Logger ─────────────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
static WASM_LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();

/// Initializes the WASM logger with the specified log level.
#[cfg(target_arch = "wasm32")]
pub fn init_wasm_logger(level: LogLevel) {
    let _ = WASM_LOG_LEVEL.set(level);
}

/// Logs a message via `console.log` / `console.warn` / `console.error` on WASM.
#[cfg(target_arch = "wasm32")]
pub fn forge_log(level: LogLevel, msg: &str) {
    let min_level = WASM_LOG_LEVEL.get().copied().unwrap_or(LogLevel::Info);
    if level >= min_level {
        let level_str = format!("{:?}", level).to_uppercase();
        let log_line = format!("[ForgeLSP] [{}] {}", level_str, msg);
        let js_val = wasm_bindgen::JsValue::from_str(&log_line);

        match level {
            LogLevel::Error => web_sys::console::error_1(&js_val),
            LogLevel::Warn => web_sys::console::warn_1(&js_val),
            _ => web_sys::console::log_1(&js_val),
        }
    }
}

// ── Cross-platform Timing ───────────────────────────────────────────────────

/// A cross-platform instant for measuring elapsed time.
/// On native: wraps `std::time::Instant`.
/// On WASM: wraps `Performance.now()` (milliseconds as f64).
pub struct Instant {
    #[cfg(not(target_arch = "wasm32"))]
    inner: std::time::Instant,
    #[cfg(target_arch = "wasm32")]
    start_ms: f64,
}

impl Instant {
    pub fn now() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self {
                inner: std::time::Instant::now(),
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let start_ms = js_sys::Date::now();
            Self { start_ms }
        }
    }

    /// Returns elapsed time formatted as a debug string (e.g. "12.34ms").
    pub fn elapsed_display(&self) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        {
            format!("{:?}", self.inner.elapsed())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let elapsed = js_sys::Date::now() - self.start_ms;
            format!("{:.2}ms", elapsed)
        }
    }
}

// ── Data Types ──────────────────────────────────────────────────────────────

/// Parameters for project-specific custom function definitions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomFunctionParam {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub rest: Option<bool>,
    #[serde(default)]
    pub arg_enum: Option<Vec<String>>,
    #[serde(default)]
    pub enum_name: Option<String>,
}

/// Structure representing a user-defined function in ForgeLSP.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CustomFunction {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub params: Option<JsonValue>,
    #[serde(default)]
    pub brackets: Option<bool>,
    #[serde(default)]
    pub alias: Option<Vec<String>>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub output: Option<Vec<String>>,
}

/// Definition for ForgeScript events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub intents: Option<Vec<String>>,
}

/// Configuration structure for ForgeLSP, typically loaded from forgeconfig.json.
#[derive(Debug, Deserialize, Clone)]
pub struct ForgeConfig {
    pub urls: Vec<String>,
    #[serde(default)]
    pub multiple_function_colors: Option<bool>,
    #[serde(default)]
    pub function_colors: Option<Vec<String>>,
    #[serde(default)]
    pub consistent_function_colors: Option<bool>,
    #[serde(default)]
    pub custom_functions: Option<Vec<CustomFunction>>,
    #[serde(default)]
    pub custom_functions_path: Option<String>,
    #[serde(default)]
    pub log_level: Option<LogLevel>,
}

// ── Config Loading (Native) ─────────────────────────────────────────────────

/// Attempts to find and load the ForgeLSP configuration from the workspace.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_forge_config(workspace_folders: &[PathBuf]) -> Option<Vec<String>> {
    load_forge_config_full(workspace_folders).map(|(cfg, _)| cfg.urls)
}

/// Recursively looks for forgeconfig.json in the workspace roots or .vscode directories.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_forge_config_full(workspace_folders: &[PathBuf]) -> Option<(ForgeConfig, PathBuf)> {
    for folder in workspace_folders {
        let possible_paths = [
            folder.join("forgeconfig.json"),
            folder.join(".vscode").join("forgeconfig.json"),
        ];

        for path in possible_paths {
            if !path.exists() {
                continue;
            }

            crate::utils::forge_log(
                crate::utils::LogLevel::Debug,
                &format!("Loading config from: {}", path.display()),
            );
            let Ok(data) = fs::read_to_string(&path) else {
                crate::utils::forge_log(
                    crate::utils::LogLevel::Warn,
                    &format!("Failed to read config at {}", path.display()),
                );
                continue;
            };
            let Ok(mut raw) = serde_json::from_str::<ForgeConfig>(&data) else {
                crate::utils::forge_log(
                    crate::utils::LogLevel::Error,
                    &format!("Invalid JSON in config at {}", path.display()),
                );
                continue;
            };

            raw.urls = raw.urls.into_iter().map(resolve_github_shorthand).collect();
            return Some((raw, path.parent().unwrap().to_path_buf()));
        }
    }
    None
}

// ── Config Loading (Cross-platform) ─────────────────────────────────────────

/// Parses a ForgeConfig from a JSON string. Works on both native and WASM.
pub fn parse_forge_config(json: &str) -> Option<ForgeConfig> {
    let mut raw: ForgeConfig = serde_json::from_str(json).ok()?;
    raw.urls = raw.urls.into_iter().map(resolve_github_shorthand).collect();
    Some(raw)
}

/// Transforms github: shorthand into raw.githubusercontent.com URLs.
fn resolve_github_shorthand(input: String) -> String {
    if !input.starts_with("github:") {
        return input;
    }

    crate::utils::forge_log(
        crate::utils::LogLevel::Trace,
        &format!("Resolving GitHub shorthand: {}", input),
    );
    let Some(trimmed) = input.strip_prefix("github:") else {
        return input;
    };

    let (path, branch) = match trimmed.split_once('#') {
        Some((p, b)) => (p, b),
        Option::None => (trimmed, "main"),
    };

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return input;
    }

    let owner = parts[0];
    let repo = parts[1];
    let file_path = if parts.len() > 2 {
        parts[2..].join("/")
    } else {
        "metadata/functions.json".to_string()
    };

    format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{file_path}")
}

// ── ForgeScript Utilities (shared) ──────────────────────────────────────────

/// Determines if a character at a given byte index is escaped by backslashes.
pub fn is_escaped(code: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 || !code.is_char_boundary(byte_idx) {
        return false;
    }

    let bytes = code.as_bytes();
    let target = bytes[byte_idx];
    let mut count = 0;
    let mut i = byte_idx;

    while i > 0 {
        i -= 1;
        if bytes[i] == b'\\' {
            count += 1;
        } else {
            break;
        }
    }

    if target == b'`' {
        count == 1
    } else {
        count == 2
    }
}

/// Verifies if a '[' character is the start of a ForgeScript function call.
pub fn is_function_call_bracket(text: &str, bracket_idx: usize) -> bool {
    if bracket_idx == 0 || text.as_bytes().get(bracket_idx) != Some(&b'[') {
        return false;
    }

    let mut i = bracket_idx;
    let bytes = text.as_bytes();

    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }

    while i > 0 && matches!(bytes[i - 1], b'!' | b'#' | b'@' | b']') {
        match bytes[i - 1] {
            b'!' | b'#' => i -= 1,
            b']' => {
                let mut depth = 0;
                let mut found = false;
                while i > 0 {
                    i -= 1;
                    if bytes[i] == b']' {
                        depth += 1;
                    } else if bytes[i] == b'[' {
                        depth -= 1;
                        if depth == 0 {
                            found = true;
                            break;
                        }
                    }
                }
                if !found || i == 0 || bytes[i - 1] != b'@' {
                    return false;
                }
                i -= 1;
            }
            _ => break,
        }
    }

    i > 0 && bytes[i - 1] == b'$' && !is_escaped(text, i - 1)
}

/// Finds the closing bracket for a function call, respecting nested calls and escapes.
pub fn find_matching_bracket(code: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    let mut iter = code.char_indices().skip_while(|&(idx, _)| idx < open_idx);

    while let Some((i, c)) = iter.next() {
        if is_escaped(code, i) {
            continue;
        }

        if c == '[' {
            if i == open_idx || is_function_call_bracket(code, i) {
                depth += 1;
            }
        } else if c == ']' {
            if depth > 0 {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        } else if c == '$' {
            if let Some(end_idx) = find_escape_function_end(code, i) {
                while let Some(&(idx, _)) = iter.clone().peekable().peek() {
                    if idx <= end_idx {
                        iter.next();
                    } else {
                        break;
                    }
                }
                continue;
            }
        }
    }
    None
}

/// Detects if the current position is the start of an escape function ($esc, etc.).
pub fn find_escape_function_end(code: &str, dollar_idx: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    if dollar_idx >= code.len() || bytes[dollar_idx] != b'$' || is_escaped(code, dollar_idx) {
        return None;
    }

    let mut pos = dollar_idx + 1;
    while pos < bytes.len() && (bytes[pos] == b'!' || bytes[pos] == b'#') {
        pos += 1;
    }

    let name_start = pos;
    while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
        pos += 1;
    }

    if pos == name_start {
        return None;
    }

    let name = &code[name_start..pos];
    if !is_escape_function(name) {
        return None;
    }

    if pos >= bytes.len() || bytes[pos] != b'[' {
        return None;
    }

    find_matching_bracket_raw(bytes, pos)
}

/// Performs a raw bracket match without considering ForgeScript escapes or functions.
pub fn find_matching_bracket_raw(bytes: &[u8], open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, &byte) in bytes.iter().enumerate().skip(open_idx) {
        if byte == b'[' {
            depth += 1;
        } else if byte == b']' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Converts a byte offset into an LSP Position (Line/Character).
pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let (line, col) = offset_to_position_raw(text, offset);
    Position::new(line, col)
}

/// Converts an LSP Position into a byte offset within the source text.
pub fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    position_to_offset_raw(text, position.line, position.character)
}

pub fn get_text_up_to_cursor(text: &str, position: Position) -> String {
    let mut text_up_to_cursor =
        if let Some(offset) = position_to_offset_raw(text, position.line, position.character) {
            text[..offset].to_string()
        } else {
            text.to_string()
        };

    if text_up_to_cursor.len() > 8 * 1024 {
        let len = text_up_to_cursor.len();
        text_up_to_cursor = text_up_to_cursor[len - 8 * 1024..].to_string();
    }
    text_up_to_cursor
}

pub fn find_active_function_call(text_up_to_cursor: &str) -> Option<(String, usize)> {
    let mut depth = 0i32;
    let mut last_open_index: Option<usize> = None;

    for (idx, ch) in text_up_to_cursor.char_indices().rev() {
        match ch {
            ']' => depth += 1,
            '[' => {
                if depth == 0 {
                    last_open_index = Some(idx);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }

    let open_index = last_open_index?;
    let before_bracket = &text_up_to_cursor[..open_index];
    let caps = SIGNATURE_FUNC_RE.captures(before_bracket)?;
    let func_name = caps.get(1)?.as_str().to_string();
    Some((func_name, open_index))
}

pub fn compute_active_param_index(text_after_bracket: &str) -> u32 {
    let mut param_index: u32 = 0;
    let mut local_depth: i32 = 0;
    let mut in_single = false;
    let mut in_double = false;
    let mut prev_escape = false;

    for ch in text_after_bracket.chars() {
        if prev_escape {
            prev_escape = false;
            continue;
        }
        if ch == '\\' {
            prev_escape = true;
            continue;
        }
        if ch == '\'' && !in_double {
            in_single = !in_single;
            continue;
        }
        if ch == '"' && !in_single {
            in_double = !in_double;
            continue;
        }
        if in_single || in_double {
            continue;
        }

        match ch {
            '[' => local_depth += 1,
            ']' => {
                if local_depth > 0 {
                    local_depth -= 1;
                } else {
                    break;
                }
            }
            ',' | ';' if local_depth == 0 => {
                param_index = param_index.saturating_add(1);
            }
            _ => {}
        }
    }
    param_index
}

/// Converts a byte offset to (line, character) — works on all targets.
pub fn offset_to_position_raw(text: &str, offset: usize) -> (u32, u32) {
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, ch) in text.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += u32::try_from(ch.len_utf16()).expect("UTF-16 length exceeds u32");
        }
    }
    (line, col)
}

/// Converts (line, character) to a byte offset — works on all targets.
pub fn position_to_offset_raw(text: &str, line: u32, character: u32) -> Option<usize> {
    let mut current_offset = 0;

    for (line_num, line_text) in text.split_inclusive('\n').enumerate() {
        if line_num as u32 == line {
            let mut col = 0;
            for (i, c) in line_text.char_indices() {
                if col == character {
                    return Some(current_offset + i);
                }
                col += c.len_utf16() as u32;
            }
            if col == character {
                return Some(current_offset + line_text.len());
            }
            return None;
        }
        current_offset += line_text.len();
    }
    None
}

/// Checks if a string matches a known ForgeScript escape function name.
pub fn is_escape_function(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "esc" || lower == "escape" || lower == "escapecode"
}

/// Advances a cursor past ForgeScript modifiers at the start of a function.
pub fn skip_modifiers(text: &str, start_idx: usize) -> usize {
    let bytes = text.as_bytes();
    let mut pos = start_idx;

    while pos < bytes.len() {
        match bytes[pos] {
            b'!' | b'#' => pos += 1,
            b'@' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'[' {
                    if let Some(end_idx) = find_matching_bracket_raw(bytes, pos + 1) {
                        pos = end_idx + 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    pos
}

/// Calculates the function nesting depth at a specific byte offset.
pub fn calculate_depth(text: &str, offset: usize) -> usize {
    let mut current_depth = 0;
    let chars: Vec<(usize, char)> = text.char_indices().collect();

    for (byte_idx, c) in chars {
        if byte_idx >= offset {
            break;
        }

        if !is_escaped(text, byte_idx) {
            if c == '[' {
                if is_function_call_bracket(text, byte_idx) {
                    current_depth += 1;
                }
            } else if c == ']' {
                if current_depth > 0 {
                    current_depth -= 1;
                }
            }
        }
    }
    current_depth
}
