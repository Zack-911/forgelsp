use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use crate::server::ForgeScriptServer;

pub async fn handle_folding_range(
    server: &ForgeScriptServer,
    params: FoldingRangeParams,
) -> Result<Option<Vec<FoldingRange>>> {
    let uri = params.text_document.uri;
    let parsed = server
        .parsed_cache
        .read()
        .expect("Server: lock poisoned")
        .get(&uri)
        .cloned()
        .ok_or(tower_lsp::jsonrpc::Error::invalid_params("Not parsed"))?;
    let text = server
        .documents
        .read()
        .expect("Server: lock poisoned")
        .get(&uri)
        .cloned()
        .ok_or(tower_lsp::jsonrpc::Error::invalid_params("No text"))?;

    let mut ranges = Vec::new();
    for func in &parsed.functions {
        let start = crate::utils::offset_to_position(&text, func.span.0);
        let end = crate::utils::offset_to_position(&text, func.span.1);
        if start.line < end.line {
            ranges.push(FoldingRange {
                start_line: start.line,
                start_character: Some(start.character),
                end_line: end.line,
                end_character: Some(end.character),
                kind: Some(FoldingRangeKind::Region),
                collapsed_text: Some(func.name.clone()),
            });
        }
    }
    Ok(Some(ranges))
}
