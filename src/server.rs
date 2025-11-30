use crate::diagnostics::analyze_and_publish;
use crate::hover::handle_hover;
use crate::metadata::MetadataManager;
use crate::parser::{ForgeScriptParser, ParseResult};
use crate::semantic::extract_semantic_tokens;
use crate::utils::load_forge_config;
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
}

impl ForgeScriptServer {
    /// Parses text and updates the diagnostic cache
    #[tracing::instrument(skip(self, text), fields(uri = %uri, text_len = text.len()))]
    pub async fn process_text(&self, uri: Url, text: String) {
        let start = std::time::Instant::now();
        tracing::debug!("📝 Processing text for {}, {} chars", uri, text.len());
        
        let mgr_arc = self.manager.read().unwrap().clone();
        let parser = ForgeScriptParser::new(mgr_arc, &text);
        
        let parse_start = std::time::Instant::now();
        let parsed = parser.parse();
        tracing::debug!("⏱️  Parsing took {:?}", parse_start.elapsed());

        let cache_start = std::time::Instant::now();
        self.parsed_cache
            .write()
            .unwrap()
            .insert(uri.clone(), parsed.clone());
        tracing::trace!("⏱️  Cache update took {:?}", cache_start.elapsed());

        let diag_start = std::time::Instant::now();
        analyze_and_publish(self, uri.clone(), &text, parsed.diagnostics).await;
        tracing::debug!("⏱️  Diagnostics publishing took {:?}", diag_start.elapsed());
        
        tracing::info!("✅ Processed text for {} in {:?} total", uri, start.elapsed());
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
    #[tracing::instrument(skip(self, params), fields(workspace_folders = ?params.workspace_folders))]
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let start = std::time::Instant::now();
        tracing::info!("🚀 Initializing ForgeScript LSP");
        
        if let Some(folders) = params.workspace_folders {
            let paths = folders
                .into_iter()
                .filter_map(|f| f.uri.to_file_path().ok())
                .collect::<Vec<_>>();
            tracing::info!("📂 Workspace folders: {:?}", paths);
            *self.workspace_folders.write().unwrap() = paths.clone();

            if let Some(urls) = load_forge_config(&paths) {
                tracing::info!("📝 Loading metadata from forgeconfig.json");
                let mgr_start = std::time::Instant::now();
                
                let manager = MetadataManager::new("./.cache", urls)
                    .await
                    .expect("Failed to initialize metadata manager");
                manager
                    .load_all()
                    .await
                    .expect("Failed to load metadata sources");

                tracing::info!("⏱️  Metadata reload took {:?}", mgr_start.elapsed());
                *self.manager.write().unwrap() = Arc::new(manager);
            }
        }

        tracing::info!("✅ LSP initialization completed in {:?}", start.elapsed());
        
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
                                    SemanticTokenType::FUNCTION,
                                    SemanticTokenType::STRING,
                                    SemanticTokenType::KEYWORD,
                                    SemanticTokenType::NUMBER,
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
        tracing::info!("✅ LSP server initialized callback");
        let count = self.function_count();
        tracing::info!("📊 Function count: {}", count);
        
        self.client
            .log_message(
                MessageType::INFO,
                format!("ForgeScript LSP initialized! Loaded {} functions.", count),
            )
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    #[tracing::instrument(skip(self, params), fields(uri = %params.text_document.uri, text_len = params.text_document.text.len()))]
    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let start = std::time::Instant::now();
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        
        tracing::info!("📄 Document opened: {} ({} chars)", uri, text.len());

        self.documents
            .write()
            .unwrap()
            .insert(uri.clone(), text.clone());
        
        self.process_text(uri.clone(), text).await;
        tracing::debug!("⏱️  did_open completed in {:?}", start.elapsed());
    }

    #[tracing::instrument(skip(self, params), fields(uri = %params.text_document.uri))]
    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let start = std::time::Instant::now();
        
        if let Some(change) = params.content_changes.into_iter().next() {
            let uri = params.text_document.uri;
            let text = change.text;

            tracing::debug!("🔄 Document changed: {} ({} chars)", uri, text.len());

            self.documents
                .write()
                .unwrap()
                .insert(uri.clone(), text.clone());
            
            self.process_text(uri, text).await;
            tracing::debug!("⏱️  did_change completed in {:?}", start.elapsed());
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        handle_hover(self, params).await
    }

    #[tracing::instrument(skip(self, params), fields(uri = %params.text_document_position.text_document.uri, position = ?params.text_document_position.position))]
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        
        tracing::debug!("🔍 Completion request at {:?}", position);
        
