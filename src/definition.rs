use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use crate::server::ForgeScriptServer;
use crate::utils::{position_to_offset, skip_modifiers};

pub async fn handle_definition(
    server: &ForgeScriptServer,
    params: GotoDefinitionParams,
) -> Result<Option<GotoDefinitionResponse>> {
    let uri = params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let text = server
        .documents
        .read()
        .expect("Server: lock poisoned")
        .get(&uri)
        .cloned()
        .ok_or(tower_lsp::jsonrpc::Error::invalid_params(
            "Document not found",
        ))?;
    let offset = position_to_offset(&text, position).ok_or(
        tower_lsp::jsonrpc::Error::invalid_params("Invalid position"),
    )?;

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
    let curr = indices
        .iter()
        .position(|&(p, _)| p >= offset)
        .unwrap_or(indices.len());

    let mut start = curr;
    while start > 0 && is_ident_char(indices[start - 1].1) {
        start -= 1;
        if indices[start].1 == '$' && !crate::utils::is_escaped(&text, indices[start].0) {
            break;
        }
    }

    let mut end = curr;
    while end < indices.len() && is_ident_char(indices[end].1) {
        if indices[end].1 == '$' && !crate::utils::is_escaped(&text, indices[end].0) && end > start
        {
            break;
        }
        end += 1;
    }

    if start >= end {
        return Ok(None);
    }
    let mut token = text[indices[start].0..if end < indices.len() {
        indices[end].0
    } else {
        text.len()
    }]
        .to_string();

    if token.starts_with('$') {
        let mod_end = skip_modifiers(&token, 1);
        if mod_end > 1 && !&token[mod_end..].is_empty() {
            token = format!("${}", &token[mod_end..]);
        }
    }

    if let Some(func) = server
        .manager
        .read()
        .expect("Server: lock poisoned")
        .get(&token)
        && let (Some(path), Some(line)) = (&func.local_path, func.line)
    {
        let target_uri =
            Url::from_file_path(path).map_err(|_| tower_lsp::jsonrpc::Error::internal_error())?;
        return Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: target_uri,
            range: Range::new(Position::new(line, 0), Position::new(line, 0)),
        })));
    }
    Ok(None)
}
