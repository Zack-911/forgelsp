use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use crate::server::ForgeScriptServer;
use crate::utils::spawn_log;

/// Handles hover requests
pub async fn handle_hover(
    server: &ForgeScriptServer,
    params: HoverParams,
) -> Result<Option<Hover>> {
    spawn_log(
        server.client.clone(),
        MessageType::INFO,
        format!(
            "⚡ Hover request received at position {:?}",
            params.text_document_position_params.position
        ),
    );

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
                    MessageType::INFO,
                    "No document found in cache.".to_string(),
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

    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == '$';
    let bytes = text.as_bytes();

    let mut start = offset;
    while start > 0 && is_ident_char(bytes[start - 1] as char) {
        start -= 1;
    }

    let mut end = offset;
    while end < bytes.len() && is_ident_char(bytes[end] as char) {
        end += 1;
    }

    if start >= end {
        return Ok(None);
    }

    let token = text[start..end].to_string();

    // Acquire a read lock on the manager
    let mgr = server.manager.read().unwrap(); // RwLockReadGuard<Arc<MetadataManager>>
    let mgr_inner = mgr.clone(); // Arc<MetadataManager>

    if let Some(func_ref) = mgr_inner.get(&token) {
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
                    .join(", ")
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

        if let Some(exs) = func_examples {
            if !exs.is_empty() {
                md.push_str("\n**Examples:**\n");
                for ex in exs.iter().take(2) {
                    md.push_str("\n```forgescript\n");
                    md.push_str(ex);
                    md.push_str("\n```\n");
                }
            }
        }

        return Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            range: None,
        }));
    }

    return Ok(None);
}
