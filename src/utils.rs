//! Utility functions for ForgeLSP.
//!
//! Includes logic for configuration loading, GitHub URL resolution, 
//! asynchronous logging, and ForgeScript-specific string manipulation.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tower_lsp::Client;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;

/// Spawns a non-blocking task to send a log message to the LSP client.
pub fn spawn_log(client: Client, ty: MessageType, msg: String) {
    tokio::spawn(async move {
        let () = client.log_message(ty, msg).await;
    });
}

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
}

/// Attempts to find and load the ForgeLSP configuration from the workspace.
pub fn load_forge_config(workspace_folders: &[PathBuf]) -> Option<Vec<String>> {
    load_forge_config_full(workspace_folders).map(|(cfg, _)| cfg.urls)
}

/// Recursively looks for forgeconfig.json in the workspace roots or .vscode directories.
pub fn load_forge_config_full(workspace_folders: &[PathBuf]) -> Option<(ForgeConfig, PathBuf)> {
    for folder in workspace_folders {
        let possible_paths = [
            folder.join("forgeconfig.json"),
            folder.join(".vscode").join("forgeconfig.json")
        ];

        for path in possible_paths {
            if !path.exists() { continue; }

            let Ok(data) = fs::read_to_string(&path) else { continue; };
            let Ok(mut raw) = serde_json::from_str::<ForgeConfig>(&data) else { continue; };

            raw.urls = raw.urls.into_iter().map(resolve_github_shorthand).collect();
            return Some((raw, path.parent().unwrap().to_path_buf()));
        }
    }
    None
}

/// Transforms github: shorthand into raw.githubusercontent.com URLs.
fn resolve_github_shorthand(input: String) -> String {
    if !input.starts_with("github:") { return input; }

    let Some(trimmed) = input.strip_prefix("github:") else { return input; };

    let (path, branch) = match trimmed.split_once('#') {
        Some((p, b)) => (p, b),
        Option::None => (trimmed, "main"),
    };

    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 { return input; }

    let owner = parts[0];
    let repo = parts[1];
    let file_path = if parts.len() > 2 {
        parts[2..].join("/")
    } else {
        "metadata/functions.json".to_string()
    };

    format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{file_path}")
}

/// Determines if a character at a given byte index is escaped by backslashes.
pub fn is_escaped(code: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 || !code.is_char_boundary(byte_idx) { return false; }

    let bytes = code.as_bytes();
    let target = bytes[byte_idx];
    let mut count = 0;
    let mut i = byte_idx;

    while i > 0 {
        i -= 1;
        if bytes[i] == b'\\' { count += 1; } else { break; }
    }

    if target == b'`' { count == 1 } else { count == 2 }
}

/// Verifies if a '[' character is the start of a ForgeScript function call.
pub fn is_function_call_bracket(text: &str, bracket_idx: usize) -> bool {
    if bracket_idx == 0 || text.as_bytes().get(bracket_idx) != Some(&b'[') { return false; }

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
                    if bytes[i] == b']' { depth += 1; }
                    else if bytes[i] == b'[' {
                        depth -= 1;
                        if depth == 0 { found = true; break; }
                    }
                }
                if !found || i == 0 || bytes[i - 1] != b'@' { return false; }
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
        if is_escaped(code, i) { continue; }

        if c == '[' {
            if i == open_idx || is_function_call_bracket(code, i) {
                depth += 1;
            }
        } else if c == ']' {
            if depth > 0 {
                depth -= 1;
                if depth == 0 { return Some(i); }
            }
        } else if c == '$' {
            if let Some(end_idx) = find_escape_function_end(code, i) {
                while let Some(&(idx, _)) = iter.clone().peekable().peek() {
                    if idx <= end_idx { iter.next(); } else { break; }
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

    if pos == name_start { return None; }

    let name = &code[name_start..pos];
    if !is_escape_function(name) { return None; }

    if pos >= bytes.len() || bytes[pos] != b'[' { return None; }

    find_matching_bracket_raw(bytes, pos)
}

/// Performs a raw bracket match without considering ForgeScript escapes or functions.
pub fn find_matching_bracket_raw(bytes: &[u8], open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, &byte) in bytes.iter().enumerate().skip(open_idx) {
        if byte == b'[' { depth += 1; }
        else if byte == b']' {
            depth -= 1;
            if depth == 0 { return Some(i); }
        }
    }
    None
}

/// Converts a byte offset into an LSP Position (Line/Character).
pub fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, ch) in text.char_indices() {
        if i >= offset { break; }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += u32::try_from(ch.len_utf16()).expect("UTF-16 length exceeds u32");
        }
    }
    Position::new(line, col)
}

/// Converts an LSP Position into a byte offset within the source text.
pub fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let mut current_offset = 0;

    for (line_num, line) in text.split_inclusive('\n').enumerate() {
        if line_num as u32 == position.line {
            let mut col = 0;
            for (i, c) in line.char_indices() {
                if col == position.character { return Some(current_offset + i); }
                col += c.len_utf16() as u32;
            }
            if col == position.character { return Some(current_offset + line.len()); }
            return None;
        }
        current_offset += line.len();
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
                    } else { break; }
                } else { break; }
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
        if byte_idx >= offset { break; }

        if !is_escaped(text, byte_idx) {
            if c == '[' {
                if is_function_call_bracket(text, byte_idx) { current_depth += 1; }
            } else if c == ']' {
                if current_depth > 0 { current_depth -= 1; }
            }
        }
    }
    current_depth
}