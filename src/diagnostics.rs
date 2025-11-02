use crate::server::ForgeScriptServer;
use tower_lsp::lsp_types::*;

/// Publish diagnostics using already-parsed results
pub async fn analyze_and_publish(
    server: &ForgeScriptServer,
    uri: Url,
    text: &str,
    diagnostics_data: Vec<crate::parser::Diagnostic>,
) {
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

    let _ = server
        .client
        .publish_diagnostics(uri, diagnostics, None)
        .await;
}
