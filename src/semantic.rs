use crate::metadata::MetadataManager;
use crate::parser::{ForgeScriptParser, TokenKind};
use regex::Regex;
use std::sync::Arc;
use tower_lsp::lsp_types::{Position, SemanticToken};

/// Extract code blocks from source, handling escaped backticks
fn extract_code_blocks(source: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let code_block_prefix = "code:";

    let mut chars = source.char_indices().peekable();
    while let Some((i, _ch)) = chars.next() {
        // Look for "code:" prefix
        if source[i..].starts_with(code_block_prefix) {
            let code_start = i + code_block_prefix.len();

            // Skip whitespace after "code:"
            let mut pos = code_start;
            while pos < source.len() && source.as_bytes()[pos].is_ascii_whitespace() {
                pos += 1;
            }

            if pos >= source.len() {
                continue;
            }

            // Count opening backticks
            let mut backtick_count = 0;
            let backtick_start = pos;
            while pos < source.len() && source.as_bytes()[pos] == b'`' {
                backtick_count += 1;
                pos += 1;
            }

            if backtick_count == 0 {
                continue;
            }

            // Find matching closing backticks, respecting escapes
            let _content_start = pos;
            let mut content = String::new();
            let bytes = source.as_bytes();

            while pos < source.len() {
                // Check if escaped
                let is_escaped = if pos > 0 {
                    let mut backslash_count = 0;
                    let mut check_pos = pos;
                    while check_pos > 0 {
                        check_pos -= 1;
                        if bytes[check_pos] == b'\\' {
                            backslash_count += 1;
                        } else {
                            break;
                        }
                    }
                    backslash_count % 2 == 1
                } else {
                    false
                };

                if !is_escaped && bytes[pos] == b'`' {
                    // Count consecutive backticks
                    let mut closing_count = 0;
                    let mut check_pos = pos;
                    while check_pos < source.len() && bytes[check_pos] == b'`' {
                        closing_count += 1;
                        check_pos += 1;
                    }

                    if closing_count == backtick_count {
                        // Found matching closing backticks
                        blocks.push((backtick_start, content));

                        // Advance past closing backticks
                        while chars.peek().is_some() && chars.peek().unwrap().0 < check_pos {
                            chars.next();
                        }
                        break;
                    }
                }

                content.push(bytes[pos] as char);
                pos += 1;
            }
        }
    }

    blocks
}

/// Extract tokens using parser-based approach for code blocks
pub fn extract_semantic_tokens(source: &str, manager: Arc<MetadataManager>) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();

    // Extract code blocks with escape support
    let code_blocks = extract_code_blocks(source);

    // Regex for literals (strings, numbers, booleans, semicolons)
    let string_re = Regex::new(r#""([^"\\]|\\.)*"|'([^'\\]|\\.)*'"#).unwrap();
    let number_re = Regex::new(r"\b\d+(?:\.\d+)?\b").unwrap();
    let bool_re = Regex::new(r"\b(?:true|false)\b").unwrap();
    let semicolon_re = Regex::new(r";").unwrap();

    for (block_start, code) in code_blocks {
        // Parse the code using our parser
        let parser = ForgeScriptParser::new(manager.clone(), &code);
        let parse_result = parser.parse();

        let mut found = Vec::new();

        // Add parser tokens
        for token in &parse_result.tokens {
            let token_type = match token.kind {
                TokenKind::FunctionName => 0, // FUNCTION
                TokenKind::JavaScript => 4,   // VARIABLE (for JS expressions)
                TokenKind::Escaped => 1,      // STRING (for escaped content)
                TokenKind::Text => continue,  // Skip text tokens
                TokenKind::Unknown => 0,      // FUNCTION (highlight unknown as function)
            };

            found.push((
                token.start + block_start,
                token.end + block_start,
                token_type,
            ));
        }

        // Add literal tokens (strings, numbers, booleans, semicolons)
        for (regex, token_type) in [
            (&string_re, 1),    // STRING
            (&bool_re, 2),      // KEYWORD
            (&number_re, 3),    // NUMBER
            (&semicolon_re, 2), // KEYWORD
        ] {
            for m in regex.find_iter(&code) {
                found.push((m.start() + block_start, m.end() + block_start, token_type));
            }
        }

        // Sort by start position
        found.sort_by_key(|(s, _, _)| *s);

        // Remove overlapping tokens (prefer parser tokens over regex tokens)
        let mut non_overlapping = Vec::new();
        for (start, end, token_type) in found {
            let overlaps = non_overlapping
                .iter()
                .any(|(s, e, _): &(usize, usize, u32)| {
                    (start >= *s && start < *e)
                        || (end > *s && end <= *e)
                        || (start <= *s && end >= *e)
                });

            if !overlaps {
                non_overlapping.push((start, end, token_type));
            }
        }

        // Convert to LSP semantic tokens
        let mut last_line = 0u32;
        let mut last_col = 0u32;

        for (start, end, token_type) in non_overlapping {
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
