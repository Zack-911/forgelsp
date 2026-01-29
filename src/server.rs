//! # LSP Server Implementation
//!
//! Implements the `LanguageServer` trait from Tower LSP for `ForgeScript`.
//!
//! Provides:
//! - Document synchronization (full sync mode)
//! - Hover tooltips with function documentation
//! - Auto-completion with modifier support (`$!`, `$.`)
//! - Signature help with parameter tracking
//! - Semantic token highlighting
//! - Real-time diagnostics

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use crate::diagnostics::publish_diagnostics;
use crate::hover::handle_hover;
use crate::metadata::MetadataManager;
use crate::parser::{ForgeScriptParser, ParseResult};
use crate::semantic::extract_semantic_tokens_with_colors;
use crate::utils::{ForgeConfig, load_forge_config_full, position_to_offset, spawn_log};
use regex::Regex;
use tower_lsp::Client;
use tower_lsp::LanguageServer;
use tower_lsp::async_trait;
use tower_lsp::jsonrpc::Result;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;

/// Regex for identifying `ForgeScript` functions in signature help.
static SIGNATURE_FUNC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*$").expect("Server: regex compile failed")
});

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HighlightRange {
    pub range: Range,
    pub color: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ForgeHighlightsParams {
    pub uri: Url,
    pub highlights: Vec<HighlightRange>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ForgeDepthParams {
    pub uri: Url,
    pub depth: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CursorMovedParams {
    pub uri: Url,
    pub position: Position,
}

/// `ForgeScript` Language Server
///
/// Maintains shared state for document content, parse results, and function metadata.
/// All state is wrapped in Arc<`RwLock`<>> for thread-safe concurrent access.

#[derive(Debug)]
pub struct ForgeScriptServer {
    pub client: Client,
    pub manager: Arc<RwLock<Arc<MetadataManager>>>,
    pub documents: Arc<RwLock<HashMap<Url, String>>>,
    pub parsed_cache: Arc<RwLock<HashMap<Url, ParseResult>>>,
    pub workspace_folders: Arc<RwLock<Vec<PathBuf>>>,
    pub multiple_function_colors: Arc<RwLock<bool>>,
    pub consistent_function_colors: Arc<RwLock<bool>>,
    pub function_colors: Arc<RwLock<Vec<String>>>,
    pub config: Arc<RwLock<Option<ForgeConfig>>>,
    pub cursor_positions: Arc<RwLock<HashMap<Url, Position>>>,
}

impl ForgeScriptServer {
    /// Analyzes the text content of a document and publishes diagnostics.
    pub async fn process_text(&self, uri: Url, text: String) {
        let start = std::time::Instant::now();

        let mgr_arc = self
            .manager
            .read()
            .expect("Server: manager lock poisoned")
            .clone();
        let parser = ForgeScriptParser::new(mgr_arc, &text);
        let parsed = parser.parse();

        self.parsed_cache
            .write()
            .expect("Server: parsed_cache lock poisoned")
            .insert(uri.clone(), parsed.clone());

        publish_diagnostics(self, &uri, &text, &parsed.diagnostics).await;
        self.send_highlights(uri.clone(), &text).await;
        self.update_depth(uri).await;

        let diag_count = parsed.diagnostics.len();
        if diag_count > 0 {
            spawn_log(
                self.client.clone(),
                MessageType::WARNING,
                format!(
                    "[WARN] Found {diag_count} diagnostics in {elapsed:?}",
                    elapsed = start.elapsed()
                ),
            );
        }
    }

    /// Returns the total number of functions managed by the server.
    pub fn function_count(&self) -> usize {
        let mgr = self.manager.read().expect("Server: manager lock poisoned");
        mgr.function_count()
    }



    /// Sends dynamic highlights notification to the client.
    pub async fn send_highlights(&self, uri: Url, text: &str) {
        let highlights = {
            let colors = self
                .function_colors
                .read()
                .expect("Server: function_colors lock poisoned")
                .clone();

            if colors.is_empty() {
                return;
            }

            let mgr = self
                .manager
                .read()
                .expect("Server: manager lock poisoned")
                .clone();

            let consistent_colors = *self
                .consistent_function_colors
                .read()
                .expect("Server: consistent_function_colors lock poisoned");

            let ranges = crate::semantic::extract_highlight_ranges(text, &colors, consistent_colors, &mgr);
            ranges
                .into_iter()
                .map(|(start, end, color)| {
                    let start_pos = crate::utils::offset_to_position(text, start);
                    let end_pos = crate::utils::offset_to_position(text, end);
                    HighlightRange {
                        range: Range::new(start_pos, end_pos),
                        color,
                    }
                })
                .collect::<Vec<HighlightRange>>()
        };

        self.client
            .send_notification::<CustomNotification>(ForgeHighlightsParams {
                uri,
                highlights,
            })
            .await;
    }

    pub async fn update_depth(&self, uri: Url) {
        let depth = {
            let docs = self.documents.read().expect("Server: docs lock poisoned");
            let Some(text) = docs.get(&uri) else { return; };

            let cursor_positions = self.cursor_positions.read().expect("Server: cursors lock poisoned");
            let Some(&position) = cursor_positions.get(&uri) else { return; };

            let offset = position_to_offset(text, position).unwrap_or(0);
            crate::utils::calculate_depth(text, offset)
        };

        self.client
            .send_notification::<DepthNotification>(ForgeDepthParams {
                uri,
                depth,
            })
            .await;
    }

    fn get_text_up_to_cursor(&self, text: &str, position: Position) -> String {
        let mut text_up_to_cursor = if let Some(offset) = position_to_offset(text, position) {
            text[..offset].to_string()
        } else {
            text.to_string()
        };

        // Cap to last 8KB for efficiency
        if text_up_to_cursor.len() > 8 * 1024 {
            let len = text_up_to_cursor.len();
            text_up_to_cursor = text_up_to_cursor[len - 8 * 1024..].to_string();
        }
        text_up_to_cursor
    }

    fn find_active_function_call(&self, text_up_to_cursor: &str) -> Option<(String, usize)> {
        let mut depth = 0i32;
        let mut last_open_index: Option<usize> = None;

        for (idx, ch) in text_up_to_cursor.char_indices().rev() {
            match ch {
                ']' => depth += 1,
                '[' => {
                    if depth == 0 {
                        last_open_index = Some(idx);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }

        let open_index = last_open_index?;
        let before_bracket = &text_up_to_cursor[..open_index];
        let caps = SIGNATURE_FUNC_RE.captures(before_bracket)?;
        let func_name = caps.get(1)?.as_str().to_string();

        Some((func_name, open_index))
    }

    fn compute_active_param_index(&self, text_after_bracket: &str) -> u32 {
        let mut param_index: u32 = 0;
        let mut local_depth: i32 = 0;
        let mut in_single = false;
        let mut in_double = false;
        let mut prev_escape = false;

        for ch in text_after_bracket.chars() {
            if prev_escape {
                prev_escape = false;
                continue;
            }

            if ch == '\\' {
                prev_escape = true;
                continue;
            }

            if ch == '\'' && !in_double {
                in_single = !in_single;
                continue;
            }

            if ch == '"' && !in_single {
                in_double = !in_double;
                continue;
            }

            if in_single || in_double {
                continue;
            }

            match ch {
                '[' => local_depth += 1,
                ']' => {
                    if local_depth > 0 {
                        local_depth -= 1;
                    } else {
                        break;
                    }
                }
                ',' | ';' if local_depth == 0 => {
                    param_index = param_index.saturating_add(1);
                }
                _ => {}
            }
        }
        param_index
    }

    fn build_signature_help_parameters(
        &self,
        func: &crate::metadata::Function,
        mgr: &MetadataManager,
    ) -> Vec<ParameterInformation> {
        let args = func.args.clone().unwrap_or_default();
        args.iter()
            .map(|a| {
                let mut name = String::new();
                if a.rest {
                    name.push_str("...");
                }
                name.push_str(&a.name);
                if a.required != Some(true) || a.rest {
                    name.push('?');
                }

                // Add type info
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

                let mut doc = a.description.clone();

                if let Some(enum_name) = &a.enum_name {
                    if let Some(values) = mgr
                        .enums
                        .read()
                        .expect("Server: enums lock poisoned")
                        .get(enum_name)
                    {
                        let _ = writeln!(doc, "\n\n**{enum_name}**:");
                        for v in values {
                            let _ = writeln!(doc, "- {v}");
                        }
                    }
                } else if let Some(values) = &a.arg_enum {
                    doc.push_str("\n\n**Values**:\n");
                    for v in values {
                        doc.push_str("- ");
                        doc.push_str(v);
                        doc.push('\n');
                    }
                }

                ParameterInformation {
                    label: ParameterLabel::Simple(name),
                    documentation: Some(Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: doc,
                    })),
                }
            })
            .collect()
    }

    fn build_completion_item(
        &self,
        f: Arc<crate::metadata::Function>,
        modifier: &str,
    ) -> CompletionItem {
        let base = f.name.clone();
        let name = if !modifier.is_empty() && base.starts_with('$') {
            format!("${modifier}{}", &base[1..])
        } else {
            base.clone()
        };

        let md = self.build_completion_markdown(&f);

        CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(f.extension.clone().unwrap_or_else(|| {
                f.category
                    .clone()
                    .unwrap_or_else(|| "Function".to_string())
            })),
            documentation: Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            })),
            insert_text: Some(name),
            filter_text: Some(base),
            ..Default::default()
        }
    }

    fn build_completion_markdown(&self, f: &Arc<crate::metadata::Function>) -> String {
        let mut md = String::new();

        // Code block with signature
        md.push_str("```forgescript\n");
        md.push_str(&f.signature_label());
        md.push_str("\n```\n\n");

        if !f.description.is_empty() {
            md.push_str(&f.description);
            md.push_str("\n\n");
        }

        if let Some(examples) = &f.examples {
            if !examples.is_empty() {
                md.push_str("**Examples:**\n");
                for ex in examples.iter().take(2) {
                    md.push_str("\n```forgescript\n");
                    md.push_str(ex);
                    md.push_str("\n```\n");
                }
            }
        }

        // Links
        let mut links = Vec::new();
        if let Some(url) = &f.source_url {
            if url.contains("githubusercontent.com") {
                let parts: Vec<&str> = url.split('/').collect();
                if parts.len() >= 5 {
                    let owner = parts[3];
                    let repo = parts[4];
                    links.push(format!("[GitHub](https://github.com/{owner}/{repo})"));
                }
            }
        }

        if let Some(extension) = &f.extension {
            let base_url = "https://docs.botforge.org";
            links.push(format!(
                "[Documentation]({base_url}/function/{func_name}?p={extension})",
                base_url = base_url,
                func_name = f.name,
                extension = extension
            ));
        }

        if !links.is_empty() {
            md.push_str("\n---\n");
            md.push_str(&links.join(" | "));
        }

        md
    }
}

