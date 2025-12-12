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
    let bytes = source.as_bytes();

    // Extract code blocks manually (handle escaped backticks)
    let mut i = 0;
    while i < bytes.len() {
        // Look for "code:" pattern
        if i + 5 <= bytes.len() && &source[i..i + 5] == "code:" {
            let mut j = i + 5;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }

            // Check for opening backtick
            if j < bytes.len() && bytes[j] == b'`' {
                j += 1;
                let content_start = j;

                // Find closing backtick (respecting escapes)
                let mut found_end = false;
                while j < bytes.len() {
                    if bytes[j] == b'\\' && j + 1 < bytes.len() {
                        j += 2;
                        continue;
                    }
                    if bytes[j] == b'`' {
                        found_end = true;
                        break;
                    }
                    j += 1;
                }

                if found_end {
                    let code = &source[content_start..j];
                    let found_tokens = extract_tokens_from_code(
                        code,
                        content_start,
                        use_function_colors,
                        manager.clone(),
                    );
                    tokens.extend(found_tokens);
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
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
            while j < char_positions.len()
                && (char_positions[j].1 == '!' || char_positions[j].1 == '#')
            {
                j += 1;
            }

            // Try matching character by character
            while j < char_positions.len()
                && (char_positions[j].1.is_alphanumeric() || char_positions[j].1 == '_')
            {
                let end_byte_idx = char_positions[j].0 + char_positions[j].1.len_utf8();
                if let Some(candidate) = code.get(i..end_byte_idx) {
                    if manager.get(candidate).is_some() {
                        best_match_len = end_byte_idx - i;
                        best_match_char_count = j - idx + 1;
                    }
                }
                j += 1;
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
            col += 1;
        }
    }

    Position::new(line, col)
}
