#[cfg(not(target_arch = "wasm32"))]
use crate::server::ForgeScriptServer;
#[cfg(not(target_arch = "wasm32"))]
use crate::utils::{LogLevel, forge_log};
use crate::utils::{
    compute_active_param_index, find_active_function_call, get_text_up_to_cursor,
    position_to_offset,
};
use lsp_types::*;
use std::sync::Arc;
#[cfg(not(target_arch = "wasm32"))]
use tower_lsp::jsonrpc::Result;

#[cfg(not(target_arch = "wasm32"))]
pub async fn handle_completion(
    server: &ForgeScriptServer,
    params: CompletionParams,
) -> Result<Option<CompletionResponse>> {
    let start = crate::utils::Instant::now();
    let uri = params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;

    forge_log(
        LogLevel::Debug,
        &format!("Completion request for {} at {:?}", uri, position),
    );

    let text = server
        .documents
        .read()
        .expect("Server: lock poisoned")
        .get(&uri)
        .cloned()
        .ok_or(tower_lsp::jsonrpc::Error::invalid_params(
            "Document not found",
        ))?;
    let mgr = server
        .manager
        .read()
        .expect("Server: lock poisoned")
        .clone();

    let res = get_completions(&text, position, &mgr);

    forge_log(
        LogLevel::Debug,
        &format!("Completion response built in {}", start.elapsed_display()),
    );
    Ok(res)
}

pub fn get_completions(
    text: &str,
    position: Position,
    mgr: &crate::metadata::MetadataManager,
) -> Option<CompletionResponse> {
    let text_up_to_cursor = get_text_up_to_cursor(text, position);
    if let Some((func_name, open_idx)) = find_active_function_call(&text_up_to_cursor) {
        let param_idx = compute_active_param_index(&text_up_to_cursor[open_idx + 1..]) as usize;
        if let Some(func) = mgr.get(&format!("${func_name}"))
            && let Some(args) = &func.args
        {
            let arg_idx = if param_idx >= args.len() && args.last().map(|a| a.rest).unwrap_or(false)
            {
                args.len() - 1
            } else {
                param_idx
            };
            if let Some(arg) = args.get(arg_idx) {
                let enum_vals = if let Some(en) = &arg.enum_name {
                    mgr.enums
                        .read()
                        .expect("Server: lock poisoned")
                        .get(en)
                        .cloned()
                } else {
                    arg.arg_enum.clone()
                };
                if let Some(vals) = enum_vals {
                    let items = vals
                        .into_iter()
                        .map(|v: String| CompletionItem {
                            label: v.clone(),
                            kind: Some(CompletionItemKind::ENUM_MEMBER),
                            detail: Some(format!("Enum for {}", arg.name)),
                            insert_text: Some(v),
                            ..Default::default()
                        })
                        .collect();
                    return Some(CompletionResponse::List(CompletionList {
                        is_incomplete: false,
                        items,
                    }));
                }
            }
        }
    }

    let line = text.lines().nth(position.line as usize).unwrap_or("");
    let offset =
        position_to_offset(line, Position::new(0, position.character)).unwrap_or(line.len());
    let before = &line[..offset];

    let Some(dollar_idx) = before.rfind('$') else {
        return None;
    };
    let after_dollar = &before[dollar_idx + 1..];
    let modifier = if after_dollar.starts_with('!') {
        "!"
    } else if after_dollar.starts_with('.') {
        "."
    } else {
        ""
    };

    let mut start_char = 0;
    for c in line[..dollar_idx].chars() {
        start_char += c.len_utf16() as u32;
    }
    let range = Range::new(Position::new(position.line, start_char), position);

    let items = mgr
        .all_functions()
        .into_iter()
        .map(|f| build_completion_item(f, modifier, range))
        .collect::<Vec<_>>();

    Some(CompletionResponse::List(CompletionList {
        is_incomplete: false,
        items,
    }))
}

pub(crate) fn build_completion_item(
    f: Arc<crate::metadata::Function>,
    modifier: &str,
    range: Range,
) -> CompletionItem {
    let base = f.name.clone();
    let name = if !modifier.is_empty() && base.starts_with('$') {
        format!("${modifier}{}", &base[1..])
    } else {
        base.clone()
    };

    CompletionItem {
        label: name.clone(),
        kind: Some(CompletionItemKind::FUNCTION),
        detail: Some(
            f.extension
                .clone()
                .unwrap_or_else(|| f.category.clone().unwrap_or_else(|| "Function".to_string())),
        ),
        documentation: Some(Documentation::MarkupContent(MarkupContent {
            kind: MarkupKind::Markdown,
            value: build_completion_markdown(&f),
        })),
        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: name,
        })),
        filter_text: Some(base),
        ..Default::default()
    }
}

pub(crate) fn build_completion_markdown(f: &Arc<crate::metadata::Function>) -> String {
    let mut md = format!("```forgescript\n{}\n```\n\n", f.signature_label());
    if !f.description.is_empty() {
        md.push_str(&f.description);
        md.push_str("\n\n");
    }

    if let Some(examples) = &f.examples {
        if !examples.is_empty() {
            md.push_str("**Examples:**\n");
            for ex in examples.iter().take(2) {
                md.push_str(&format!("\n```forgescript\n{ex}\n```\n"));
            }
        }
    }

    let mut links = Vec::new();
    if let Some(url) = &f.source_url
        && url.contains("githubusercontent.com")
    {
        let parts: Vec<&str> = url.split('/').collect();
        if parts.len() >= 5 {
            links.push(format!(
                "[GitHub](https://github.com/{}/{})",
                parts[3], parts[4]
            ));
        }
    }

    if let Some(extension) = &f.extension {
        links.push(format!(
            "[Documentation](https://docs.botforge.org/function/{}?p={})",
            f.name, extension
        ));
    }

    if !links.is_empty() {
        md.push_str("\n---\n");
        md.push_str(&links.join(" | "));
    }
    md
}
