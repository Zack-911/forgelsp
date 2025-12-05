use regex::Regex;
use tower_lsp::lsp_types::{Position, SemanticToken};

/// Extract tokens using regex-based rules inside code blocks
pub fn extract_semantic_tokens(source: &str) -> Vec<SemanticToken> {
    let mut tokens = Vec::new();
    let code_block_re = Regex::new(r"code:\s*`{1,3}((?:\\`|[\s\S])*?)`{1,3}").unwrap();
    let func_re = Regex::new(
        r"\$(?:[a-zA-Z_][a-zA-Z0-9_]*|![a-zA-Z_][a-zA-Z0-9_]*|#[a-zA-Z_][a-zA-Z0-9_]*|@\[[^\]]*\])",
    )
    .unwrap();
    let string_re = Regex::new(r#""([^"\\]|\\.)*"|'([^'\\]|\\.)*'"#).unwrap();
    let number_re = Regex::new(r"\b\d+(?:\.\d+)?\b").unwrap();
    let bool_re = Regex::new(r"\b(?:true|false)\b").unwrap();
    let semicolon_re = Regex::new(r";").unwrap();

    for block in code_block_re.captures_iter(source) {
        if let Some(code_match) = block.get(1) {
            let code = code_match.as_str();
            let code_start = code_match.start();

            let mut found = Vec::new();

            for (regex, token_type) in [
                (&func_re, 0),      // FUNCTION
                (&string_re, 1),    // STRING
                (&bool_re, 2),      // KEYWORD
                (&number_re, 3),    // NUMBER
                (&semicolon_re, 2), // KEYWORD (reuse)
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
