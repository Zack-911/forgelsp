//! # Utility Functions Module
//!
//! Provides helper functions for:
//! - Loading and parsing `forgeconfig.json` configuration files
//! - Transforming GitHub shorthand URLs to raw githubusercontent URLs
//! - Asynchronous LSP client logging

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tower_lsp::Client;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;

/// Spawns an asynchronous task to log a message to the LSP client.
///
/// This is non-blocking and errors are ignored (logging failures don't affect functionality).
///
/// # Arguments
/// * `client` - LSP client to send the log message to
/// * `ty` - Message type (INFO, WARNING, ERROR, LOG)
/// * `msg` - Message content
pub fn spawn_log(client: Client, ty: MessageType, msg: String) {
    tokio::spawn(async move {
        let () = client.log_message(ty, msg).await;
    });
}

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CustomFunction {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub params: Option<JsonValue>, // Can be array of objects or array of strings
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub intents: Option<Vec<String>>,
}

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

/// Loads `forgeconfig.json` from any workspace folder.
/// Supports GitHub shorthand entries like:
///   github:owner/repo
///   github:owner/repo#branch
///   github:owner/repo/path#branch
///
/// These are expanded to:
///   `<https://raw.githubusercontent.com/owner/repo/<branch>/forge.json>`
pub fn load_forge_config(workspace_folders: &[PathBuf]) -> Option<Vec<String>> {
    load_forge_config_full(workspace_folders).map(|(cfg, _)| cfg.urls)
}

/// Loads the full `forgeconfig.json` configuration from any workspace folder.
/// Returns the config and the path to the directory containing the config file.
pub fn load_forge_config_full(workspace_folders: &[PathBuf]) -> Option<(ForgeConfig, PathBuf)> {
    for folder in workspace_folders {
        // defined priority: 1. root, 2. .vscode
        let possible_paths = [folder.join("forgeconfig.json"), folder.join(".vscode").join("forgeconfig.json")];

        for path in possible_paths {
            if !path.exists() {
                continue;
            }

            let Ok(data) = fs::read_to_string(&path) else {
                continue;
            };

            let Ok(mut raw) = serde_json::from_str::<ForgeConfig>(&data) else {
                continue;
            };

            // Transform shorthand URLs into fully-qualified URLs
            raw.urls = raw.urls.into_iter().map(resolve_github_shorthand).collect();

            // Return the config and the *directory* containing it
            return Some((raw, path.parent().unwrap().to_path_buf()));
        }
    }

    None
}

/// Converts GitHub shorthands into full raw URLs.
/// Examples:
///   github:owner/repo
///   github:owner/repo#dev
///   github:owner/repo/path/to/file.json
///
/// Output:
///   `<https://raw.githubusercontent.com/owner/repo/<branch>/path/to/file.json>`
fn resolve_github_shorthand(input: String) -> String {
    // Only rewrite GitHub-style keys. Leave normal URLs untouched.
    if !input.starts_with("github:") {
        return input;
    }

    // Remove the "github:" prefix
    let Some(trimmed) = input.strip_prefix("github:") else {
        return input;
    };

    // Split branch if provided (default to "main" if not specified)
    let (path, branch) = match trimmed.split_once('#') {
        Some((p, b)) => (p, b),
        Option::None => (trimmed, "main"), // default branch
    };

    // Parse owner/repo/path structure
    // Expected format: owner/repo or owner/repo/custom/path
    let parts: Vec<&str> = path.split('/').collect();

    if parts.len() < 2 {
        // Invalid format, return as-is
        return input;
    }

    let owner = parts[0];
    let repo = parts[1];

    // If there's a file path specified, use it; otherwise default to metadata/functions.json
    let file_path = if parts.len() > 2 {
        parts[2..].join("/")
    } else {
        "metadata/functions.json".to_string()
    };

    // Construct the raw.githubusercontent.com URL
    format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{file_path}")
}

