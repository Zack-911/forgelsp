//! # Hover Provider Module
//!
//! Implements LSP hover functionality for ForgeScript functions.
//! Provides rich markdown tooltips with function signatures, descriptions, and examples
//! when users hover over function names in their code.

use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use crate::server::ForgeScriptServer;
use crate::utils::spawn_log;

/// Check if a character at the given byte index is escaped by a backslash.
///
/// Counts consecutive backslashes before the character:
/// - Odd number of backslashes → character is escaped
/// - Even number of backslashes → character is NOT escaped
fn is_escaped(text: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 {
        return false;
    }

    let bytes = text.as_bytes();
    let mut backslash_count = 0;
    let mut pos = byte_idx;

    while pos > 0 {
        pos -= 1;
        if bytes[pos] == b'\\' {
            backslash_count += 1;
        } else {
            break;
        }
    }

    // Odd number of backslashes means the character is escaped
    backslash_count % 2 == 1
}

/// Handles hover requests for ForgeScript
pub async fn handle_hover(
    server: &ForgeScriptServer,
    params: HoverParams,
) -> Result<Option<Hover>> {
    let start = std::time::Instant::now();

    let uri = params
        .text_document_position_params
        .text_document
        .uri
        .clone();
    let position = params.text_document_position_params.position;

    // Fetch document text safely
    let text: String = {
        let docs = server.documents.read().unwrap();
        match docs.get(&uri) {
            Some(t) => t.clone(),
            None => {
                spawn_log(
                    server.client.clone(),
                    MessageType::WARNING,
                    "[WARN] No document found in cache for hover".to_string(),
                );
                return Ok(None);
            }
        }
    };

    // Calculate byte offset
    let mut offset = 0usize;
    for (line_idx, line) in text.split_inclusive('\n').enumerate() {
        if line_idx as u32 == position.line {
            offset += position.character as usize;
            break;
        } else {
            offset += line.len();
        }
    }

    if offset >= text.len() {
        return Ok(None);
    }

    // Include modifier characters in the initial token capture
    let is_ident_char = |c: char| {
        c.is_alphanumeric()
            || c == '_'
            || c == '.'
            || c == '$'
            || c == '!'
            || c == '#'
            || c == '@'
            || c == '['
            || c == ']'
    };
    let bytes = text.as_bytes();

    // Find start of token, skipping escaped $ characters
    let mut start_pos = offset;
    while start_pos > 0 && is_ident_char(bytes[start_pos - 1] as char) {
        // If we hit a $, check if it's escaped
        if bytes[start_pos - 1] == b'$' && is_escaped(&text, start_pos - 1) {
            break;
        }
        start_pos -= 1;
    }

    let mut end = offset;
    while end < bytes.len() && is_ident_char(bytes[end] as char) {
        end += 1;
    }

    if start_pos >= end {
        return Ok(None);
    }

    let raw_token = text[start_pos..end].to_string();

    // Don't provide hover for escape functions or JavaScript expressions
    if raw_token == "$esc" || raw_token == "$escape" {
        return Ok(None);
    }

    // Check if this is a JavaScript expression ${...}
    if raw_token.starts_with("${") {
        return Ok(None);
    }

    // Process modifiers to find the actual function name
    // Modifiers can be: ! (silent), # (negated), @[...] (scope)
    // Example: $!#@[user]ban
    let mut clean_token = raw_token.clone();

    if clean_token.starts_with('$') {
        let mut chars = clean_token.chars().peekable();
        chars.next(); // consume $

        let mut modifier_end_idx = 1; // start after $

        while let Some(&c) = chars.peek() {
            if c == '!' || c == '#' {
                modifier_end_idx += 1;
                chars.next();
            } else if c == '@' {
                // Handle @[...]
                modifier_end_idx += 1;
                chars.next(); // consume @

                if let Some(&'[') = chars.peek() {
                    modifier_end_idx += 1;
                    chars.next(); // consume [

                    // Find matching ]
                    let mut depth = 1;
                    while let Some(inner_c) = chars.next() {
                        modifier_end_idx += inner_c.len_utf8();
                        if inner_c == '[' {
                            depth += 1;
                        } else if inner_c == ']' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                    }
                }
            } else {
                break;
            }
        }

        // Reconstruct the token with just $ + function name
        if modifier_end_idx > 1 {
            // Check if we have a valid function name after modifiers
            let after_modifiers = &clean_token[modifier_end_idx..];
            if !after_modifiers.is_empty() {
                clean_token = format!("${}", after_modifiers);
            }
        }
    }

    // Acquire a read lock on the manager
    let mgr = server.manager.read().unwrap();
    let mgr_inner = mgr.clone();

    // Try to find the function using the cleaned token
    if let Some(func_ref) = mgr_inner.get(&clean_token) {
        let func_name = &func_ref.name;
        let func_description = &func_ref.description;
        let func_args = &func_ref.args;
        let func_output = &func_ref.output;
        let func_examples = &func_ref.examples;
        let func_brackets = &func_ref.brackets;

        let mut md = String::new();
        let args_str = func_args
            .as_ref()
            .map(|v| {
                v.iter()
                    .map(|a| {
                        let mut name = String::new();
                        if a.rest {
                            name.push_str("...");
                        }
                        name.push_str(&a.name);
                        if a.required == Some(false) {
                            name.push('?');
                        }
                        name
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default();

        let outputs_str = func_output
            .as_ref()
            .map(|v| v.join(";"))
            .unwrap_or_else(|| "void".to_string());

        md.push_str("```forgescript\n");
        if func_brackets == &Some(true) {
            md.push_str(&format!("{}[{}] -> {}\n", func_name, args_str, outputs_str));
        } else if func_brackets == &Some(false) {
            md.push_str(&format!("{}[{}] -> {}\n", func_name, args_str, outputs_str));
            md.push_str("Note: brackets are optional.\n");
        } else {
            md.push_str(&format!("{} -> {}\n", func_name, outputs_str));
        }
        md.push_str("```\n");

        if !func_description.is_empty() {
            md.push_str(func_description);
            md.push('\n');
        }

        if let Some(exs) = func_examples
            && !exs.is_empty()
        {
            md.push_str("\n**Examples:**\n");
            for ex in exs.iter().take(2) {
                md.push_str("\n```forgescript\n");
                md.push_str(ex);
                md.push_str("\n```\n");
            }
        }

        spawn_log(
            server.client.clone(),
            MessageType::LOG,
            format!("[PERF] hover: {} in {:?}", func_name, start.elapsed()),
        );

        return Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            range: None,
        }));
    }

    Ok(None)
}
