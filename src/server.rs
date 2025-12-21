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

use std::collections::HashMap;
use std::fmt::Write;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use crate::diagnostics::publish_diagnostics;
use crate::hover::handle_hover;
use crate::metadata::MetadataManager;
use crate::parser::{ForgeScriptParser, ParseResult};
use crate::semantic::extract_semantic_tokens_with_colors;
use crate::utils::{load_forge_config_full, spawn_log};
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

    /// Returns a list of all functions managed by the server.
    pub fn all_functions(&self) -> Vec<Arc<crate::metadata::Function>> {
        let mgr = self.manager.read().expect("Server: manager lock poisoned");
        mgr.all_functions()
    }

    /// Convert UTF-16 character offset to byte offset for a line
    fn get_byte_offset(line: &str, utf16_col: u32) -> usize {
        let mut col = 0;
        for (i, c) in line.char_indices() {
            if col == utf16_col {
                return i;
            }
            col += u32::try_from(c.len_utf16()).expect("UTF-16 length exceeds u32");
        }
        if col == utf16_col {
            return line.len();
        }
        // Fallback to clamping to line length if out of bounds
        line.len()
    }
}

/// Helper to load custom functions from folders specified in the config.
fn load_custom_functions_from_path(
    client: &Client,
    manager: &MetadataManager,
    paths: &[PathBuf],
    custom_path: &str,
) {
    spawn_log(
        client.clone(),
        MessageType::INFO,
        format!("[INFO] Custom functions path from config: {custom_path}"),
    );
    for folder in paths {
        let path = folder.join(custom_path);
        spawn_log(
            client.clone(),
            MessageType::INFO,
            format!(
                "[INFO] Searching for custom functions in: {}",
                path.display()
            ),
        );
        if path.exists() {
            match manager.load_custom_functions_from_folder(path) {
                Ok((files, count)) => {
                    spawn_log(
                        client.clone(),
                        MessageType::INFO,
                        format!(
                            "[INFO] Found {files_count} .js/.ts files, registered {count} custom functions",
                            files_count = files.len()
                        ),
                    );
                    for f in files {
                        spawn_log(
                            client.clone(),
                            MessageType::INFO,
                            format!("[INFO] Parsed file: {}", f.display()),
                        );
                    }
                }
                Err(e) => {
                    spawn_log(
                        client.clone(),
                        MessageType::ERROR,
                        format!("[ERROR] Failed to load custom functions from folder: {e}"),
                    );
                }
            }
        } else {
            spawn_log(
                client.clone(),
                MessageType::WARNING,
                format!(
                    "[WARN] Custom functions path does not exist: {}",
                    path.display()
                ),
            );
        }
    }
}

