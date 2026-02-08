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
use crate::semantic::extract_semantic_tokens_with_colors;
use crate::utils::{load_forge_config_full, position_to_offset, spawn_log, ForgeConfig};
use regex::Regex;
use tower_lsp::async_trait;
use tower_lsp::jsonrpc::Result;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;
use tower_lsp::Client;
use tower_lsp::LanguageServer;

/// Regex for identifying '$' followed by alphanumeric characters at the end of a string.
static SIGNATURE_FUNC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*$").expect("Server: regex failure")
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
        let mgr_arc = self.manager.read().expect("Server: lock poisoned").clone();
        let parser = ForgeScriptParser::new(mgr_arc, &text);
        let parsed = parser.parse();

        self.parsed_cache.write().expect("Server: lock poisoned").insert(uri.clone(), parsed.clone());

        publish_diagnostics(self, &uri, &text, &parsed.diagnostics).await;
        self.send_highlights(uri.clone(), &text).await;
        self.update_depth(uri).await;

        let diag_count = parsed.diagnostics.len();
        if diag_count > 0 {
            spawn_log(self.client.clone(), MessageType::WARNING, format!("[WARN] {diag_count} diagnostics found in {elapsed:?}", elapsed = start.elapsed()));
        }
    }

    /// Returns the total number of functions currently indexed by the metadata manager.
    pub fn function_count(&self) -> usize {
        self.manager.read().expect("Server: lock poisoned").function_count()
    }

    /// Computes and sends custom syntax highlighting data to the client.
    pub async fn send_highlights(&self, uri: Url, text: &str) {
        let highlights = {
            let colors = self.function_colors.read().expect("Server: lock poisoned").clone();
            if colors.is_empty() { return; }

            let mgr = self.manager.read().expect("Server: lock poisoned").clone();
            let consistent = *self.consistent_function_colors.read().expect("Server: lock poisoned");

            crate::semantic::extract_highlight_ranges(text, &colors, consistent, &mgr)
                .into_iter()
                .map(|(start, end, color)| {
                    HighlightRange {
                        range: Range::new(crate::utils::offset_to_position(text, start), crate::utils::offset_to_position(text, end)),
                        color,
                    }
                })
                .collect::<Vec<HighlightRange>>()
        };

        self.client.send_notification::<CustomNotification>(ForgeHighlightsParams { uri, highlights }).await;
    }

    /// Notifies the client about the current nesting depth at the cursor position.
    pub async fn update_depth(&self, uri: Url) {
        let depth = {
            let docs = self.documents.read().expect("Server: lock poisoned");
            let Some(text) = docs.get(&uri) else { return; };
            let cursors = self.cursor_positions.read().expect("Server: lock poisoned");
            let Some(&position) = cursors.get(&uri) else { return; };
            let offset = position_to_offset(text, position).unwrap_or(0);
            crate::utils::calculate_depth(text, offset)
        };
        self.client.send_notification::<DepthNotification>(ForgeDepthParams { uri, depth }).await;
    }

    fn get_text_up_to_cursor(&self, text: &str, position: Position) -> String {
        let mut text_up_to_cursor = if let Some(offset) = position_to_offset(text, position) {
            text[..offset].to_string()
        } else { text.to_string() };

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
                    if depth == 0 { last_open_index = Some(idx); break; }
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
            if prev_escape { prev_escape = false; continue; }
            if ch == '\\' { prev_escape = true; continue; }
            if ch == '\'' && !in_double { in_single = !in_single; continue; }
            if ch == '"' && !in_single { in_double = !in_double; continue; }
            if in_single || in_double { continue; }

            match ch {
                '[' => local_depth += 1,
                ']' => { if local_depth > 0 { local_depth -= 1; } else { break; } }
                ',' | ';' if local_depth == 0 => { param_index = param_index.saturating_add(1); }
                _ => {}
            }
        }
        param_index
    }

    fn build_signature_help_parameters(&self, func: &crate::metadata::Function) -> Vec<ParameterInformation> {
        func.args.clone().unwrap_or_default().iter().map(|a| {
            let mut name = String::new();
            if a.rest { name.push_str("..."); }
            name.push_str(&a.name);
            if a.required != Some(true) || a.rest { name.push('?'); }

            let type_str = match &a.arg_type {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(arr) => arr.iter().map(|v| v.as_str().unwrap_or("?").to_string()).collect::<Vec<_>>().join("|"),
                _ => "Any".to_string(),
            };
            if !type_str.is_empty() { name.push_str(": "); name.push_str(&type_str); }

            ParameterInformation {
                label: ParameterLabel::Simple(name),
                documentation: Some(Documentation::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value: a.description.clone() })),
            }
        }).collect()
    }

    fn build_completion_item(&self, f: Arc<crate::metadata::Function>, modifier: &str, range: Range) -> CompletionItem {
        let base = f.name.clone();
        let name = if !modifier.is_empty() && base.starts_with('$') { format!("${modifier}{}", &base[1..]) } else { base.clone() };

        CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(f.extension.clone().unwrap_or_else(|| f.category.clone().unwrap_or_else(|| "Function".to_string()))),
            documentation: Some(Documentation::MarkupContent(MarkupContent { kind: MarkupKind::Markdown, value: self.build_completion_markdown(&f) })),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit { range, new_text: name })),
            filter_text: Some(base),
            ..Default::default()
        }
    }

    fn build_completion_markdown(&self, f: &Arc<crate::metadata::Function>) -> String {
        let mut md = format!("```forgescript\n{}\n```\n\n", f.signature_label());
        if !f.description.is_empty() { md.push_str(&f.description); md.push_str("\n\n"); }

        if let Some(examples) = &f.examples {
            if !examples.is_empty() {
                md.push_str("**Examples:**\n");
                for ex in examples.iter().take(2) { md.push_str(&format!("\n```forgescript\n{ex}\n```\n")); }
            }
        }

        let mut links = Vec::new();
        if let Some(url) = &f.source_url && url.contains("githubusercontent.com") {
            let parts: Vec<&str> = url.split('/').collect();
            if parts.len() >= 5 { links.push(format!("[GitHub](https://github.com/{}/{})", parts[3], parts[4])); }
        }

        if let Some(extension) = &f.extension {
            links.push(format!("[Documentation](https://docs.botforge.org/function/{}?p={})", f.name, extension));
        }

        if !links.is_empty() { md.push_str("\n---\n"); md.push_str(&links.join(" | ")); }
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

struct TriggerCompletionNotification;
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
            trigger_characters: Some(vec!["$".into(), ".".into(), "[".into(), ";".into(), ",".into(), " ".into()]),
            ..Default::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["$".into(), "[".into(), ";".into(), ",".into(), " ".into()]),
            retrigger_characters: Some(vec![",".into(), " ".into()]),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        }),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                work_done_progress_options: WorkDoneProgressOptions::default(),
                legend: SemanticTokensLegend {
                    token_types: vec![
                        SemanticTokenType::FUNCTION, SemanticTokenType::KEYWORD, SemanticTokenType::NUMBER,
                        SemanticTokenType::PARAMETER, SemanticTokenType::STRING, SemanticTokenType::COMMENT,
                    ],
                    token_modifiers: vec![],
                },
                range: Some(false),
                full: Some(SemanticTokensFullOptions::Bool(true)),
            },
        )),
        execute_command_provider: Some(ExecuteCommandOptions { commands: vec!["forge/cursorMoved".to_string()], ..Default::default() }),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities { supported: Some(true), change_notifications: Some(OneOf::Left(true)) }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

