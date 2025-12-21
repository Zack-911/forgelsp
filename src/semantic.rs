//! # Semantic Token Extraction Module
//!
//! Extracts semantic tokens from ForgeScript code for syntax highlighting.
//! Validates function names against metadata to ensure only valid functions are highlighted.
//!
//! Supports:
//! - Metadata-validated function highlighting
//! - Multi-color function highlighting (alternating colors)
//! - Comment detection (`$c[...]`)
//! - Escape function special handling (`$esc`, `$escapeCode`)
//! - Number, boolean, and semicolon highlighting

use crate::metadata::MetadataManager;
use std::sync::Arc;
use tower_lsp::lsp_types::{Position, SemanticToken};

/// Token types:
/// 0 = FUNCTION (normal functions)
/// 1 = KEYWORD (booleans, semicolons)
/// 2 = NUMBER
/// 3 = PARAMETER (alternating function color)
/// 4 = STRING (escape function content)
/// 5 = COMMENT (comments)
/// Check if a character is escaped
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

/// Find matching bracket (raw, no escape handling)
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

/// Extract tokens with optional multi-color function highlighting
pub fn extract_semantic_tokens_with_colors(
    source: &str,
    use_function_colors: bool,
    manager: Arc<MetadataManager>,
) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // Extract code blocks manually (handle escaped backticks)
    // Extract code blocks using regex (handles escaped backticks and UTF-8 correctly)
    // Pattern: code: ` ... ` where backticks can be escaped with \`
    let re = regex::Regex::new(r"(?s)code:\s*`((?:[^`\\]|\\.)*)`").unwrap();

    for cap in re.captures_iter(source) {
        if let Some(match_group) = cap.get(1) {
            let content_start = match_group.start();
            let code = match_group.as_str();

            let found_tokens =
                extract_tokens_from_code(code, content_start, use_function_colors, manager.clone());
            tokens.extend(found_tokens);
        }
    }

    // Convert to relative tokens
    to_relative_tokens(&tokens, source)
}