struct CustomNotification;
impl tower_lsp::lsp_types::notification::Notification for CustomNotification {
    type Params = ForgeHighlightsParams;
    const METHOD: &'static str = "forge/highlights";
}

struct DepthNotification;
impl tower_lsp::lsp_types::notification::Notification for DepthNotification {
    type Params = ForgeDepthParams;
    const METHOD: &'static str = "forge/updateDepth";
}



/// Returns the server capabilities for this LSP.
fn build_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec!["$".into(), ".".into()]),
            ..Default::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec![
                "$".into(),
                "[".into(),
                ";".into(),
                ",".into(),
                " ".into(),
            ]),
            retrigger_characters: Some(vec![",".into(), " ".into()]),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                work_done_progress_options: WorkDoneProgressOptions {
                    work_done_progress: None,
                },
                legend: SemanticTokensLegend {
                    token_types: vec![
                        SemanticTokenType::FUNCTION,  // 0
                        SemanticTokenType::KEYWORD,   // 1
                        SemanticTokenType::NUMBER,    // 2
                        SemanticTokenType::PARAMETER, // 3
                        SemanticTokenType::STRING,    // 4
                        SemanticTokenType::COMMENT,   // 5
                    ],
                    token_modifiers: vec![],
                },
                range: Some(false),
                full: Some(SemanticTokensFullOptions::Bool(true)),
            },
        )),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec!["forge/cursorMoved".to_string()],
            ..Default::default()
        }),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            file_operations: None,
        }),
        ..Default::default()
    }
}

