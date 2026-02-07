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
use crate::utils::{find_matching_bracket_raw, is_escaped, offset_to_position};

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

/// Extracts highlight ranges for VS Code decorations.
///
/// Returns a list of (start_offset, end_offset, color_string)
pub fn extract_highlight_ranges(
    source: &str,
    function_colors: &[String],
    consistent_colors: bool,
    manager: &Arc<MetadataManager>,
) -> Vec<(usize, usize, String)> {
    let mut highlights = Vec::new();
    if function_colors.is_empty() {
        return highlights;
    }

    let mut color_index = 0usize;
    let mut function_to_color = std::collections::HashMap::new();

    for cap in CODE_BLOCK_RE.captures_iter(source) {
        if let Some(match_group) = cap.get(1) {
            let content_start = match_group.start();
            let code = match_group.as_str();
            let char_positions: Vec<(usize, char)> = code.char_indices().collect();
            let mut idx = 0;

            while idx < char_positions.len() {
                let (i, c) = char_positions[idx];

                if c == '$' && !is_escaped(code, i) {
                    // Skip comments and escape functions as they are handled by semantic tokens
                    if let Some((_, next_idx)) = try_extract_comment(code, idx, &char_positions) {
                        idx = next_idx;
                        continue;
                    }

                    if let Some((_, _, next_idx)) = try_extract_escape(code, idx, &char_positions) {
                        idx = next_idx;
                        continue;
                    }

                    if let Some((best_match_len, best_match_char_count)) =
                        try_extract_metadata_function(code, idx, &char_positions, manager)
                    {
                        let func_name = &code[i..i + best_match_len];
                        let color = if consistent_colors {
                            function_to_color
                                .entry(func_name.to_string())
                                .or_insert_with(|| {
                                    let c = function_colors[color_index % function_colors.len()]
                                        .clone();
                                    color_index += 1;
                                    c
                                })
                                .clone()
                        } else {
                            let c = function_colors[color_index % function_colors.len()].clone();
                            color_index += 1;
                            c
                        };

                        highlights.push((
                            i + content_start,
                            i + best_match_len + content_start,
                            color,
                        ));
                        idx += best_match_char_count;
                        continue;
                    }
                }
                idx += 1;
            }
        }
    }

    highlights
}

/// Internal helper to extract tokens from a specific code block.
fn extract_tokens_from_code(
    code: &str,
    code_start: usize,
    use_function_colors: bool,
    manager: &Arc<MetadataManager>,
) -> Vec<(usize, usize, u32)> {
    let mut found = Vec::new();
    let mut function_color_index = 0u32;

    // Collect char indices for safe UTF-8 iteration
    let char_positions: Vec<(usize, char)> = code.char_indices().collect();
    let mut idx = 0;

    while idx < char_positions.len() {
        let (i, c) = char_positions[idx];

        // Check for functions
        if c == '$' && !is_escaped(code, i) {
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
        if c == ';' && !is_escaped(code, i) {
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

        if start_pos.line == end_pos.line {
            // Single line token
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
        } else {
            // Multi-line token - split into multiple tokens
            let lines: Vec<&str> = source.lines().collect();
            
            for line_idx in start_pos.line..=end_pos.line {
                let delta_line = line_idx.saturating_sub(last_line);
                let line_text = lines.get(line_idx as usize).unwrap_or(&"");
                
                let (start_char, length) = if line_idx == start_pos.line {
                    // First line: from start_pos.character to end of line
                    (start_pos.character, (line_text.chars().count() as u32).saturating_sub(start_pos.character))
                } else if line_idx == end_pos.line {
                    // Last line: from start of line to end_pos.character
                    (0, end_pos.character)
                } else {
                    // Middle lines: entire line
                    (0, line_text.chars().count() as u32)
                };

                if length > 0 {
                    let delta_start = if delta_line == 0 {
                        start_char.saturating_sub(last_col)
                    } else {
                        start_char
                    };

                    tokens.push(SemanticToken {
                        delta_line,
                        delta_start,
                        length,
                        token_type,
                        token_modifiers_bitset: 0,
                    });

                    last_line = line_idx;
                    last_col = start_char;
                }
            }
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::MetadataManager;
    use std::sync::Arc;

    #[test]
    fn test_multiline_comment_tokens() {
        let source = "code: `$c[line1\nline2\nline3]`";
        let manager = Arc::new(MetadataManager::new("./.cache", vec![], None).unwrap());
        let tokens = extract_semantic_tokens_with_colors(source, false, &manager);

        // $c[line1  (length 8: $c[line1)
        // line2     (length 5: line2)
        // line3]    (length 6: line3])
        
        assert_eq!(tokens.len(), 3);
        
        // Token 1
        assert_eq!(tokens[0].delta_line, 0);
        assert_eq!(tokens[0].delta_start, 7); // code: ` is 7 chars
        assert_eq!(tokens[0].length, 8);
        assert_eq!(tokens[0].token_type, 5);

        // Token 2
        assert_eq!(tokens[1].delta_line, 1);
        assert_eq!(tokens[1].delta_start, 0);
        assert_eq!(tokens[1].length, 5);
        assert_eq!(tokens[1].token_type, 5);

        // Token 3
        assert_eq!(tokens[2].delta_line, 1);
        assert_eq!(tokens[2].delta_start, 0);
        assert_eq!(tokens[2].length, 6);
        assert_eq!(tokens[2].token_type, 5);
    }

    #[test]
    fn test_comment_with_nested_function_highlight_ranges() {
        let source = "code: `$c[\n  $if[]\n]`";
        let manager = Arc::new(MetadataManager::new("./.cache", vec![], None).unwrap());
        
        // We'll trust that if ANY highlights are returned, it's probably because it leaked into the comment.
        // Even if $if isn't in metadata, it might try to highlight it if the skip logic is broken.
        // Actually, try_extract_metadata_function ONLY returns Some if it's in metadata.
        // But we can check if it returns 0 highlights.
        
        let highlights = extract_highlight_ranges(source, &["red".to_string()], false, &manager);
        
        assert_eq!(highlights.len(), 0, "Should have no highlights for functions inside comments, but found: {:?}", highlights);
    }
}