/// Returns the server capabilities for this LSP.
fn build_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
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

            if let Some(config) = load_forge_config_full(&paths) {
                let urls = config.urls.clone();
                let manager = MetadataManager::new("./.cache", urls)
                    .expect("Failed to initialize metadata manager");
                manager
                    .load_all()
                    .await
                    .expect("Failed to load metadata sources");

                // Load custom functions from config if available
                if let Some(custom_funcs) = config.custom_functions.filter(|f| !f.is_empty()) {
                    manager
                        .add_custom_functions(custom_funcs)
                        .expect("Failed to add custom functions");
                }

                // Load custom functions from path if available
                if let Some(custom_path) = config.custom_functions_path {
                    load_custom_functions_from_path(&self.client, &manager, &paths, &custom_path);
                }

                *self.manager.write().expect("Server: manager lock poisoned") = Arc::new(manager);

                // Load function color highlighting setting
                if let Some(use_colors) = config.multiple_function_colors {
                    *self
                        .multiple_function_colors
                        .write()
                        .expect("Server: multiple_function_colors lock poisoned") = use_colors;
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

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let docs = self
            .documents
            .read()
            .expect("Server: documents lock poisoned");
        let text = docs.get(&uri);

        if let Some(text) = text {
            let lines: Vec<&str> = text.lines().collect();
            let line = lines.get(position.line as usize).unwrap_or(&"");

            let byte_offset = Self::get_byte_offset(line, position.character);
            let before_cursor = &line[..byte_offset];

            if let Some(last_dollar_idx) = before_cursor.rfind('$') {
                let after_dollar = &before_cursor[last_dollar_idx + 1..];
                let mut modifier = "";

                if after_dollar.starts_with('!') {
                    modifier = "!";
                } else if after_dollar.starts_with('.') {
                    modifier = ".";
                }

                let items: Vec<CompletionItem> = self
                    .all_functions()
                    .into_iter()
                    .map(|f| {
                        let base = f.name.clone();
                        let name = if !modifier.is_empty() && base.starts_with('$') {
                            format!("${modifier}{}", &base[1..])
                        } else {
                            base.clone()
                        };

                        CompletionItem {
                            label: name.clone(),
                            kind: Some(CompletionItemKind::FUNCTION),
                            detail: Some(f.category.clone()),
                            documentation: Some(Documentation::String(f.description.clone())),
                            insert_text: Some(name),
                            // Important: filter WITHOUT modifier
                            filter_text: Some(base),
                            ..Default::default()
                        }
                    })
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
                return Ok(Some(CompletionResponse::Array(items)));
            }
        }

        Ok(None)
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let start = std::time::Instant::now();

        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self
            .documents
            .read()
            .expect("Server: documents lock poisoned");
        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };

        // Gather everything before the cursor
        let mut text_up_to_cursor = String::new();
        for (i, line) in text.lines().enumerate() {
            if i < position.line as usize {
                text_up_to_cursor.push_str(line);
                text_up_to_cursor.push('\n');
            } else if i == position.line as usize {
                let byte_offset = Self::get_byte_offset(line, position.character);
                let slice = &line[..byte_offset];
                text_up_to_cursor.push_str(slice);
                break;
            }
        }

        // Cap to last 8KB for efficiency
        if text_up_to_cursor.len() > 8 * 1024 {
            let len = text_up_to_cursor.len();
            text_up_to_cursor = text_up_to_cursor[len - 8 * 1024..].to_string();
        }

        // Scan backwards to find the nearest unmatched '['
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

        let Some(open_index) = last_open_index else {
            return Ok(None);
        };

        // Extract the function name before the '['
        let before_bracket = &text_up_to_cursor[..open_index];
        let Some(caps) = SIGNATURE_FUNC_RE.captures(before_bracket) else {
            return Ok(None);
        };

        let func_name = caps
            .get(1)
            .expect("Server: signature regex capture group missing")
            .as_str();

        // Compute active parameter index
        let start_scan = open_index + 1;
        let sub = &text_up_to_cursor[start_scan..];
        let mut param_index: u32 = 0;
        let mut local_depth: i32 = 0;
        let mut in_single = false;
        let mut in_double = false;
        let mut prev_escape = false;

        for ch in sub.chars() {
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

        // Look up metadata and return signature help
        let mgr = self
            .manager
            .read()
            .expect("Server: manager lock poisoned")
            .clone();
        let lookup = format!("${func_name}");

        if let Some(func) = mgr.get(&lookup) {
            let args = func.args.clone().unwrap_or_default();
            let params: Vec<ParameterInformation> = args
                .iter()
                .map(|a| {
                    let mut name = String::new();
                    if a.rest {
                        name.push_str("...");
                    }
                    name.push_str(&a.name);
                    if a.required == Some(false) {
                        name.push('?');
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
                        // Handle inline enums if any
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
                .collect();

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

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let manager_outer = self
            .manager
            .read()
            .expect("Server: manager lock poisoned")
            .clone();
        let manager = manager_outer.as_ref();
        for change in params.changes {
            if let Ok(path) = change.uri.to_file_path() {
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
