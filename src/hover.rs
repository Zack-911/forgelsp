//! Implementation of the LSP Hover provider for ForgeScript.
//!
//! Provides context-aware tooltips for functions, including signatures,
//! descriptions, and documentation links.

use tower_lsp::jsonrpc::Result;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;

use crate::server::ForgeScriptServer;
use crate::utils::{is_escaped, position_to_offset, skip_modifiers};

/// Processes a hover request by identifying the symbol under the cursor.
pub async fn handle_hover(
    server: &ForgeScriptServer,
    params: HoverParams,
) -> Result<Option<Hover>> {
    let start = crate::utils::Instant::now();
    let uri = params
        .text_document_position_params
        .text_document
        .uri
        .clone();
    crate::utils::forge_log(
        crate::utils::LogLevel::Debug,
        &format!("Hover request for {}", uri),
    );
    let uri = params
        .text_document_position_params
        .text_document
        .uri
        .clone();
    let position = params.text_document_position_params.position;

    // Retrieve the document content from the server's cache.
    let text: String = {
        let docs = server
            .documents
            .read()
            .expect("Hover: documents lock poisoned");
        match docs.get(&uri) {
            Some(t) => t.clone(),
            _ => {
                return Ok(None);
            }
        }
    };

    // Convert the LSP UTF-16 cursor position to a byte offset.
    let offset = match position_to_offset(&text, position) {
        Some(o) => o,
        _ => return Ok(None),
    };

    // Defines characters allowed in ForgeScript function identifiers and modifiers.
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

    let indices: Vec<(usize, char)> = text.char_indices().collect();

    // Map the byte offset to an index in our character vector.
    let mut current_char_idx = indices.len();
    for (idx, (byte_pos, _)) in indices.iter().enumerate() {
        if *byte_pos >= offset {
            current_char_idx = idx;
            break;
        }
    }

    // Expand search backwards to find the start of the function call (leading '$').
    let mut start_char_idx = current_char_idx;
    while start_char_idx > 0 {
        let (byte_pos, c) = indices[start_char_idx - 1];
        if is_ident_char(c) {
            if c == '$' && !is_escaped(&text, byte_pos) {
                start_char_idx -= 1;
                break;
            }
            start_char_idx -= 1;
        } else {
            break;
        }
    }

    // Expand search forwards to find the end of the identifier or until the next function call.
    let mut end_char_idx = current_char_idx;
    while end_char_idx < indices.len() {
        let (byte_pos, c) = indices[end_char_idx];
        if is_ident_char(c) {
            if c == '$' && !is_escaped(&text, byte_pos) && end_char_idx > start_char_idx {
                break;
            }
            end_char_idx += 1;
        } else {
            break;
        }
    }

    if start_char_idx >= end_char_idx {
        return Ok(None);
    }

    let start_byte = indices[start_char_idx].0;
    let end_byte = if end_char_idx < indices.len() {
        indices[end_char_idx].0
    } else {
        text.len()
    };

    let raw_token = text[start_byte..end_byte].to_string();

    // Ignore structural symbols and internal expressions.
    if raw_token == "$esc" || raw_token == "$escape" || raw_token.starts_with("${") {
        return Ok(None);
    }

    // Strip modifiers (!, #, @[...]) to extract the base function name for metadata lookup.
    let mut clean_token = raw_token.clone();
    if clean_token.starts_with('$') {
        let modifier_end_idx = skip_modifiers(&clean_token, 1);
        if modifier_end_idx > 1 {
            let after_modifiers = &clean_token[modifier_end_idx..];
            if !after_modifiers.is_empty() {
                clean_token = format!("${after_modifiers}");
            }
        }
    }

    // Lookup metadata for the identified function.
    let mgr = server.manager.read().expect("Hover: manager lock poisoned");
    let mgr_inner = mgr.clone();

    if let Some(func_ref) = mgr_inner.get(&clean_token) {
        let func_name = &func_ref.name;
        let func_description = &func_ref.description;
        let func_args = &func_ref.args;
        let func_output = &func_ref.output;
        let func_examples = &func_ref.examples;
        let func_brackets = &func_ref.brackets;

        let mut md = String::new();

        // Build the signature representation.
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
                        if a.required != Some(true) || a.rest {
                            name.push('?');
                        }

                        let type_str = match &a.arg_type {
                            serde_json::Value::String(s) => s.clone(),
                            serde_json::Value::Array(arr) => arr
                                .iter()
                                .map(|v| v.as_str().unwrap_or("?").to_string())
                                .collect::<Vec<_>>()
                                .join("|"),
                            _ => "Any".to_string(),
                        };

                        if !type_str.is_empty() {
                            name.push_str(": ");
                            name.push_str(&type_str);
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
        if func_brackets.is_some() {
            md.push_str(&format!("{func_name}[{args_str}] -> {outputs_str}\n"));
        } else {
            md.push_str(&format!("{func_name} -> {outputs_str}\n"));
        }
        md.push_str("```\n");

        if !func_description.is_empty() {
            md.push_str(func_description);
            md.push('\n');
        }

        // Include usage examples if provided in metadata.
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

        // Append external documentation and source links.
        let mut links = Vec::new();
        if let Some(url) = &func_ref.source_url {
            if url.contains("githubusercontent.com") {
                let parts: Vec<&str> = url.split('/').collect();
                if parts.len() >= 5 {
                    let owner = parts[3];
                    let repo = parts[4];
                    links.push(format!("[GitHub Repo](https://github.com/{owner}/{repo})"));
                }
            }
        }

        if let Some(extension) = &func_ref.extension {
            let base_url = "https://docs.botforge.org";
            links.push(format!(
                "[Documentation]({base_url}/function/{func_name}?p={extension})",
                base_url = base_url,
                func_name = func_name,
                extension = extension
            ));
        }

        if !links.is_empty() {
            md.push_str("\n---\n");
            md.push_str(&links.join(" | "));
            md.push('\n');
        }

        crate::utils::forge_log(
            crate::utils::LogLevel::Debug,
            &format!("Hover resolution took {}", start.elapsed_display()),
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
