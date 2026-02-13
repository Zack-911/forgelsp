use crate::server::ForgeScriptServer;
use crate::utils::{compute_active_param_index, find_active_function_call, get_text_up_to_cursor};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

pub async fn handle_signature_help(
    server: &ForgeScriptServer,
    params: SignatureHelpParams,
) -> Result<Option<SignatureHelp>> {
    let uri = params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let text = server
        .documents
        .read()
        .expect("Server: lock poisoned")
        .get(&uri)
        .cloned()
        .ok_or(tower_lsp::jsonrpc::Error::invalid_params(
            "Document not found",
        ))?;

    let text_up_to_cursor = get_text_up_to_cursor(&text, pos);
    let Some((func_name, open_idx)) = find_active_function_call(&text_up_to_cursor) else {
        return Ok(None);
    };
    let param_idx = compute_active_param_index(&text_up_to_cursor[open_idx + 1..]);

    let mgr = server
        .manager
        .read()
        .expect("Server: lock poisoned")
        .clone();
    if let Some(func) = mgr.get(&format!("${func_name}")) {
        let sig = SignatureInformation {
            label: func.signature_label(),
            documentation: Some(Documentation::String(func.description.clone())),
            parameters: Some(build_signature_help_parameters(&func)),
            active_parameter: Some(param_idx),
        };
        return Ok(Some(SignatureHelp {
            signatures: vec![sig],
            active_signature: Some(0),
            active_parameter: Some(param_idx),
        }));
    }
    Ok(None)
}

pub(crate) fn build_signature_help_parameters(
    func: &crate::metadata::Function,
) -> Vec<ParameterInformation> {
    func.args
        .clone()
        .unwrap_or_default()
        .iter()
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

            ParameterInformation {
                label: ParameterLabel::Simple(name),
                documentation: Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: a.description.clone(),
                })),
            }
        })
        .collect()
}