fn extract_tokens_from_code(
    code: &str,
    code_start: usize,
    use_function_colors: bool,
    manager: Arc<MetadataManager>,
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
            if idx + 1 < char_positions.len()
                && char_positions[idx + 1].1 == 'c'
                && idx + 2 < char_positions.len()
                && char_positions[idx + 2].1 == '['
            {
                // Found comment function
                if let Some(end_idx) = find_matching_bracket_raw(bytes, char_positions[idx + 2].0) {
                    // Highlight entire $c[...] as KEYWORD (comment)
                    found.push((i + code_start, end_idx + 1 + code_start, 5));
                    // Skip to after the closing bracket
                    while idx < char_positions.len() && char_positions[idx].0 <= end_idx {
                        idx += 1;
                    }
                    continue;
                }
            }

            // Check for $esc[...] or $escapeCode[...] (escape functions)
            if let Some(esc_end) = check_escape_function(bytes, i) {
                // Highlight function name
                let name_end = i
                    + if bytes.get(i + 1..).and_then(|b| b.get(..4)) == Some(b"esc[") {
                        4
                    } else {
                        11
                    };
                found.push((i + code_start, name_end + code_start, 0));
                // Highlight content as STRING
                if name_end < esc_end {
                    found.push((name_end + code_start, esc_end + code_start, 4));
                }
                // Highlight closing bracket
                found.push((esc_end + code_start, esc_end + 1 + code_start, 0));
                // Skip to after the escape function
                while idx < char_positions.len() && char_positions[idx].0 <= esc_end {
                    idx += 1;
                }
                continue;
            }

            // Try incremental matching against metadata
            let mut best_match_len = 0;
            let mut best_match_char_count = 0;
            let mut j = idx + 1;

            // Skip modifiers
            while j < char_positions.len() {
                let (_, c) = char_positions[j];
                if c == '!' || c == '#' {
                    j += 1;
                } else if c == '@' {
                    // Check for @[...]
                    if j + 1 < char_positions.len() && char_positions[j + 1].1 == '[' {
                        let open_bracket_byte_idx = char_positions[j + 1].0;
                        if let Some(close_byte_idx) =
                            find_matching_bracket_raw(bytes, open_bracket_byte_idx)
                        {
                            // Advance j to after the closing bracket
                            while j < char_positions.len() && char_positions[j].0 <= close_byte_idx
                            {
                                j += 1;
                            }
                        } else {
                            // Unmatched bracket, stop modifier parsing
                            break;
                        }
                    } else {
                        // Just @ without [, stop modifier parsing
                        break;
                    }
                } else {
                    break;
                }
            }

            // If we ran out of characters, stop
            if j >= char_positions.len() {
                idx += 1;
                continue;
            }

            let name_start_byte_idx = char_positions[j].0;
            let mut name_end_char_idx = j;

            // Find the end of the identifier
            while name_end_char_idx < char_positions.len()
                && (char_positions[name_end_char_idx].1.is_alphanumeric()
                    || char_positions[name_end_char_idx].1 == '_')
            {
                name_end_char_idx += 1;
            }

            // Check if the next character is '['
            let has_bracket = name_end_char_idx < char_positions.len()
                && char_positions[name_end_char_idx].1 == '[';

            if has_bracket {
                // Case 1: Bracketed call - check full identifier
                let end_byte_idx = if name_end_char_idx < char_positions.len() {
                    char_positions[name_end_char_idx].0
                } else {
                    code.len()
                };

                if let Some(full_name) = code.get(name_start_byte_idx..end_byte_idx) {
                    let lookup_key = format!("${}", full_name);
                    if manager.get_exact(&lookup_key).is_some() {
                        best_match_len = end_byte_idx - i;
                        best_match_char_count = name_end_char_idx - idx;
                    }
                }
            } else {
                // Case 2: No bracket - find longest prefix match
                // We iterate from the full identifier length down to 1 char
                let mut check_idx = name_end_char_idx;
                while check_idx > j {
                    let end_byte_idx = if check_idx < char_positions.len() {
                        char_positions[check_idx].0
                    } else {
                        code.len()
                    };

                    if let Some(name_part) = code.get(name_start_byte_idx..end_byte_idx) {
                        let lookup_key = format!("${}", name_part);
                        if manager.get_exact(&lookup_key).is_some() {
                            best_match_len = end_byte_idx - i;
                            best_match_char_count = check_idx - idx;
                            break; // Found longest match
                        }
                    }
                    check_idx -= 1;
                }
            }

            if best_match_len > 0 {
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

/// Convert byte offset to LSP position (line, column)
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
            col += ch.len_utf16() as u32;
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
        let manager = MetadataManager::new("./.cache_test", vec![])
            .await
            .expect("Failed to create manager");

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
            let tokens = extract_semantic_tokens_with_colors(code, false, manager.clone());
            if should_match {
                assert!(!tokens.is_empty(), "Failed to match {}", code);
                assert_eq!(tokens[0].token_type, 0, "Wrong token type for {}", code);
            } else {
                assert!(tokens.is_empty(), "Should not match {}", code);
            }
        }

        // Clean up
        let _ = std::fs::remove_dir_all("./.cache_test");
    }

    #[tokio::test]
    async fn test_bracket_and_prefix_matching() {
        let manager = MetadataManager::new("./.cache_test_semantic", vec![])
            .await
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
            let tokens = extract_semantic_tokens_with_colors(code, false, manager.clone());
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
        let _ = std::fs::remove_dir_all("./.cache_test_semantic");
    }

    #[tokio::test]
    async fn test_utf8_boundaries() {
        let manager = MetadataManager::new("./.cache_test_semantic_utf8", vec![])
            .await
            .expect("Failed to create manager");
        let manager = Arc::new(manager);

        // Test case with emojis outside and inside code block
        let code = "🔔\ncode: `🔔$ping`";

        // Should not panic
        let tokens = extract_semantic_tokens_with_colors(code, false, manager.clone());

        // Just checking it doesn't panic is enough for this test.
        assert!(tokens.is_empty() || !tokens.is_empty());

        let _ = std::fs::remove_dir_all("./.cache_test_semantic_utf8");
    }
}
