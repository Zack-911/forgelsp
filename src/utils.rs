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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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

/// Check if a character at the given byte index is escaped.
/// For backtick: 1 backslash escapes it (\`)
/// For special chars ($, ;, [, ]): 2 backslashes escape it (\\$, \\;, etc.)
pub fn is_escaped(code: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 || !code.is_char_boundary(byte_idx) {
        return false;
    }

    let bytes = code.as_bytes();
    let c = bytes[byte_idx];

    // For backtick, check if there's exactly 1 backslash before it
    if c == b'`' {
        if byte_idx >= 1 && bytes[byte_idx - 1] == b'\\' {
            let mut backslash_count = 1;
            let mut pos = byte_idx - 1;
            while pos > 0 {
                pos -= 1;
                if bytes[pos] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            return backslash_count % 2 == 1;
        }
        return false;
    }

    // For special chars ($, ;, [, ]), check if there are exactly 2 backslashes before it
    if matches!(c, b'$' | b';' | b'[' | b']') {
        if byte_idx >= 2 && bytes[byte_idx - 1] == b'\\' && bytes[byte_idx - 2] == b'\\' {
            let mut backslash_count = 2;
            let mut pos = byte_idx - 2;
            while pos > 0 {
                pos -= 1;
                if bytes[pos] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            return backslash_count == 2 || backslash_count % 2 == 0;
        }
        return false;
    }

    false
}

/// Finds the matching closing bracket `]` for an opening bracket `[` at `open_idx`.
/// This version respects ForgeScript escape sequences (2 backslashes for brackets).
/// Also skips over escape functions ($esc, $escape, $escapeCode) entirely.
pub fn find_matching_bracket(code: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    let mut iter = code.char_indices().skip_while(|&(idx, _)| idx < open_idx);

    while let Some((i, c)) = iter.next() {
        if is_escaped(code, i) {
            continue;
        }

        // Check if we're at the start of an escape function
        if c == '$' {
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

        if c == '[' {
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_escaped() {
        let code = r"foo \\$ bar";
        assert!(is_escaped(code, 6)); // $ is at index 6, preceded by \\
        
        let code2 = r"foo \$ bar";
        assert!(!is_escaped(code2, 5)); // $ is at index 5, only 1 \ before it (not enough for $)
        
        let code3 = r"print\\[\\]";
        // p r i n t \ \ [ \ \ ]
        // 0 1 2 3 4 5 6 7 8 9 0
        assert!(is_escaped(code3, 7)); // [ is at 7
        assert!(is_escaped(code3, 10)); // ] is at 10
    }

    #[test]
    fn test_find_matching_bracket() {
        // Simple case
        let code = "$foo[bar]";
        assert_eq!(find_matching_bracket(code, 4), Some(8));

        // Escaped bracket inside
        let code2 = r"$foo[bar\\[baz]";
        // \ is at 8, \ at 9, [ at 10. So total length is 15.
        // $ f o o [ b a r \  \  [  b  a  z  ]
        // 0 1 2 3 4 5 6 7 8  9  10 11 12 13 14
        assert_eq!(find_matching_bracket(code2, 4), Some(14));

        // Escaped closing bracket
        let code3 = r"$foo[bar\\]baz]";
        assert_eq!(find_matching_bracket(code3, 4), Some(14));

        // Nested brackets
        let code4 = "$foo[bar[baz]]";
        assert_eq!(find_matching_bracket(code4, 4), Some(13));
        
        // Escape function inside
        let code5 = "$foo[bar$esc[ignore me]]";
        assert_eq!(find_matching_bracket(code5, 4), Some(23));
    }

    #[test]
    fn test_reported_issue() {
        let code = r#"$advancedTextSplit[$httpResult;"videoData":\\[;1;,"player_version;0]"#;
        // The first [ is at index 18 (after $advancedTextSplit)
        assert_eq!(find_matching_bracket(code, 18), Some(code.len() - 1));
    }
}

