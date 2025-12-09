use crate::diagnostics::publish_diagnostics;
use crate::hover::handle_hover;
use crate::metadata::MetadataManager;
use crate::parser::{ForgeScriptParser, ParseResult};
use crate::semantic::extract_semantic_tokens_with_colors;
use crate::utils::{load_forge_config_full, spawn_log};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use tower_lsp::{Client, LanguageServer, async_trait, jsonrpc::Result, lsp_types::*};

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
    /// Parses text and updates the diagnostic cache
    pub async fn process_text(&self, uri: Url, text: String) {
        let start = std::time::Instant::now();

        let mgr_arc = self.manager.read().unwrap().clone();
        let parser = ForgeScriptParser::new(mgr_arc, &text);
        let parsed = parser.parse();

        self.parsed_cache
            .write()
            .unwrap()
            .insert(uri.clone(), parsed.clone());

        publish_diagnostics(self, &uri, &text, &parsed.diagnostics).await;

        let diag_count = parsed.diagnostics.len();
        if diag_count > 0 {
            spawn_log(
                self.client.clone(),
                MessageType::WARNING,
                format!(
                    "[WARN] Found {} diagnostics in {:?}",
                    diag_count,
                    start.elapsed()
                ),
            );
        }
    }

    pub fn function_count(&self) -> usize {
        let mgr = self.manager.read().unwrap();
        mgr.function_count()
    }

    pub fn all_functions(&self) -> Vec<Arc<crate::metadata::Function>> {
        let mgr = self.manager.read().unwrap();
        mgr.all_functions()
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
            *self.workspace_folders.write().unwrap() = paths.clone();

            if let Some(config) = load_forge_config_full(&paths) {
                let urls = config.urls.clone();
                let manager = MetadataManager::new("./.cache", urls)
                    .await
                    .expect("Failed to initialize metadata manager");
                manager
                    .load_all()
                    .await
                    .expect("Failed to load metadata sources");

                *self.manager.write().unwrap() = Arc::new(manager);
                
                // Load function color highlighting setting
                if let Some(use_colors) = config.multiple_function_colors {
                    *self.multiple_function_colors.write().unwrap() = use_colors;
                }
            }
        }

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
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
                    work_done_progress_options: Default::default(),
                }),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: WorkDoneProgressOptions {
                                work_done_progress: None,
                            },
                            legend: SemanticTokensLegend {
                                token_types: vec![
                                    SemanticTokenType::FUNCTION,      // 0
                                    SemanticTokenType::KEYWORD,       // 1
                                    SemanticTokenType::NUMBER,        // 2
                                    SemanticTokenType::PARAMETER,     // 3
                                ],
                                token_modifiers: vec![],
                            },
                            range: Some(false),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                        },
                    ),
                ),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let count = self.function_count();

        self.client
            .log_message(
                MessageType::INFO,
                format!("[INFO] ForgeLSP initialized with {} functions", count),
            )
            .await;
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
                "[PERF] did_open: {} chars in {:?}",
                text_len,
                start.elapsed()
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
                .unwrap()
                .insert(uri.clone(), text.clone());

            self.process_text(uri, text).await;

            spawn_log(
                self.client.clone(),
                MessageType::LOG,
                format!(
                    "[PERF] did_change: {} chars in {:?}",
                    text_len,
                    start.elapsed()
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

        let docs = self.documents.read().unwrap();
        let text = docs.get(&uri);

        if let Some(text) = text {
            let lines: Vec<&str> = text.lines().collect();
            let line = lines.get(position.line as usize).unwrap_or(&"");
            let before_cursor = &line[..position.character as usize];

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
                            format!("${}{}", modifier, &base[1..])
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
                        "[PERF] completion: {} items in {:?}",
                        items.len(),
                        start.elapsed()
                    ),
                );
                return Ok(Some(CompletionResponse::Array(items)));
            }
        }

        Ok(None)
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let start = std::time::Instant::now();
        use regex::Regex;

        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().unwrap();
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
                let slice = &line[..position.character.min(line.len() as u32) as usize];
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
        let func_re = Regex::new(r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*$").unwrap();

        let Some(caps) = func_re.captures(before_bracket) else {
            return Ok(None);
        };

        let func_name = caps.get(1).unwrap().as_str();

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
        let mgr = self.manager.read().unwrap().clone();
        let lookup = format!("${}", func_name);

        if let Some(func) = mgr.get(&lookup) {
            let args = func.args.clone().unwrap_or_default();
            let params: Vec<ParameterInformation> = args
                .iter()
                .map(|a| ParameterInformation {
                    label: ParameterLabel::Simple(a.name.clone()),
                    documentation: Some(Documentation::String(a.description.clone())),
                })
                .collect();

            let sig_label = if func.brackets == Some(true) {
                format!(
                    "{}[{}]",
                    func.name,
                    args.iter()
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>()
                        .join("; ")
                )
            } else {
                format!(
                    "${} {}",
                    func.name,
                    args.iter()
                        .map(|a| a.name.clone())
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            };

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
                    "[PERF] signature_help: ${} in {:?}",
                    func_name,
                    start.elapsed()
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

        let docs = self.documents.read().unwrap();

        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };

        let use_colors = *self.multiple_function_colors.read().unwrap();
        let tokens = extract_semantic_tokens_with_colors(text, use_colors);

        spawn_log(
            self.client.clone(),
            MessageType::LOG,
            format!(
                "[PERF] semantic_tokens: {} tokens in {:?}",
                tokens.len(),
                start.elapsed()
            ),
        );

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }
}
