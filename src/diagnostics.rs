use crate::server::ForgeScriptServer;
use tower_lsp::lsp_types::*;

/// Publish diagnostics using already-parsed results
/// Publish diagnostics using already-parsed results
#[tracing::instrument(skip(server, text, diagnostics_data), fields(uri = %uri, diag_count = diagnostics_data.len()))]
pub async fn analyze_and_publish(
    server: &ForgeScriptServer,
    uri: Url,
    text: &str,
    diagnostics_data: Vec<crate::parser::Diagnostic>,
) {
    let start = std::time::Instant::now();
    tracing::debug!("📊 Publishing {} diagnostics for {}", diagnostics_data.len(), uri);
    
    let convert_start = std::time::Instant::now();
    let diagnostics: Vec<Diagnostic> = diagnostics_data
        .into_iter()
        .map(|d| {
            let start_line = text[..d.start].matches('\n').count() as u32;
            let start_char = (d.start - text[..d.start].rfind('\n').unwrap_or(0)) as u32;
            let end_line = text[..d.end].matches('\n').count() as u32;
            let end_char = (d.end - text[..d.end].rfind('\n').unwrap_or(0)) as u32;

            Diagnostic {
                range: Range {
                    start: Position {
                        line: start_line,
                        character: start_char,
                    },
                    end: Position {
                        line: end_line,
                        character: end_char,
                    },
                },
                severity: Some(DiagnosticSeverity::WARNING),
                message: d.message,
                ..Default::default()
            }
        })
        .collect();
    tracing::trace!("⏱️  Diagnostic conversion took {:?}", convert_start.elapsed());

    let publish_start = std::time::Instant::now();
    let _ = server
        .client
        .publish_diagnostics(uri.clone(), diagnostics, None)
        .await;
    tracing::debug!("⏱️  Diagnostic publishing took {:?}", publish_start.elapsed());
    
    tracing::info!("✅ Diagnostics published in {:?} total", start.elapsed());
}
