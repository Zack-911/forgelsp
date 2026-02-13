//! Analyzes ForgeScript source code to extract semantic tokens for highlighting.
//!
//! Validates function calls against available metadata to distinguish valid
//! ForgeScript identifiers from text.

use crate::metadata::MetadataManager;
#[cfg(not(target_arch = "wasm32"))]
use crate::server::{CustomNotification, ForgeHighlightsParams, ForgeScriptServer, HighlightRange};
use crate::utils::{find_matching_bracket_raw, is_escaped, offset_to_position};
use lsp_types::*;
use regex::Regex;
use std::sync::{Arc, LazyLock};
#[cfg(not(target_arch = "wasm32"))]
use tower_lsp::jsonrpc::Result;

/// Identifies ForgeScript code blocks in host configuration files.
static CODE_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)code:\s*`((?:[^`\\]|\\.)*)`").expect("Semantic: regex failure")
});

/// Processes document content to produce an LSP-compatible set of semantic tokens.
pub fn extract_semantic_tokens_with_colors(
    source: &str,
    use_function_colors: bool,
    manager: &Arc<MetadataManager>,
) -> Vec<SemanticToken> {
    let start = crate::utils::Instant::now();
    let mut tokens = Vec::new();

    // Iterate through all "code:" blocks found in the source.
    for cap in CODE_BLOCK_RE.captures_iter(source) {
        if let Some(match_group) = cap.get(1) {
            let content_start = match_group.start();
            let code = match_group.as_str();

            let found_tokens =
                extract_tokens_from_code(code, content_start, use_function_colors, manager);
            tokens.extend(found_tokens);
        }
    }

    crate::utils::forge_log(
        crate::utils::LogLevel::Debug,
        &format!("Extracted semantic tokens in {}", start.elapsed_display()),
    );
    // Convert absolute token offsets to relative offsets for the LSP payload.
    to_relative_tokens(&tokens, source)
}

/// Identifies highlighting ranges for VS Code-specific decorations.
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
                        let raw_func = &code[i..i + best_match_len];
                        let base_name = if consistent_colors {
                            let name_start = try_find_name_start(raw_func);
                            format!("${}", &raw_func[name_start..])
                        } else {
                            raw_func.to_string()
                        };

                        let color = if consistent_colors {
                            function_to_color
                                .entry(base_name)
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

/// Tokenizes the content of a ForgeScript code block.
fn extract_tokens_from_code(
    code: &str,
    code_start: usize,
    use_function_colors: bool,
    manager: &Arc<MetadataManager>,
) -> Vec<(usize, usize, u32)> {
    let mut found = Vec::new();
    let mut function_color_index = 0u32;
    let char_positions: Vec<(usize, char)> = code.char_indices().collect();
    let mut idx = 0;

    while idx < char_positions.len() {
        let (i, c) = char_positions[idx];

        if c == '$' && !is_escaped(code, i) {
            if let Some((end_idx, next_idx)) = try_extract_comment(code, idx, &char_positions) {
                found.push((i + code_start, end_idx + 1 + code_start, 5));
                idx = next_idx;
                continue;
            }

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

            if let Some((best_match_len, best_match_char_count)) =
                try_extract_metadata_function(code, idx, &char_positions, manager)
            {
                let token_type = if use_function_colors {
                    let color = (function_color_index % 2) * 3; // Alternates between 0 and 3
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
        if let Some(end_idx) = find_matching_bracket_raw(code.as_bytes(), char_positions[idx + 2].0)
        {
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
    if let Some(esc_end) = crate::utils::find_escape_function_end(code, i) {
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

    // Skip ForgeScript modifiers
    while j < char_positions.len() {
        let (_, c) = char_positions[j];
        if c == '!' || c == '#' {
            j += 1;
        } else if c == '@' && j + 1 < char_positions.len() && char_positions[j + 1].1 == '[' {
            if let Some(close_idx) =
                find_matching_bracket_raw(code.as_bytes(), char_positions[j + 1].0)
            {
                while j < char_positions.len() && char_positions[j].0 <= close_idx {
                    j += 1;
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

    let name_start_byte = char_positions[j].0;
    let mut name_end_char = j;
    while name_end_char < char_positions.len()
        && (char_positions[name_end_char].1.is_alphanumeric()
            || char_positions[name_end_char].1 == '_')
    {
        name_end_char += 1;
    }

    let has_bracket =
        name_end_char < char_positions.len() && char_positions[name_end_char].1 == '[';
    let check_fn = |end_char_idx: usize| -> bool {
        let end_byte = if end_char_idx < char_positions.len() {
            char_positions[end_char_idx].0
        } else {
            code.len()
        };
        code.get(name_start_byte..end_byte)
            .map(|name| manager.get_exact(&format!("${name}")).is_some())
            .unwrap_or(false)
    };

    if has_bracket {
        if check_fn(name_end_char) {
            return Some((char_positions[name_end_char].0 - i, name_end_char - idx));
        }
    } else {
        let mut check_idx = name_end_char;
        while check_idx > j {
            if check_fn(check_idx) {
                let end_byte = if check_idx < char_positions.len() {
                    char_positions[check_idx].0
                } else {
                    code.len()
                };
                return Some((end_byte - i, check_idx - idx));
            }
            check_idx -= 1;
        }
    }
    None
}

/// Computes relative offsets for semantic tokens.
fn to_relative_tokens(found: &[(usize, usize, u32)], source: &str) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let mut last_line = 0u32;
    let mut last_col = 0u32;

    for &(start, end, token_type) in found {
        let start_pos = offset_to_position(source, start);
        let end_pos = offset_to_position(source, end);

        if start_pos.line == end_pos.line {
            let delta_line: u32 = start_pos.line.saturating_sub(last_line);
            let delta_start: u32 = if delta_line == 0 {
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
            let lines: Vec<&str> = source.lines().collect();
            for line_idx in (start_pos.line as u32)..=(end_pos.line as u32) {
                let delta_line: u32 = line_idx.saturating_sub(last_line);
                let line_text = lines.get(line_idx as usize).unwrap_or(&"");
                let (start_char, length) = if line_idx == start_pos.line {
                    (
                        start_pos.character,
                        (line_text.chars().count() as u32).saturating_sub(start_pos.character),
                    )
                } else if line_idx == end_pos.line {
                    (0, end_pos.character)
                } else {
                    (0, line_text.chars().count() as u32)
                };

                if length > 0 {
                    let delta_start: u32 = if delta_line == 0 {
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

fn try_find_name_start(raw_func: &str) -> usize {
    let chars: Vec<(usize, char)> = raw_func.char_indices().collect();
    if chars.is_empty() {
        return 0;
    }
    let mut j = 1;
    while j < chars.len() {
        let (_, c) = chars[j];
        if c == '!' || c == '#' {
            j += 1;
        } else if c == '@' && j + 1 < chars.len() && chars[j + 1].1 == '[' {
            if let Some(close_idx) = find_matching_bracket_raw(raw_func.as_bytes(), chars[j + 1].0)
            {
                while j < chars.len() && chars[j].0 <= close_idx {
                    j += 1;
                }
            } else {
                break;
            }
        } else {
            break;
        }
    }
    if j < chars.len() {
        chars[j].0
    } else {
        raw_func.len()
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn handle_semantic_tokens_full(
    server: &ForgeScriptServer,
    params: SemanticTokensParams,
) -> Result<Option<SemanticTokensResult>> {
    let text = server
        .documents
        .read()
        .expect("Server: lock poisoned")
        .get(&params.text_document.uri)
        .cloned()
        .ok_or(tower_lsp::jsonrpc::Error::invalid_params(
            "Document not found",
        ))?;
    let use_colors = *server
        .multiple_function_colors
        .read()
        .expect("Server: lock poisoned");
    let mgr = server
        .manager
        .read()
        .expect("Server: lock poisoned")
        .clone();
    let tokens = extract_semantic_tokens_with_colors(&text, use_colors, &mgr);
    Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: tokens,
    })))
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn handle_send_highlights(server: &ForgeScriptServer, uri: Url, text: &str) {
    let start = crate::utils::Instant::now();
    let highlights = {
        let colors = server
            .function_colors
            .read()
            .expect("Server: lock poisoned")
            .clone();
        if colors.is_empty() {
            return;
        }

        let mgr = server
            .manager
            .read()
            .expect("Server: lock poisoned")
            .clone();
        let consistent = *server
            .consistent_function_colors
            .read()
            .expect("Server: lock poisoned");

        extract_highlight_ranges(text, &colors, consistent, &mgr)
            .into_iter()
            .map(|(start, end, color)| HighlightRange {
                range: Range::new(
                    offset_to_position(text, start),
                    offset_to_position(text, end),
                ),
                color,
            })
            .collect::<Vec<HighlightRange>>()
    };

    server
        .client
        .send_notification::<CustomNotification>(ForgeHighlightsParams {
            uri: uri.clone(),
            highlights,
        })
        .await;
    crate::utils::forge_log(
        crate::utils::LogLevel::Debug,
        &format!("Highlights sent for {} in {}", uri, start.elapsed_display()),
    );
}