#[async_trait]
impl LanguageServer for ForgeScriptServer {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        if let Some(folders) = params.workspace_folders {
            let paths = folders.into_iter().filter_map(|f| f.uri.to_file_path().ok()).collect::<Vec<_>>();
            self.workspace_folders.write().expect("Server: lock poisoned").clone_from(&paths);

            if let Some((config, config_path)) = load_forge_config_full(&paths) {
                let manager = MetadataManager::new("./.cache", config.urls.clone(), Some(self.client.clone())).expect("Metadata initialization failed");
                manager.load_all().await.expect("Metadata load failed");
                manager.load_custom_functions_from_config(&config, &config_path).expect("Custom function load failed");

                *self.manager.write().expect("Server: lock poisoned") = Arc::new(manager);
                if let Some(use_colors) = config.multiple_function_colors { *self.multiple_function_colors.write().expect("Server: lock poisoned") = use_colors; }
                if let Some(consistent) = config.consistent_function_colors { *self.consistent_function_colors.write().expect("Server: lock poisoned") = consistent; }
            }
        }
        Ok(InitializeResult { capabilities: build_capabilities(), ..Default::default() })
    }

    async fn initialized(&self, _: InitializedParams) {
        let count = self.function_count();
        self.client.log_message(MessageType::INFO, format!("[INFO] ForgeLSP initialized with {count} functions")).await;
        self.client.register_capability(vec![Registration {
            id: "watch-custom-functions".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                watchers: vec![FileSystemWatcher { glob_pattern: GlobPattern::String("**/*.{js,ts}".to_string()), kind: Some(WatchKind::all()) }],
            }).expect("Server: serialization failure")),
        }]).await.ok();
    }

    async fn shutdown(&self) -> Result<()> {
        spawn_log(self.client.clone(), MessageType::INFO, "[INFO] ForgeLSP shutting down".to_string());
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let start = std::time::Instant::now();
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let text_len = text.len();
        self.documents.write().expect("Server: lock poisoned").insert(uri.clone(), text.clone());
        self.process_text(uri.clone(), text).await;
        spawn_log(self.client.clone(), MessageType::LOG, format!("[PERF] did_open: {text_len} chars in {elapsed:?}", elapsed = start.elapsed()));
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let start = std::time::Instant::now();
        if let Some(change) = params.content_changes.into_iter().next() {
            let uri = params.text_document.uri;
            let text = change.text;
            let text_len = text.len();
            self.documents.write().expect("Server: lock poisoned").insert(uri.clone(), text.clone());
            self.process_text(uri, text).await;
            spawn_log(self.client.clone(), MessageType::LOG, format!("[PERF] did_change: {text_len} chars in {elapsed:?}", elapsed = start.elapsed()));
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> { handle_hover(self, params).await }

    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        let text = self.documents.read().expect("Server: lock poisoned").get(&uri).cloned().ok_or(tower_lsp::jsonrpc::Error::invalid_params("Document not found"))?;
        let offset = position_to_offset(&text, position).ok_or(tower_lsp::jsonrpc::Error::invalid_params("Invalid position"))?;

        let is_ident_char = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == '$' || c == '!' || c == '#' || c == '@' || c == '[' || c == ']';
        let indices: Vec<(usize, char)> = text.char_indices().collect();
        let curr = indices.iter().position(|&(p, _)| p >= offset).unwrap_or(indices.len());

        let mut start = curr;
        while start > 0 && is_ident_char(indices[start - 1].1) {
            start -= 1;
            if indices[start].1 == '$' && !crate::utils::is_escaped(&text, indices[start].0) { break; }
        }

        let mut end = curr;
        while end < indices.len() && is_ident_char(indices[end].1) {
            if indices[end].1 == '$' && !crate::utils::is_escaped(&text, indices[end].0) && end > start { break; }
            end += 1;
        }

        if start >= end { return Ok(None); }
        let mut token = text[indices[start].0..if end < indices.len() { indices[end].0 } else { text.len() }].to_string();

        if token.starts_with('$') {
            let mod_end = crate::utils::skip_modifiers(&token, 1);
            if mod_end > 1 && !&token[mod_end..].is_empty() { token = format!("${}", &token[mod_end..]); }
        }

        if let Some(func) = self.manager.read().expect("Server: lock poisoned").get(&token) && let (Some(path), Some(line)) = (&func.local_path, func.line) {
            let target_uri = Url::from_file_path(path).map_err(|_| tower_lsp::jsonrpc::Error::internal_error())?;
            return Ok(Some(GotoDefinitionResponse::Scalar(Location { uri: target_uri, range: Range::new(Position::new(line, 0), Position::new(line, 0)) })));
        }
        Ok(None)
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = params.text_document.uri;
        let parsed = self.parsed_cache.read().expect("Server: lock poisoned").get(&uri).cloned().ok_or(tower_lsp::jsonrpc::Error::invalid_params("Not parsed"))?;
        let text = self.documents.read().expect("Server: lock poisoned").get(&uri).cloned().ok_or(tower_lsp::jsonrpc::Error::invalid_params("No text"))?;

        let mut ranges = Vec::new();
        for func in &parsed.functions {
            let start = crate::utils::offset_to_position(&text, func.span.0);
            let end = crate::utils::offset_to_position(&text, func.span.1);
            if start.line < end.line {
                ranges.push(FoldingRange {
                    start_line: start.line, start_character: Some(start.character),
                    end_line: end.line, end_character: Some(end.character),
                    kind: Some(FoldingRangeKind::Region), collapsed_text: Some(func.name.clone()),
                });
            }
        }
        Ok(Some(ranges))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let text = self.documents.read().expect("Server: lock poisoned").get(&uri).cloned().ok_or(tower_lsp::jsonrpc::Error::invalid_params("Document not found"))?;
        let mgr = self.manager.read().expect("Server: lock poisoned").clone();

        let text_up_to_cursor = self.get_text_up_to_cursor(&text, position);
        if let Some((func_name, open_idx)) = self.find_active_function_call(&text_up_to_cursor) {
            let param_idx = self.compute_active_param_index(&text_up_to_cursor[open_idx + 1..]) as usize;
            if let Some(func) = mgr.get(&format!("${func_name}")) && let Some(args) = &func.args {
                let arg_idx = if param_idx >= args.len() && args.last().map(|a| a.rest).unwrap_or(false) { args.len() - 1 } else { param_idx };
                if let Some(arg) = args.get(arg_idx) {
                    let enum_vals = if let Some(en) = &arg.enum_name { mgr.enums.read().expect("Server: lock poisoned").get(en).cloned() } else { arg.arg_enum.clone() };
                    if let Some(vals) = enum_vals {
                        let items = vals.into_iter().map(|v| CompletionItem { label: v.clone(), kind: Some(CompletionItemKind::ENUM_MEMBER), detail: Some(format!("Enum for {}", arg.name)), insert_text: Some(v), ..Default::default() }).collect();
                        return Ok(Some(CompletionResponse::Array(items)));
                    }
                }
            }
        }

        let line = text.lines().nth(position.line as usize).unwrap_or("");
        let offset = position_to_offset(line, Position::new(0, position.character)).unwrap_or(line.len());
        let before = &line[..offset];
        
        let Some(dollar_idx) = before.rfind('$') else { return Ok(None); };
        let after_dollar = &before[dollar_idx + 1..];
        let modifier = if after_dollar.starts_with('!') { "!" } else if after_dollar.starts_with('.') { "." } else { "" };

        let mut start_char = 0;
        for c in line[..dollar_idx].chars() { start_char += c.len_utf16() as u32; }
        let range = Range::new(Position::new(position.line, start_char), position);

        let items = mgr.all_functions().into_iter().map(|f| self.build_completion_item(f, modifier, range)).collect::<Vec<_>>();
        spawn_log(self.client.clone(), MessageType::LOG, format!("[PERF] completion: {} items in {elapsed:?}", items.len(), elapsed = start.elapsed()));
        Ok(Some(CompletionResponse::Array(items)))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let text = self.documents.read().expect("Server: lock poisoned").get(&uri).cloned().ok_or(tower_lsp::jsonrpc::Error::invalid_params("Document not found"))?;
        
        let text_up_to_cursor = self.get_text_up_to_cursor(&text, pos);
        let Some((func_name, open_idx)) = self.find_active_function_call(&text_up_to_cursor) else { return Ok(None); };
        let param_idx = self.compute_active_param_index(&text_up_to_cursor[open_idx + 1..]);
        
        let mgr = self.manager.read().expect("Server: lock poisoned").clone();
        if let Some(func) = mgr.get(&format!("${func_name}")) {
            let sig = SignatureInformation {
                label: func.signature_label(),
                documentation: Some(Documentation::String(func.description.clone())),
                parameters: Some(self.build_signature_help_parameters(&func)),
                active_parameter: Some(param_idx),
            };
            spawn_log(self.client.clone(), MessageType::LOG, format!("[PERF] signature_help: ${func_name} in {elapsed:?}", elapsed = start.elapsed()));
            return Ok(Some(SignatureHelp { signatures: vec![sig], active_signature: Some(0), active_parameter: Some(param_idx) }));
        }
        Ok(None)
    }

    async fn semantic_tokens_full(&self, params: SemanticTokensParams) -> Result<Option<SemanticTokensResult>> {
        let start = std::time::Instant::now();
        let text = self.documents.read().expect("Server: lock poisoned").get(&params.text_document.uri).cloned().ok_or(tower_lsp::jsonrpc::Error::invalid_params("Document not found"))?;
        let use_colors = *self.multiple_function_colors.read().expect("Server: lock poisoned");
        let mgr = self.manager.read().expect("Server: lock poisoned").clone();
        let tokens = extract_semantic_tokens_with_colors(&text, use_colors, &mgr);
        spawn_log(self.client.clone(), MessageType::LOG, format!("[PERF] semantic_tokens: {} tokens in {elapsed:?}", tokens.len(), elapsed = start.elapsed()));
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens { result_id: None, data: tokens })))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<serde_json::Value>> {
        if params.command == "forge/cursorMoved" && let Some(args) = params.arguments.get(0) {
            if let Ok(moved) = serde_json::from_value::<CursorMovedParams>(args.clone()) {
                self.cursor_positions.write().expect("Server: lock poisoned").insert(moved.uri.clone(), moved.position);
                self.update_depth(moved.uri.clone()).await;
                
                let should_trigger = (|| {
                    let docs = self.documents.read().expect("Server: lock poisoned");
                    let text = docs.get(&moved.uri)?;
                    let up_to_cursor = self.get_text_up_to_cursor(text, moved.position);
                    let (name, open) = self.find_active_function_call(&up_to_cursor)?;
                    let idx = self.compute_active_param_index(&up_to_cursor[open + 1..]) as usize;
                    let mgr = self.manager.read().expect("Server: lock poisoned").clone();
                    let func = mgr.get(&format!("${name}"))?;
                    let args = func.args.as_ref()?;
                    let arg_idx = if idx >= args.len() && args.last()?.rest { args.len() - 1 } else { idx };
                    let arg = args.get(arg_idx)?;
                    Some(arg.enum_name.is_some() || arg.arg_enum.is_some())
                })().unwrap_or(false);

                if should_trigger { self.client.send_notification::<TriggerCompletionNotification>(moved.uri.clone()).await; }
            }
        }
        Ok(None)
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mgr_outer = self.manager.read().expect("Server: lock poisoned").clone();
        let mgr = mgr_outer.as_ref();
        let config = self.config.read().expect("Server: lock poisoned");
        let ws_folders = self.workspace_folders.read().expect("Server: lock poisoned");
        let rel_path = config.as_ref().and_then(|c| c.custom_functions_path.as_ref());

        for change in params.changes {
            if let Ok(path) = change.uri.to_file_path() {
                let mut is_allowed = false;
                if let Some(rel) = rel_path {
                    for root in ws_folders.iter() { if path.starts_with(root.join(rel)) { is_allowed = true; break; } }
                }
                if !is_allowed { continue; }
                if !matches!(path.extension().and_then(|s| s.to_str()), Some("js") | Some("ts")) { continue; }

                match change.typ {
                    FileChangeType::CREATED | FileChangeType::CHANGED => {
                        if let Ok(count) = mgr.reload_file(path.clone()) {
                            spawn_log(self.client.clone(), MessageType::INFO, format!("[INFO] Reloaded custom functions from {}: {count} found", path.display()));
                        }
                    }
                    FileChangeType::DELETED => {
                        mgr.remove_functions_at_path(&path);
                        spawn_log(self.client.clone(), MessageType::INFO, format!("[INFO] Removed functions from deleted file: {}", path.display()));
                    }
                    _ => {}
                }
            }
        }
    }
}