#[async_trait]
impl LanguageServer for ForgeScriptServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(folders) = params.workspace_folders {
            let paths = folders
                .into_iter()
                .filter_map(|f| f.uri.to_file_path().ok())
                .collect::<Vec<_>>();
            {
                let mut ws_folders = self
                    .workspace_folders
                    .write()
                    .expect("Server: workspace_folders lock poisoned");
                (*ws_folders).clone_from(&paths);
            }

            if let Some((config, config_path)) = load_forge_config_full(&paths) {
                let urls = config.urls.clone();
                let manager = MetadataManager::new("./.cache", urls, Some(self.client.clone()))
                    .expect("Failed to initialize metadata manager");
                manager
                    .load_all()
                    .await
                    .expect("Failed to load metadata sources");

                // Load custom functions using the config and its directory
                manager
                    .load_custom_functions_from_config(&config, &config_path)
                    .expect("Failed to load custom functions");

                *self.manager.write().expect("Server: manager lock poisoned") = Arc::new(manager);

                // Load function color highlighting setting
                if let Some(use_colors) = config.multiple_function_colors {
                    *self
                        .multiple_function_colors
                        .write()
                        .expect("Server: multiple_function_colors lock poisoned") = use_colors;
                }

                if let Some(consistent) = config.consistent_function_colors {
                    *self
                        .consistent_function_colors
                        .write()
                        .expect("Server: consistent_function_colors lock poisoned") = consistent;
                }
            }
        }

        Ok(InitializeResult {
            capabilities: build_capabilities(),
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let count = self.function_count();

        self.client
            .log_message(
                MessageType::INFO,
                format!("[INFO] ForgeLSP initialized with {count} functions"),
            )
            .await;

        // Register file watcher for custom functions
        self.client
            .register_capability(vec![Registration {
                id: "watch-custom-functions".to_string(),
                method: "workspace/didChangeWatchedFiles".to_string(),
                register_options: Some(
                    serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                        watchers: vec![FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.{js,ts}".to_string()),
                            kind: Some(WatchKind::all()),
                        }],
                    })
                    .expect("Server: failed to serialize watch options"),
                ),
            }])
            .await
            .ok();
    }

    async fn shutdown(&self) -> Result<()> {
        spawn_log(
            self.client.clone(),
            MessageType::INFO,
            "[INFO] ForgeLSP shutting down".to_string(),
        );
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let start = std::time::Instant::now();
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let text_len = text.len();

        self.documents
            .write()
            .unwrap()
            .insert(uri.clone(), text.clone());

        self.process_text(uri.clone(), text).await;

        spawn_log(
            self.client.clone(),
            MessageType::LOG,
            format!(
                "[PERF] did_open: {text_len} chars in {elapsed:?}",
                elapsed = start.elapsed()
            ),
        );
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let start = std::time::Instant::now();

        if let Some(change) = params.content_changes.into_iter().next() {
            let uri = params.text_document.uri;
            let text = change.text;
            let text_len = text.len();

            self.documents
                .write()
                .expect("Server: documents lock poisoned")
                .insert(uri.clone(), text.clone());

            self.process_text(uri, text).await;

            spawn_log(
                self.client.clone(),
                MessageType::LOG,
                format!(
                    "[PERF] did_change: {text_len} chars in {elapsed:?}",
                    elapsed = start.elapsed()
                ),
            );
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        handle_hover(self, params).await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let text = {
            let docs = self.documents.read().expect("Server: documents lock poisoned");
            match docs.get(&uri) {
                Some(t) => t.clone(),
                _ => return Ok(None),
            }
        };

        let offset = match position_to_offset(&text, position) {
            Some(o) => o,
            _ => return Ok(None),
        };

        // Reuse is_ident_char from hover.rs logic or define here
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
        let mut current_char_idx = indices.len();
        for (idx, (byte_pos, _)) in indices.iter().enumerate() {
            if *byte_pos >= offset {
                current_char_idx = idx;
                break;
            }
        }

        let mut start_char_idx = current_char_idx;
        while start_char_idx > 0 {
            let (byte_pos, c) = indices[start_char_idx - 1];
            if is_ident_char(c) {
                if c == '$' && !crate::utils::is_escaped(&text, byte_pos) {
                    start_char_idx -= 1;
                    break;
                }
                start_char_idx -= 1;
            } else {
                break;
            }
        }

        let mut end_char_idx = current_char_idx;
        while end_char_idx < indices.len() {
            let (byte_pos, c) = indices[end_char_idx];
            if is_ident_char(c) {
                if c == '$' && !crate::utils::is_escaped(&text, byte_pos) && end_char_idx > start_char_idx {
                    break;
                }
                end_char_idx += 1;
            } else {
                break;
            }
        }

        if start_char_idx >= end_char_idx {
            return Ok(None);
        }

        let start_byte = indices[start_char_idx].0;
        let end_byte = if end_char_idx < indices.len() {
            indices[end_char_idx].0
        } else {
            text.len()
        };

        let raw_token = text[start_byte..end_byte].to_string();
        let mut clean_token = raw_token.clone();

        if clean_token.starts_with('$') {
            let modifier_end_idx = crate::utils::skip_modifiers(&clean_token, 1);
            if modifier_end_idx > 1 {
                let after_modifiers = &clean_token[modifier_end_idx..];
                if !after_modifiers.is_empty() {
                    clean_token = format!("${after_modifiers}");
                }
            }
        }

        let mgr = self.manager.read().expect("Server: manager lock poisoned");
        if let Some(func) = mgr.get(&clean_token) {
            if let (Some(path), Some(line)) = (&func.local_path, func.line) {
                let target_uri = Url::from_file_path(path).map_err(|_| {
                    tower_lsp::jsonrpc::Error::invalid_params("Invalid file path for definition")
                })?;
                
                return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri: target_uri,
                    range: Range::new(Position::new(line, 0), Position::new(line, 0)),
                })));
            }
        }

        Ok(None)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let docs = self
            .documents
            .read()
            .expect("Server: documents lock poisoned");
        let text = docs.get(&uri);

        let Some(text) = text else { return Ok(None); };

        let lines: Vec<&str> = text.lines().collect();
        let line = lines.get(position.line as usize).unwrap_or(&"");

        let byte_offset = position_to_offset(line, Position::new(0, position.character)).unwrap_or(line.len());
        let before_cursor = &line[..byte_offset];

        let Some(last_dollar_idx) = before_cursor.rfind('$') else {
            return Ok(None);
        };

        let after_dollar = &before_cursor[last_dollar_idx + 1..];
        let mut modifier = "";

        if after_dollar.starts_with('!') {
            modifier = "!";
        } else if after_dollar.starts_with('.') {
            modifier = ".";
        }

        let mgr = self.manager.read().expect("Server: manager lock poisoned").clone();
        let items: Vec<CompletionItem> = mgr
            .all_functions()
            .into_iter()
            .map(|f| self.build_completion_item(f, modifier))
            .collect();

        spawn_log(
            self.client.clone(),
            MessageType::LOG,
            format!(
                "[PERF] completion: {count} items in {elapsed:?}",
                count = items.len(),
                elapsed = start.elapsed()
            ),
        );
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let start = std::time::Instant::now();

        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().expect("Server: documents lock poisoned");
        let Some(text) = docs.get(&uri) else { return Ok(None); };

        let text_up_to_cursor = self.get_text_up_to_cursor(text, position);
        let Some((func_name, open_index)) = self.find_active_function_call(&text_up_to_cursor) else {
            return Ok(None);
        };

        let param_index = self.compute_active_param_index(&text_up_to_cursor[open_index + 1..]);

        let mgr = self.manager.read().expect("Server: manager lock poisoned").clone();
        let lookup = format!("${func_name}");

        if let Some(func) = mgr.get(&lookup) {
            let params = self.build_signature_help_parameters(&func, &mgr);
            let sig_label = func.signature_label();

            let signature = SignatureInformation {
                label: sig_label,
                documentation: Some(Documentation::String(func.description.clone())),
                parameters: Some(params),
                active_parameter: Some(param_index),
            };

            spawn_log(
                self.client.clone(),
                MessageType::LOG,
                format!(
                    "[PERF] signature_help: ${func_name} in {elapsed:?}",
                    elapsed = start.elapsed()
                ),
            );

            return Ok(Some(SignatureHelp {
                signatures: vec![signature],
                active_signature: Some(0),
                active_parameter: Some(param_index),
            }));
        }

        Ok(None)
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let start = std::time::Instant::now();
        let uri = params.text_document.uri;

        let docs = self
            .documents
            .read()
            .expect("Server: documents lock poisoned");

        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };

        let use_colors = *self
            .multiple_function_colors
            .read()
            .expect("Server: multiple_function_colors lock poisoned");
        let mgr = self
            .manager
            .read()
            .expect("Server: manager lock poisoned")
            .clone();
        let tokens = extract_semantic_tokens_with_colors(text, use_colors, &mgr);

        spawn_log(
            self.client.clone(),
            MessageType::LOG,
            format!(
                "[PERF] semantic_tokens: {count} tokens in {elapsed:?}",
                count = tokens.len(),
                elapsed = start.elapsed()
            ),
        );

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<serde_json::Value>> {
        if params.command == "forge/cursorMoved" {
            if let Some(args) = params.arguments.get(0) {
                if let Ok(moved_params) = serde_json::from_value::<CursorMovedParams>(args.clone()) {
                    {
                        let mut cursors = self.cursor_positions.write().expect("Server: cursors lock poisoned");
                        cursors.insert(moved_params.uri.clone(), moved_params.position);
                    }
                    self.update_depth(moved_params.uri).await;
                }
            }
        }
        Ok(None)
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
    let manager_outer = self.manager.read().expect("Server: manager lock poisoned").clone();
    let manager = manager_outer.as_ref();

    let config_guard = self.config.read().expect("Server: config lock poisoned");
    let workspace_folders = self.workspace_folders.read().expect("Server: workspace lock poisoned");

    // Get the raw string from config (e.g., "custom_logic")
    let relative_custom_path = config_guard
        .as_ref()
        .and_then(|c| c.custom_functions_path.as_ref());

    for change in params.changes {
        if let Ok(path) = change.uri.to_file_path() {
            
            // Validate if the file is in the custom folder
            let mut is_in_custom_folder = false;
            
            if let Some(rel_path) = relative_custom_path {
                for root in workspace_folders.iter() {
                    // Combine workspace root + relative config path
                    let full_allowed_dir = root.join(rel_path);
                    
                    // Use starts_with on the absolute paths
                    if path.starts_with(&full_allowed_dir) {
                        is_in_custom_folder = true;
                        break;
                    }
                }
            }

            if !is_in_custom_folder {
                continue;
            }

            // Only process .js or .ts files
            let extension = path.extension().and_then(|s| s.to_str());
            if !matches!(extension, Some("js") | Some("ts")) {
                continue;
            }

            match change.typ {
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    if let Ok(count) = manager.reload_file(path.clone()) {
                        spawn_log(
                            self.client.clone(),
                            MessageType::INFO,
                            format!(
                                "[INFO] Reloaded custom functions from {}: {count} functions registered",
                                path.display()
                            ),
                        );
                    }
                }
                FileChangeType::DELETED => {
                    manager.remove_functions_at_path(&path);
                    spawn_log(
                        self.client.clone(),
                        MessageType::INFO,
                        format!(
                            "[INFO] Removed custom functions from deleted file: {}",
                            path.display()
                        ),
                    );
                }
                _ => {}
            }
        }
    }
}
}