        let docs = self.documents.read().unwrap();
        let text = docs.get(&uri);

        if let Some(text) = text {
            let lines: Vec<&str> = text.lines().collect();
            let line = lines.get(position.line as usize).unwrap_or(&"");
            let before_cursor = &line[..position.character as usize];

            if before_cursor.ends_with('$') || before_cursor.contains('$') {
                tracing::trace!("  Generating completion items");
                let items: Vec<CompletionItem> = self
                    .all_functions()
                    .into_iter()
                    .map(|f| CompletionItem {
                        label: f.name.clone(),
                        kind: Some(CompletionItemKind::FUNCTION),
                        detail: Some(f.category.clone()),
                        documentation: Some(Documentation::String(f.description.clone())),
                        insert_text: Some(f.name.clone()),
                        ..Default::default()
                    })
                    .collect();

                tracing::info!("✅ Completion returned {} items in {:?}", items.len(), start.elapsed());
                return Ok(Some(CompletionResponse::Array(items)));
            }
        }

        tracing::debug!("❌ No completion items in {:?}", start.elapsed());
        Ok(None)
    }

    #[tracing::instrument(skip(self, params), fields(uri = %params.text_document_position_params.text_document.uri, position = ?params.text_document_position_params.position))]
    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let start = std::time::Instant::now();
        use regex::Regex;

        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        tracing::debug!("✍️  Signature help requested at {:?}", position);

        let docs = self.documents.read().unwrap();
        let Some(text) = docs.get(&uri) else {
            tracing::warn!("❌ No text found for URI: {}", uri);
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

        // --- Scan backwards to find the nearest unmatched '[' (innermost open) ---
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
            tracing::debug!("❌ No unmatched '[' found");
            return Ok(None);
        };

        // --- Extract the function name before the '[' ---
        let before_bracket = &text_up_to_cursor[..open_index];
        let func_re = Regex::new(r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*$").unwrap();

        let Some(caps) = func_re.captures(before_bracket) else {
            tracing::debug!("❌ No function pattern found before '['");
            return Ok(None);
        };

        let func_name = caps.get(1).unwrap().as_str();
        tracing::debug!("  Found open function: ${}", func_name);

        // --- Compute active parameter index by scanning forward from '[' to cursor ---
        // Count top-level separators (',' or ';') while ignoring nested brackets and quoted strings.
        let start_scan = open_index + 1;
        let sub = &text_up_to_cursor[start_scan..];
        let mut param_index: u32 = 0;
        let mut local_depth: i32 = 0;
        let mut in_single = false;
        let mut in_double = false;
        let mut prev_escape = false;

        for ch in sub.chars() {
            if prev_escape {
                // previous char was backslash, this char is escaped — skip special handling
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

            // ignore separators inside quotes
            if in_single || in_double {
                continue;
            }

            match ch {
                '[' => local_depth += 1,
                ']' => {
                    if local_depth > 0 {
                        local_depth -= 1;
                    } else {
                        // This closes the original '[' we found — stop scanning
                        break;
                    }
                }
                // treat both ',' and ';' as top-level parameter separators in ForgeScript
                ',' | ';' if local_depth == 0 => {
                    param_index = param_index.saturating_add(1);
                }
                _ => {}
            }
        }

        tracing::debug!("  Active parameter index: {}", param_index);

        // --- Look up metadata and return signature help with active parameter ---
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

            tracing::info!("✅ Signature help for ${} returned in {:?}", func_name, start.elapsed());
            return Ok(Some(SignatureHelp {
                signatures: vec![signature],
                active_signature: Some(0),
                active_parameter: Some(param_index),
            }));
        }

        tracing::warn!("❌ No metadata found for ${} (took {:?})", func_name, start.elapsed());
        Ok(None)
    }

    #[tracing::instrument(skip(self, params), fields(uri = %params.text_document.uri))]
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let start = std::time::Instant::now();
        let uri = params.text_document.uri;
        tracing::debug!("🎨 Semantic tokens request for {}", uri);
        
        let docs = self.documents.read().unwrap();

        let Some(text) = docs.get(&uri) else {
            tracing::warn!("❌ No text found for semantic tokens");
            return Ok(None);
        };
        
        let tokens = extract_semantic_tokens(text);

        tracing::info!("✅ Semantic tokens returned {} tokens in {:?}", tokens.len(), start.elapsed());
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }
}
