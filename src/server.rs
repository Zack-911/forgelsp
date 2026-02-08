//! Implements the core Language Server Protocol (LSP) logic for ForgeScript.
//!
//! This module handles document synchronization, provides intelligent features like
//! hover, completion, and signature help, and manages the lifecycle of the LSP server.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, RwLock};

use crate::diagnostics::publish_diagnostics;
use crate::hover::handle_hover;
use crate::metadata::MetadataManager;
use crate::parser::{ForgeScriptParser, ParseResult};
use crate::utils::{ForgeConfig, load_forge_config_full};
use regex::Regex;
use tower_lsp::Client;
use tower_lsp::LanguageServer;
use tower_lsp::async_trait;
use tower_lsp::jsonrpc::Result;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;

/// Regex for identifying '$' followed by alphanumeric characters at the end of a string.
pub(crate) static SIGNATURE_FUNC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*$").expect("Server: regex failure"));

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

/// The core ForgeScript language server state.
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
    /// Parses the updated text, updates the cache, and triggers diagnostic/highlight updates.
    pub async fn process_text(&self, uri: Url, text: String) {
        let start = std::time::Instant::now();
        crate::utils::forge_log(
            crate::utils::LogLevel::Debug,
            &format!("Processing text for {}", uri),
        );
        let mgr_arc = self.manager.read().expect("Server: lock poisoned").clone();
        let parser = ForgeScriptParser::new(mgr_arc, &text);
        let parsed = parser.parse();

        self.parsed_cache
            .write()
            .expect("Server: lock poisoned")
            .insert(uri.clone(), parsed.clone());

        publish_diagnostics(self, &uri, &text, &parsed.diagnostics).await;
        crate::semantic::handle_send_highlights(self, uri.clone(), &text).await;
        crate::depth::handle_update_depth(self, uri.clone()).await;

        crate::utils::forge_log(
            crate::utils::LogLevel::Debug,
            &format!("Finished processing {} in {:?}", uri, start.elapsed()),
        );
    }

    /// Returns the total number of functions currently indexed by the metadata manager.
    pub fn function_count(&self) -> usize {
        self.manager
            .read()
            .expect("Server: lock poisoned")
            .function_count()
    }
    pub(crate) fn get_text_up_to_cursor(&self, text: &str, position: Position) -> String {
        let mut text_up_to_cursor =
            if let Some(offset) = crate::utils::position_to_offset(text, position) {
                text[..offset].to_string()
            } else {
                text.to_string()
            };

        if text_up_to_cursor.len() > 8 * 1024 {
            let len = text_up_to_cursor.len();
            text_up_to_cursor = text_up_to_cursor[len - 8 * 1024..].to_string();
        }
        text_up_to_cursor
    }

    pub(crate) fn find_active_function_call(
        &self,
        text_up_to_cursor: &str,
    ) -> Option<(String, usize)> {
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

    pub(crate) fn compute_active_param_index(&self, text_after_bracket: &str) -> u32 {
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
}

pub(crate) struct CustomNotification;
impl tower_lsp::lsp_types::notification::Notification for CustomNotification {
    type Params = ForgeHighlightsParams;
    const METHOD: &'static str = "forge/highlights";
}

pub(crate) struct DepthNotification;
impl tower_lsp::lsp_types::notification::Notification for DepthNotification {
    type Params = ForgeDepthParams;
    const METHOD: &'static str = "forge/updateDepth";
}

pub(crate) struct TriggerCompletionNotification;
impl tower_lsp::lsp_types::notification::Notification for TriggerCompletionNotification {
    type Params = Url;
    const METHOD: &'static str = "forge/triggerCompletion";
}

fn build_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(false),
            trigger_characters: Some(vec![
                "$".into(),
                ".".into(),
                "[".into(),
                ";".into(),
                ",".into(),
                " ".into(),
            ]),
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
                work_done_progress_options: WorkDoneProgressOptions::default(),
                legend: SemanticTokensLegend {
                    token_types: vec![
                        SemanticTokenType::FUNCTION,
                        SemanticTokenType::KEYWORD,
                        SemanticTokenType::NUMBER,
                        SemanticTokenType::PARAMETER,
                        SemanticTokenType::STRING,
                        SemanticTokenType::COMMENT,
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
            ..Default::default()
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
            self.workspace_folders
                .write()
                .expect("Server: lock poisoned")
                .clone_from(&paths);

            if let Some((config, config_path)) = load_forge_config_full(&paths) {
                let manager = MetadataManager::new(
                    "./.cache",
                    config.urls.clone(),
                    Some(self.client.clone()),
                )
                .expect("Metadata initialization failed");
                manager.load_all().await.expect("Metadata load failed");
                manager
                    .load_custom_functions_from_config(&config, &config_path)
                    .expect("Custom function load failed");

                *self.manager.write().expect("Server: lock poisoned") = Arc::new(manager);
                if let Some(use_colors) = config.multiple_function_colors {
                    *self
                        .multiple_function_colors
                        .write()
                        .expect("Server: lock poisoned") = use_colors;
                }
                if let Some(consistent) = config.consistent_function_colors {
                    *self
                        .consistent_function_colors
                        .write()
                        .expect("Server: lock poisoned") = consistent;
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
                    .expect("Server: serialization failure"),
                ),
            }])
            .await
            .ok();
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        self.documents
            .write()
            .expect("Server: lock poisoned")
            .insert(uri.clone(), text.clone());
        self.process_text(uri.clone(), text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.first() {
            let uri = params.text_document.uri;
            let text = change.text.clone();
            self.documents
                .write()
                .expect("Server: lock poisoned")
                .insert(uri.clone(), text.clone());
            self.process_text(uri, text).await;
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        handle_hover(self, params).await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        crate::definition::handle_definition(self, params).await
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        crate::folding_range::handle_folding_range(self, params).await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        crate::completion::handle_completion(self, params).await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        crate::signature_help::handle_signature_help(self, params).await
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        crate::semantic::handle_semantic_tokens_full(self, params).await
    }

    async fn execute_command(
        &self,
        params: ExecuteCommandParams,
    ) -> Result<Option<serde_json::Value>> {
        crate::commands::handle_execute_command(self, params).await
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mgr_outer = self.manager.read().expect("Server: lock poisoned").clone();
        let mgr = mgr_outer.as_ref();
        let config = self.config.read().expect("Server: lock poisoned");
        let ws_folders = self
            .workspace_folders
            .read()
            .expect("Server: lock poisoned");
        let rel_path = config
            .as_ref()
            .and_then(|c| c.custom_functions_path.as_ref());

        for change in params.changes {
            if let Ok(path) = change.uri.to_file_path() {
                let mut is_allowed = false;
                if let Some(rel) = rel_path {
                    for root in ws_folders.iter() {
                        if path.starts_with(root.join(rel)) {
                            is_allowed = true;
                            break;
                        }
                    }
                }
                if !is_allowed {
                    continue;
                }
                if !matches!(
                    path.extension().and_then(|s| s.to_str()),
                    Some("js") | Some("ts")
                ) {
                    continue;
                }

                match change.typ {
                    FileChangeType::CREATED | FileChangeType::CHANGED => {
                        if let Ok(_count) = mgr.reload_file(path.clone()) {}
                    }
                    FileChangeType::DELETED => {
                        mgr.remove_functions_at_path(&path);
                    }
                    _ => {}
                }
            }
        }
    }
}
