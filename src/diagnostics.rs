use crate::parser::Diagnostic as ParseDiagnostic;
use crate::server::ForgeScriptServer;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range, Url};

fn offset_to_position(text: &str, offset: usize) -> Position {
    let line = text[..offset].matches('\n').count() as u32;
    let char_in_line = offset - text[..offset].rfind('\n').unwrap_or(0);
    Position {
        line,
        character: char_in_line as u32,
    }
}

pub async fn publish_diagnostics(
    server: &ForgeScriptServer,
    uri: &Url,
    text: &str,
    diagnostics_data: &[ParseDiagnostic],
) {
    let diagnostics: Vec<Diagnostic> = diagnostics_data
        .iter()
        .map(|d| {
            let start_pos = offset_to_position(text, d.start);
            let end_pos = offset_to_position(text, d.end);

            Diagnostic {
                range: Range {
                    start: start_pos,
                    end: end_pos,
                },
                severity: Some(DiagnosticSeverity::ERROR),
                message: d.message.clone(),
                ..Default::default()
            }
        })
        .collect();

    server
        .client
        .publish_diagnostics(uri.clone(), diagnostics, None)
        .await;
}
