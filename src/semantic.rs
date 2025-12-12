use regex::Regex;
use tower_lsp::lsp_types::{Position, SemanticToken};

/// Extract tokens with optional multi-color function highlighting
/// When `use_function_colors` is true, functions alternate between FUNCTION (0) and PARAMETER (3)
/// token types sequentially as they appear in the code.
pub fn extract_semantic_tokens_with_colors(
    source: &str,
    use_function_colors: bool,
) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let code_block_re = Regex::new(r"code:\s*`{1,3}((?:\\`|[\s\S])*?)`{1,3}").unwrap();
    let func_re = Regex::new(
        r"\$(?:[a-zA-Z_][a-zA-Z0-9_]*|![a-zA-Z_][a-zA-Z0-9_]*|#[a-zA-Z_][a-zA-Z0-9_]*|@\[[^\]]*\])",
    )
    .unwrap();

    let number_re = Regex::new(r"\b\d+(?:\.\d+)?\b").unwrap();
    let bool_re = Regex::new(r"\b(?:true|false)\b").unwrap();
    let semicolon_re = Regex::new(r";").unwrap();

    let mut function_color_index = 0u32;

    for block in code_block_re.captures_iter(source) {
        if let Some(code_match) = block.get(1) {
            let code = code_match.as_str();
            let code_start = code_match.start();

            let mut found = Vec::new();

            // Collect function matches
            let func_matches: Vec<_> = func_re
                .find_iter(code)
                .map(|m| (m.start() + code_start, m.end() + code_start))
                .collect();

            // Add function tokens with alternating color assignment
            for (start, end) in func_matches {
                let token_type = if use_function_colors {
                    let colors = [0, 3]; // FUNCTION, PARAMETER
                    let color = colors[(function_color_index as usize) % colors.len()];
                    function_color_index += 1;
                    color
                } else {
                    0 // Default FUNCTION type
                };
                found.push((start, end, token_type));
            }

            // Add other token types
            for (regex, token_type) in [
                (&bool_re, 1),      // KEYWORD
                (&number_re, 2),    // NUMBER
                (&semicolon_re, 1), // KEYWORD (reuse)
            ] {
                for m in regex.find_iter(code) {
                    found.push((m.start() + code_start, m.end() + code_start, token_type));
                }
            }

            found.sort_by_key(|(s, _, _)| *s);

            let mut last_line = 0u32;
            let mut last_col = 0u32;

            for (start, end, token_type) in found {
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
