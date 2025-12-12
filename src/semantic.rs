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
    for i in open_idx..code.len() {
        if code[i] == b'[' {
            depth += 1;
        } else if code[i] == b']' {
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
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];

        // Check for functions
        if c == b'$' && !is_char_escaped(bytes, i) {
            // Check for $c[...] (comment)
            if i + 1 < bytes.len() && bytes[i + 1] == b'c' {
                if i + 2 < bytes.len() && bytes[i + 2] == b'[' {
                    // Found comment function
                    if let Some(end_idx) = find_matching_bracket_raw(bytes, i + 2) {
                        // Highlight entire $c[...] as KEYWORD (comment)
                        found.push((i + code_start, end_idx + 1 + code_start, 5));
                        i = end_idx + 1;
                        continue;
                    }
                }
            }

            // Check for $esc[...] or $escapeCode[...] (escape functions)
            if let Some(esc_end) = check_escape_function(bytes, i) {
                // Highlight function name
                let name_end = i + if bytes[i + 1..].starts_with(b"esc[") {
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
                i = esc_end + 1;
                continue;
            }

            // Try incremental matching against metadata
            let mut best_match_len = 0;
            let mut j = i + 1;

            // Skip modifiers
            while j < bytes.len() && (bytes[j] == b'!' || bytes[j] == b'#') {
                j += 1;
            }

            // Try matching character by character
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                let candidate = &code[i..=j];
                if manager.get(candidate).is_some() {
                    best_match_len = j - i + 1;
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
                i += best_match_len;
                continue;
            }
        }

        // Check for numbers
        if c.is_ascii_digit() {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            found.push((start + code_start, i + code_start, 2));
            continue;
        }

        // Check for booleans
        if i + 4 <= bytes.len() && &code[i..i + 4] == "true" {
            found.push((i + code_start, i + 4 + code_start, 1));
            i += 4;
            continue;
        }
        if i + 5 <= bytes.len() && &code[i..i + 5] == "false" {
            found.push((i + code_start, i + 5 + code_start, 1));
            i += 5;
            continue;
        }

        // Check for semicolons
        if c == b';' && !is_char_escaped(bytes, i) {
            found.push((i + code_start, i + 1 + code_start, 1));
        }

        i += 1;
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
