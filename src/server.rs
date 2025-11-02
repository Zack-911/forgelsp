use crate::diagnostics::analyze_and_publish;
use crate::hover::handle_hover;
use crate::metadata::MetadataManager;
use crate::parser::{ForgeScriptParser, ParseResult};
use crate::semantic::extract_semantic_tokens;
use crate::utils::load_forge_config;
use regex::Regex;
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
    pub async fn process_text(&self, uri: Url, text: String) {
        let mgr_arc = self.manager.read().unwrap().clone();
        let parser = ForgeScriptParser::new(mgr_arc, &text);
        let parsed = parser.parse();

        self.parsed_cache
            .write()
            .unwrap()
            .insert(uri.clone(), parsed.clone());

        analyze_and_publish(self, uri, &text, parsed.diagnostics).await;
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

            if let Some(urls) = load_forge_config(&paths) {
                let manager = MetadataManager::new("./.cache", urls)
                    .await
                    .expect("Failed to initialize metadata manager");
                manager
                    .load_all()
                    .await
                    .expect("Failed to load metadata sources");

                *self.manager.write().unwrap() = Arc::new(manager);
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
        let count = self.function_count();
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

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;

        self.documents
            .write()
            .unwrap()
            .insert(uri.clone(), text.clone());
        self.process_text(uri, text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        if let Some(change) = params.content_changes.into_iter().next() {
            let uri = params.text_document.uri;
            let text = change.text;

            self.documents
                .write()
                .unwrap()
                .insert(uri.clone(), text.clone());
            self.process_text(uri, text).await;
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        handle_hover(self, params).await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let docs = self.documents.read().unwrap();
        let text = docs.get(&uri);

        if let Some(text) = text {
            let lines: Vec<&str> = text.lines().collect();
            let line = lines.get(position.line as usize).unwrap_or(&"");
            let before_cursor = &line[..position.character as usize];

            if before_cursor.ends_with('$') || before_cursor.contains('$') {
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

                return Ok(Some(CompletionResponse::Array(items)));
            }
        }

        Ok(None)
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        use regex::Regex;

        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;
        eprintln!(
            "[LSP] Signature help requested for {} at {:?}",
            uri, position
        );

        let docs = self.documents.read().unwrap();
        let Some(text) = docs.get(&uri) else {
            eprintln!("[LSP] No text found for URI: {}", uri);
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
            eprintln!("[LSP] No unmatched '[' found.");
            return Ok(None);
        };

        // --- Extract the function name before the '[' ---
        let before_bracket = &text_up_to_cursor[..open_index];
        let func_re = Regex::new(r"\$([a-zA-Z_][a-zA-Z0-9_]*)\s*$").unwrap();

        let Some(caps) = func_re.captures(before_bracket) else {
            eprintln!("[LSP] No function pattern found before '['.");
            return Ok(None);
        };

        let func_name = caps.get(1).unwrap().as_str();
        eprintln!("🔍 Found open function: ${}", func_name);

        // --- Compute active parameter index by scanning forward from '[' to cursor ---
        // Count top-level separators (',' or ';') while ignoring nested brackets and quoted strings.
        let start = open_index + 1;
        let sub = &text_up_to_cursor[start..];
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

        eprintln!("[LSP] Active parameter index: {}", param_index);

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

            return Ok(Some(SignatureHelp {
                signatures: vec![signature],
                active_signature: Some(0),
                active_parameter: Some(param_index),
            }));
        }

        eprintln!("[LSP] No metadata found for {}", func_name);
        Ok(None)
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri;
        let docs = self.documents.read().unwrap();

        let Some(text) = docs.get(&uri) else {
            return Ok(None);
        };
        let tokens = extract_semantic_tokens(text);

        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data: tokens,
        })))
    }
}
