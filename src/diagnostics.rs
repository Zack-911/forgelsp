//! # Diagnostics Module
//!
//! Converts parser diagnostics to LSP format and publishes them to the client.
//! Handles byte offset to line/character position conversion for LSP compatibility.

use crate::parser::Diagnostic as ParseDiagnostic;
use crate::server::ForgeScriptServer;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range, Url};

/// Converts a byte offset in the source text to an LSP Position (line, character).
///
/// # Arguments
/// * `text` - The source code
/// * `offset` - Byte offset in the source
///
/// # Returns
/// LSP Position with 0-indexed line and character numbers
fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0;
    let mut col = 0;

    for (i, c) in text.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += c.len_utf16() as u32;
        }
    }

    Position {
        line,
        character: col,
    }
}

/// Publishes diagnostics to the LSP client for a given document.
///
/// Converts internal parser diagnostics (with byte offsets) to LSP diagnostics
/// (with line/character positions) and sends them to the client.
///
/// # Arguments
/// * `server` - The ForgeScriptServer instance
/// * `uri` - The document URI to publish diagnostics for
/// * `text` - The full document text (needed for position conversion)
/// * `diagnostics_data` - Array of parser diagnostics with byte offsets
pub async fn publish_diagnostics(
    server: &ForgeScriptServer,
    uri: &Url,
    text: &str,
    diagnostics_data: &[ParseDiagnostic],
) {
    // Convert each parser diagnostic to LSP format
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
                // Currently all diagnostics are errors
                // Future: could use WARNING for deprecated functions, INFO for suggestions
                severity: Some(DiagnosticSeverity::ERROR),
                message: d.message.clone(),
                ..Default::default()
            }
        })
        .collect();

    // Publish diagnostics to the LSP client
    server
        .client
        .publish_diagnostics(uri.clone(), diagnostics, None)
        .await;
}
