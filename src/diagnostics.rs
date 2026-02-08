//! Logic for transforming and publishing document diagnostics.

use crate::parser::Diagnostic as ParseDiagnostic;
use crate::server::ForgeScriptServer;
use crate::utils::offset_to_position;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;

/// Maps internal parser errors to LSP-compliant diagnostics and publishes them to the client.
///
/// This function converts byte offsets within the source text into line and character 
/// positions required by the LSP specification.
pub async fn publish_diagnostics(
    server: &ForgeScriptServer,
    uri: &Url,
    text: &str,
    diagnostics_data: &[ParseDiagnostic],
) {
    // Map each parser-generated diagnostic to an LSP Diagnostic object.
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

    // Send the diagnostic set to the client for the specified document URI.
    server
        .client
        .publish_diagnostics(uri.clone(), diagnostics, None)
        .await;
}