pub fn is_escaped(code: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 || !code.is_char_boundary(byte_idx) {
        return false;
    }

    let bytes = code.as_bytes();
    let target = bytes[byte_idx];

    // Count consecutive backslashes immediately before byte_idx
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

/// Checks if the bracket at `bracket_idx` is preceded by a function pattern `$name`.
pub fn is_function_call_bracket(text: &str, bracket_idx: usize) -> bool {
    if bracket_idx == 0 || text.as_bytes().get(bracket_idx) != Some(&b'[') {
        return false;
    }

    let mut i = bracket_idx;
    let bytes = text.as_bytes();
    
    // Step 1: Skip alphanumeric function name
    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }

    // Step 2: Skip modifiers: !, #, @[...]
    while i > 0 && matches!(bytes[i - 1], b'!' | b'#' | b'@' | b']') {
        match bytes[i - 1] {
            b'!' | b'#' => i -= 1,
            b']' => {
                // Skip bracketed count: @[10]
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
                i -= 1; // Skip '@'
            }
            b'@' => {
                // Lone '@' is not a modifier here (it expects [])
                break;
            }
            _ => break,
        }
    }
    
    // Step 3: Check for leading $ and ensure it's not escaped
    i > 0 && bytes[i - 1] == b'$' && !is_escaped(text, i - 1)
}

/// Finds the matching closing bracket `]` for an opening bracket `[` at `open_idx`.
/// This version respects ForgeScript escape sequences (2 backslashes for brackets).
/// Also skips over escape functions ($esc, $escape, $escapeCode) entirely.
/// ONLY increments depth for $function[...] and the initial open bracket.
pub fn find_matching_bracket(code: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    let mut iter = code.char_indices().skip_while(|&(idx, _)| idx < open_idx);

    while let Some((i, c)) = iter.next() {
        if is_escaped(code, i) {
            continue;
        }

        if c == '[' {
            // Always increment for the initial bracket that started the search.
            // For subsequent brackets, only increment if they are part of a function call.
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
                // Skip the entire escape function
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

/// Detect if we're at the start of an escape function and return its end position.
/// Returns None if not at an escape function.
/// This helps bracket matchers skip escape function contents entirely.
pub fn find_escape_function_end(code: &str, dollar_idx: usize) -> Option<usize> {
    let bytes = code.as_bytes();

    // Check if we're at a $ that's not escaped
    if dollar_idx >= code.len() || bytes[dollar_idx] != b'$' {
        return None;
    }

    if is_escaped(code, dollar_idx) {
        return None;
    }

    // Skip $ and any modifiers (!, #)
    let mut pos = dollar_idx + 1;
    while pos < bytes.len() && (bytes[pos] == b'!' || bytes[pos] == b'#') {
        pos += 1;
    }

    // Read function name
    let name_start = pos;
    while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
        pos += 1;
    }

    if pos == name_start {
        return None; // No function name
    }

    let name = &code[name_start..pos];
    if !is_escape_function(name) {
        return None; // Not an escape function
    }

    // Check for opening bracket
    if pos >= bytes.len() || bytes[pos] != b'[' {
        return None; // Escape function must have brackets
    }

    // Find the matching bracket using raw matching (no escape handling)
    find_matching_bracket_raw(bytes, pos)
}

/// Finds the matching closing bracket `]` for an opening bracket `[` at `open_idx`.
/// This does NOT handle escape sequences and is used for raw content.
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

/// Converts a byte offset within a string to an LSP Position (line and character).
pub fn offset_to_position(text: &str, offset: usize) -> Position {
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

    Position::new(line, col)
}

/// Converts an LSP Position (line, character) to a byte offset within the text.
pub fn position_to_offset(text: &str, position: Position) -> Option<usize> {
    let mut current_offset = 0;

    for (line_num, line) in text.split_inclusive('\n').enumerate() {
        if line_num as u32 == position.line {
            let mut col = 0;
            for (i, c) in line.char_indices() {
                if col == position.character {
                    return Some(current_offset + i);
                }
                col += c.len_utf16() as u32;
            }
            if col == position.character {
                return Some(current_offset + line.len());
            }
            return None;
        }
        current_offset += line.len();
    }
    None
}

/// Checks if the function name is an escape function.
pub fn is_escape_function(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "esc" || lower == "escape" || lower == "escapecode"
}

/// Returns the index after modifiers (!, #, @[...]).
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
/// Calculates the nesting depth at a given byte offset by counting unclosed, non-escaped brackets.
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