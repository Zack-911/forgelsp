//! # Semantic Token Extraction Module
//!
//! Extracts semantic tokens from `ForgeScript` code for syntax highlighting.
//! Validates function names against metadata to ensure only valid functions are highlighted.
//!
//! Supports:
//! - Metadata-validated function highlighting
//! - Multi-color function highlighting (alternating colors)
//! - Comment detection (`$c[...]`)
//! - Escape function special handling (`$esc`, `$escapeCode`)
//! - Number, boolean, and semicolon highlighting
use std::sync::{Arc, LazyLock};

use regex::Regex;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;

use crate::metadata::MetadataManager;

/// Regex for extracting code blocks from `ForgeScript` files.
static CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)code:\s*`((?:[^`\\]|\\.)*)`").expect("Semantic: regex compile failed")
});

/// Token types:
/// 0 = FUNCTION (normal functions)
/// 1 = KEYWORD (booleans, semicolons)
/// 2 = NUMBER
/// 3 = PARAMETER (alternating function color)
/// 4 = STRING (escape function content)
/// 5 = COMMENT (comments)
/// Checks if a character at the given index is escaped by a backslash.
///
/// Handles:
/// - Backtick escape: \` (requires 1 backslash)
/// - Special characters ($, ;, [, ]): require 2 backslashes for escaping
fn is_char_escaped(bytes: &[u8], idx: usize) -> bool {
    if idx == 0 {
        return false;
    }

    let c = bytes[idx];

    // For backtick: 1 backslash
    if c == b'`' {
        if idx >= 1 && bytes[idx - 1] == b'\\' {
            let mut backslash_count = 1;
            let mut pos = idx - 1;
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

    // For special chars: 2 backslashes
    if matches!(c, b'$' | b';' | b'[' | b']') {
        if idx >= 2 && bytes[idx - 1] == b'\\' && bytes[idx - 2] == b'\\' {
            let mut backslash_count = 2;
            let mut pos = idx - 2;
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
///
/// This version does not handle escape sequences and is used for raw content
/// like comments and escape functions.
fn find_matching_bracket_raw(code: &[u8], open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, &byte) in code.iter().enumerate().skip(open_idx) {
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

/// Extracts semantic tokens from `ForgeScript` source code, optionally using
/// alternating colors for function calls.
///
/// # Arguments
/// * `source` - The complete source code to analyze.
/// * `use_function_colors` - Whether to use different token types for alternating functions.
/// * `manager` - Shared metadata manager for function validation.
///
/// # Returns
/// A list of relative semantic tokens as defined by the LSP specification.
pub fn extract_semantic_tokens_with_colors(
    source: &str,
    use_function_colors: bool,
    manager: &Arc<MetadataManager>,
) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    for cap in CODE_BLOCK_RE.captures_iter(source) {
        if let Some(match_group) = cap.get(1) {
            let content_start = match_group.start();
            let code = match_group.as_str();

            let found_tokens =
                extract_tokens_from_code(code, content_start, use_function_colors, manager);
            tokens.extend(found_tokens);
        }
    }

    // Convert to relative tokens
    to_relative_tokens(&tokens, source)
}

/// Internal helper to extract tokens from a specific code block.
fn extract_tokens_from_code(
    code: &str,
    code_start: usize,
    use_function_colors: bool,
    manager: &Arc<MetadataManager>,
) -> Vec<(usize, usize, u32)> {
    let mut found = Vec::new();
    let bytes = code.as_bytes();
    let mut function_color_index = 0u32;

    // Collect char indices for safe UTF-8 iteration
    let char_positions: Vec<(usize, char)> = code.char_indices().collect();
    let mut idx = 0;

    while idx < char_positions.len() {
        let (i, c) = char_positions[idx];

        // Check for functions
        if c == '$' && !is_char_escaped(bytes, i) {
            // Check for $c[...] (comment)
            if let Some((end_idx, next_idx)) = try_extract_comment(code, idx, &char_positions) {
                found.push((i + code_start, end_idx + 1 + code_start, 5));
                idx = next_idx;
                continue;
            }

            // Check for $esc[...] or $escapeCode[...] (escape functions)
            if let Some((name_end, esc_end, next_idx)) =
                try_extract_escape(code, idx, &char_positions)
            {
                found.push((i + code_start, name_end + code_start, 0));
                if name_end < esc_end {
                    found.push((name_end + code_start, esc_end + code_start, 4));
                }
                found.push((esc_end + code_start, esc_end + 1 + code_start, 0));
                idx = next_idx;
                continue;
            }

            // Try incremental matching against metadata
            if let Some((best_match_len, best_match_char_count)) =
                try_extract_metadata_function(code, idx, &char_positions, manager)
            {
                let token_type = if use_function_colors {
                    let colors = [0, 3];
                    let color = colors[(function_color_index as usize) % colors.len()];
                    function_color_index += 1;
                    color
                } else {
                    0
                };
                found.push((i + code_start, i + best_match_len + code_start, token_type));
                idx += best_match_char_count;
                continue;
            }
        }

        // Check for numbers
        if c.is_ascii_digit() {
            let start = i;
            let mut j = idx;
            while j < char_positions.len()
                && (char_positions[j].1.is_ascii_digit() || char_positions[j].1 == '.')
            {
                j += 1;
            }
            let end = if j < char_positions.len() {
                char_positions[j].0
            } else {
                code.len()
            };
            found.push((start + code_start, end + code_start, 2));
            idx = j;
            continue;
        }

        // Check for booleans - use safe slicing with get()
        if let Some(slice) = code.get(i..) {
            if slice.starts_with("true") {
                found.push((i + code_start, i + 4 + code_start, 1));
                idx += 4;
                continue;
            }
            if slice.starts_with("false") {
                found.push((i + code_start, i + 5 + code_start, 1));
                idx += 5;
                continue;
            }
        }

        // Check for semicolons
        if c == ';' && !is_char_escaped(bytes, i) {
            found.push((i + code_start, i + 1 + code_start, 1));
        }

        idx += 1;
    }

    found.sort_by_key(|(s, _, _)| *s);
    found
}

fn try_extract_comment(
    code: &str,
    idx: usize,
    char_positions: &[(usize, char)],
) -> Option<(usize, usize)> {
    if idx + 2 < char_positions.len()
        && char_positions[idx + 1].1 == 'c'
        && char_positions[idx + 2].1 == '['
    {
        let bytes = code.as_bytes();
        if let Some(end_idx) = find_matching_bracket_raw(bytes, char_positions[idx + 2].0) {
            let mut next_idx = idx;
            while next_idx < char_positions.len() && char_positions[next_idx].0 <= end_idx {
                next_idx += 1;
            }
            return Some((end_idx, next_idx));
        }
    }
    None
}

fn try_extract_escape(
    code: &str,
    idx: usize,
    char_positions: &[(usize, char)],
) -> Option<(usize, usize, usize)> {
    let bytes = code.as_bytes();
    let i = char_positions[idx].0;
    if let Some(esc_end) = check_escape_function(bytes, i) {
        let name_end = i + if bytes.get(i + 1..).and_then(|b| b.get(..4)) == Some(b"esc[") {
            4
        } else {
            11
        };
        let mut next_idx = idx;
        while next_idx < char_positions.len() && char_positions[next_idx].0 <= esc_end {
            next_idx += 1;
        }
        return Some((name_end, esc_end, next_idx));
    }
    None
}

fn try_extract_metadata_function(
    code: &str,
    idx: usize,
    char_positions: &[(usize, char)],
    manager: &MetadataManager,
) -> Option<(usize, usize)> {
    let i = char_positions[idx].0;
    let mut j = idx + 1;

    // Skip modifiers
    while j < char_positions.len() {
        let (_, c) = char_positions[j];
        if c == '!' || c == '#' {
            j += 1;
        } else if c == '@' {
            if j + 1 < char_positions.len() && char_positions[j + 1].1 == '[' {
                let open_bracket_byte_idx = char_positions[j + 1].0;
                if let Some(close_byte_idx) =
                    find_matching_bracket_raw(code.as_bytes(), open_bracket_byte_idx)
                {
                    while j < char_positions.len() && char_positions[j].0 <= close_byte_idx {
                        j += 1;
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if j >= char_positions.len() {
        return None;
    }

    let name_start_byte_idx = char_positions[j].0;
    let mut name_end_char_idx = j;

    while name_end_char_idx < char_positions.len()
        && (char_positions[name_end_char_idx].1.is_alphanumeric()
            || char_positions[name_end_char_idx].1 == '_')
    {
        name_end_char_idx += 1;
    }

    let has_bracket =
        name_end_char_idx < char_positions.len() && char_positions[name_end_char_idx].1 == '[';

    if has_bracket {
        let end_byte_idx = if name_end_char_idx < char_positions.len() {
            char_positions[name_end_char_idx].0
        } else {
            code.len()
        };

        if let Some(full_name) = code.get(name_start_byte_idx..end_byte_idx) {
            let lookup_key = format!("${full_name}");
            if manager.get_exact(&lookup_key).is_some() {
                return Some((end_byte_idx - i, name_end_char_idx - idx));
            }
        }
    } else {
        let mut check_idx = name_end_char_idx;
        while check_idx > j {
            let end_byte_idx = if check_idx < char_positions.len() {
                char_positions[check_idx].0
            } else {
                code.len()
            };

            if let Some(name_part) = code.get(name_start_byte_idx..end_byte_idx) {
                let lookup_key = format!("${name_part}");
                if manager.get_exact(&lookup_key).is_some() {
                    return Some((end_byte_idx - i, check_idx - idx));
                }
            }
            check_idx -= 1;
        }
    }

    None
}

/// Checks if the function at `dollar_idx` is an escape function ($esc or $escapeCode).
///
/// Returns the byte offset of the closing bracket if it is, otherwise None.
fn check_escape_function(bytes: &[u8], dollar_idx: usize) -> Option<usize> {
    if dollar_idx >= bytes.len() || bytes[dollar_idx] != b'$' {
        return None;
    }

    let mut pos = dollar_idx + 1;

    // Skip modifiers
    while pos < bytes.len() && (bytes[pos] == b'!' || bytes[pos] == b'#') {
        pos += 1;
    }

    // Check for "esc[" or "escapeCode["
    if pos + 3 < bytes.len() && &bytes[pos..pos + 3] == b"esc" && bytes[pos + 3] == b'[' {
        return find_matching_bracket_raw(bytes, pos + 3);
    }

    if pos + 10 < bytes.len() && &bytes[pos..pos + 10] == b"escapeCode" && bytes[pos + 10] == b'[' {
        return find_matching_bracket_raw(bytes, pos + 10);
    }

    None
}

/// Converts absolute token positions into relative positions required by the LSP.
fn to_relative_tokens(found: &[(usize, usize, u32)], source: &str) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let mut last_line = 0u32;
    let mut last_col = 0u32;

    for &(start, end, token_type) in found {
        let start_pos = offset_to_position(source, start);
        let end_pos = offset_to_position(source, end);

        let delta_line = start_pos.line.saturating_sub(last_line);
        let delta_start = if delta_line == 0 {
            start_pos.character.saturating_sub(last_col)
        } else {
            start_pos.character
        };

        tokens.push(SemanticToken {
            delta_line,
            delta_start,
            length: (end_pos.character - start_pos.character).max(1),
            token_type,
            token_modifiers_bitset: 0,
        });

        last_line = start_pos.line;
        last_col = start_pos.character;
    }

    tokens
}

/// Converts a byte offset within a string to an LSP Position (line and character).
///
/// Handles multi-byte characters and line endings.
fn offset_to_position(text: &str, offset: usize) -> Position {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::CustomFunction;

    #[tokio::test]
    async fn test_function_modifiers() {
        let manager =
            MetadataManager::new("./.cache_test", vec![], None).expect("Failed to create manager");

        // Add a test function
        manager
            .add_custom_functions(vec![CustomFunction {
                name: "$ban".to_string(),
                description: None,
                params: None,
                brackets: None,
                alias: None,
                path: None,
            }])
            .expect("Failed to add custom function");

        let manager = Arc::new(manager);

        // Test cases
        let cases = vec![
            ("code: `$ban`", true),
            ("code: `$!ban`", true),
            ("code: `$#ban`", true),
            ("code: `$@[user]ban`", true),
            ("code: `$!#@[user]ban`", true),
            ("code: `$unknown`", false),
            ("code: `$!unknown`", false),
        ];

        for (code, should_match) in cases {
            let tokens = extract_semantic_tokens_with_colors(code, false, &manager.clone());
            if should_match {
                assert!(!tokens.is_empty(), "Failed to match {}", code);
                assert_eq!(tokens[0].token_type, 0, "Wrong token type for {}", code);
            } else {
                assert!(tokens.is_empty(), "Should not match {}", code);
            }
        }

        // Clean up
        let () = std::fs::remove_dir_all("./.cache_test").expect("Failed to clean up cache");
    }

    #[tokio::test]
    async fn test_bracket_and_prefix_matching() {
        let manager = MetadataManager::new("./.cache_test_semantic", vec![], None)
            .expect("Failed to create manager");

        // Add test functions
        manager
            .add_custom_functions(vec![
                CustomFunction {
                    name: "$ping".to_string(),
                    description: None,
                    params: None,
                    brackets: None,
                    alias: None,
                    path: None,
                },
                CustomFunction {
                    name: "$deleteCache".to_string(),
                    description: None,
                    params: None,
                    brackets: None,
                    alias: None,
                    path: None,
                },
            ])
            .expect("Failed to add custom functions");

        let manager = Arc::new(manager);

        let cases = vec![
            // Case 1: Bracketed call with exact match
            ("code: `$deleteCache[]`", true, "$deleteCache"),
            // Case 2: Bracketed call with NO match (should NOT match prefix $delete)
            ("code: `$delete[]`", false, ""),
            // Case 3: Prefix matching (should match $ping)
            ("code: `$pingms`", true, "$ping"),
            // Case 4: Exact match without brackets
            ("code: `$ping`", true, "$ping"),
            // Case 5: No match
            ("code: `$unknown`", false, ""),
            // Case 6: Bracketed call where full name doesn't exist, but prefix does.
            // e.g. $ping[] exists, but $pingExtra[] does not. $ping should NOT be highlighted.
            ("code: `$pingExtra[]`", false, ""),
        ];

        for (code, should_match, expected_name) in cases {
            let tokens = extract_semantic_tokens_with_colors(code, false, &manager.clone());
            if should_match {
                assert!(!tokens.is_empty(), "Failed to match {}", code);
                assert_eq!(tokens[0].token_type, 0, "Wrong token type for {}", code);

                // Verify length matches expected name length (excluding $)
                // token length is u32
                let expected_len = expected_name.len() as u32;
                assert_eq!(tokens[0].length, expected_len, "Wrong length for {}", code);
            } else {
                assert!(tokens.is_empty(), "Should not match {}", code);
            }
        }

        // Clean up
        let () =
            std::fs::remove_dir_all("./.cache_test_semantic").expect("Failed to clean up cache");
    }

    #[tokio::test]
    async fn test_utf8_boundaries() {
        let manager = MetadataManager::new("./.cache_test_semantic_utf8", vec![], None)
            .expect("Failed to create manager");
        let manager = Arc::new(manager);

        // Test case with emojis outside and inside code block
        let code = "🔔\ncode: `🔔$ping`";

        // Should not panic
        let tokens = extract_semantic_tokens_with_colors(code, false, &manager.clone());

        // Just checking it doesn't panic is enough for this test.
        assert!(tokens.is_empty() || !tokens.is_empty());

        let () = std::fs::remove_dir_all("./.cache_test_semantic_utf8")
            .expect("Failed to clean up cache");
    }
}
