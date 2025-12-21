# server.rs

## Overview

The `server.rs` module implements the `LanguageServer` trait from Tower LSP, providing all the core LSP functionality for ForgeScript including document synchronization, hover, completion, signature help, and semantic tokens.

## ForgeScriptServer Structure

```rust
pub struct ForgeScriptServer {
    pub client: Client,                                    // LSP client connection
    pub manager: Arc<RwLock<Arc<MetadataManager>>>,       // Function metadata (dynamic reload)
    pub documents: Arc<RwLock<HashMap<Url, String>>>,     // Document content cache
    pub parsed_cache: Arc<RwLock<HashMap<Url, ParseResult>>>, // Parse result cache
    pub workspace_folders: Arc<RwLock<Vec<PathBuf>>>,     // Active workspaces
    pub multiple_function_colors: Arc<RwLock<bool>>,      // Semantic highlighting config
}
```

**Thread Safety:**
- All shared state wrapped in `Arc<RwLock<>>` for concurrent access
- Multiple LSP handlers can read simultaneously
- Writes block all other access

## LSP Lifecycle

### `initialize`

Called once when client connects to the server.

**Responsibilities:**
1. Process workspace folders from client
2. Load workspace-specific `forgeconfig.json`
3. Reload metadata with new configuration
4. Apply custom functions and settings
5. Return server capabilities

**Capabilities Registration:**

```rust
ServerCapabilities {
    text_document_sync: TextDocumentSyncKind::FULL,     // Full document sync
    hover_provider: Some(true),                          // Hover support
    completion_provider: Some(CompletionOptions {
        trigger_characters: vec!["$", "."],              // Trigger on $ and .
        ...
    }),
    signature_help_provider: Some(SignatureHelpOptions {
        trigger_characters: vec!["$", "[", ";", ",", " "],
        retrigger_characters: vec![",", " "],
        ...
    }),
    semantic_tokens_provider: SemanticTokensOptions {
        ...
    },
    workspace: Some(WorkspaceServerCapabilities {
        workspace_folders: Some(WorkspaceFoldersServerCapabilities {
            supported: Some(true),
            change_notifications: Some(OneOf::Left(true)),
        }),
    }),
    ...
}
```

**Dynamic Reload:**
- If workspace has `forgeconfig.json`, metadata is reloaded
- Old manager replaced with new one containing workspace config
- Custom functions added if specified

### `initialized`

Called after `initialize` completes.

```rust
async fn initialized(&self, _: InitializedParams) {
    let count = self.function_count();
    self.client.log_message(
        MessageType::INFO,
        format!("[INFO] ForgeLSP initialized with {} functions", count)
    ).await;

    // Register global file watcher for .js and .ts files
    self.client.register_capability(vec![Registration {
        id: "watch-custom-functions".to_string(),
        method: "workspace/didChangeWatchedFiles".to_string(),
        register_options: Some(json!({
            "watchers": [{
                "globPattern": "**/*.{js,ts}",
                "kind": 7 // Create, Change, Delete
            }]
        })),
    }]).await;
}
```

Logs successful initialization with function count.

### `shutdown`

Called when client disconnects:

```rust
async fn shutdown(&self) -> Result<()> {
    spawn_log(
        self.client.clone(),
        MessageType::INFO,
        "[INFO] ForgeLSP shutting down".to_string()
    );
    Ok(())
}
```

## Document Synchronization

### `did_open`

Called when a document is first opened:

```rust
async fn did_open(&self, params: DidOpenTextDocumentParams) {
    let uri = params.text_document.uri;
    let text = params.text_document.text;
    
    // Cache document content
    self.documents.write().unwrap().insert(uri.clone(), text.clone());
    
    // Parse and publish diagnostics
    self.process_text(uri, text).await;
    
    // Log performance
    spawn_log(client, LOG, format!("[PERF] did_open: {} chars in {:?}", ...));
}
```

### `did_change`

Called when document content changes:

```rust
async fn did_change(&self, params: DidChangeTextDocumentParams) {
    // ... update cache and reparse ...
}

### `did_change_watched_files`

Handles external file changes for custom functions:

```rust
async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
    for change in params.changes {
        let path = change.uri.to_file_path()?;
        match change.typ {
            CREATED | CHANGED => {
                manager.reload_file(path).await?;
            }
            DELETED => {
                manager.remove_functions_at_path(&path);
            }
        }
    }
}
```

This ensures that adding, removing, or modifying `.js`/`.ts` files in the custom functions directory immediately updates the LSP's metadata without requiring a restart.

**Full Sync Mode:**
- Client sends entire document on each change
- No incremental updates needed
- Simplifies implementation

### `process_text`

Core document processing:

```rust
pub async fn process_text(&self, uri: Url, text: String) {
    let mgr_arc = self.manager.read().unwrap().clone();
    let parser = ForgeScriptParser::new(mgr_arc, &text);
    let parsed = parser.parse();
    
    // Cache parse result
    self.parsed_cache.write().unwrap().insert(uri.clone(), parsed.clone());
    
    // Publish diagnostics
    publish_diagnostics(self, &uri, &text, &parsed.diagnostics).await;
}
```

## Language Features

### Hover

Delegates to `hover.rs`:

```rust
async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
    handle_hover(self, params).await
}
```

See [hover.md](./hover.md) for details.

### Completion

Context-aware function suggestions:

```rust
async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
    let position = params.text_document_position.position;
    
    // Get text before cursor
    let line = lines.get(position.line as usize)?;
    let before_cursor = &line[..position.character as usize];
    
    // Find last $
    if let Some(last_dollar_idx) = before_cursor.rfind('$') {
        let after_dollar = &before_cursor[last_dollar_idx + 1..];
        let mut modifier = "";
        
        // Detect modifier
        if after_dollar.starts_with('!') {
            modifier = "!";
        } else if after_dollar.starts_with('.') {
            modifier = ".";
        }
        
        // Generate completion items
        let items: Vec<CompletionItem> = self.all_functions()
            .into_iter()
            .map(|f| {
                let name = if !modifier.is_empty() {
                    format!("${}{}", modifier, &f.name[1..])  // Add modifier
                } else {
                    f.name.clone()
                };
                
                CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::FUNCTION),
                    detail: Some(f.category.clone()),
                    documentation: Some(Documentation::String(f.description.clone())),
                    insert_text: Some(name),
                    filter_text: Some(f.name),  // Filter without modifier
                    ...
                }
            })
            .collect();
    }
}
```

**Modifier Support:**
- `$!` → Silent execution modifier
- `$.` → Property access modifier
- Completion shows `$!ban`, `$.property`
- Filtering done on base name without modifier

**Example:**
- User types: `$!b`
- Shows: `$!ban`, `$!button`, etc.
- Inserts: `$!ban` (with modifier)
- Filters on: `$ban` (without modifier)

### Signature Help

Real-time parameter hints:

**Algorithm:**
1. **Find Context:** Scan backward to find unmatched `[`
2. **Extract Function Name:** Regex match `$functionName` before `[`
3. **Calculate Parameter Index:** Count `;` and `,` separators at depth 0
4. **Lookup Metadata:** Get function details from manager
5. **Build Signature:** Format with parameters and active index

**Bracket Depth Tracking:**

```rust
let mut depth = 0i32;
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
```

Finds the opening bracket of the current function call.

**Parameter Index Calculation:**

```rust
let mut param_index: u32 = 0;
let mut local_depth: i32 = 0;

for ch in sub.chars() {
    match ch {
        '[' => local_depth += 1,
        ']' => local_depth = local_depth.saturating_sub(1),
        ',' | ';' if local_depth == 0 => {
            param_index = param_index.saturating_add(1);
        }
        _ => {}
    }
}
```

Counts separators only at depth 0 (not inside nested brackets).

**Quote Handling:**
- Tracks `'` and `"` to avoid counting separators inside strings
- Respects escape sequences with backslash tracking

**Signature Format:**

```rust
if func.brackets == Some(true) {
    format!("{}[{}]", func.name, args.join("; "))
} else {
    format!("{} {}", func.name, args.join(" "))
}
```

### Semantic Tokens

Delegates to `semantic.rs`:

```rust
async fn semantic_tokens_full(&self, params: SemanticTokensParams)
    -> Result<Option<SemanticTokensResult>>
{
    let use_colors = *self.multiple_function_colors.read().unwrap();
    let mgr = self.manager.read().unwrap().clone();
    let tokens = extract_semantic_tokens_with_colors(text, use_colors, mgr);
    
    Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data: tokens,
    })))
}
```

See [semantic.md](./semantic.md) for details.

## Helper Methods

### `function_count`

Returns total number of functions in metadata:

```rust
pub fn function_count(&self) -> usize {
    let mgr = self.manager.read().unwrap();
    mgr.function_count()
}
```

### `all_functions`

Returns all function metadata:

```rust
pub fn all_functions(&self) -> Vec<Arc<Function>> {
    let mgr = self.manager.read().unwrap();
    mgr.all_functions()
}
```

## Performance Logging

All major operations log timing:

```rust
spawn_log(
    self.client.clone(),
    MessageType::LOG,
    format!("[PERF] operation: details in {:?}", elapsed)
);
```

**Logged Operations:**
- Document open/change
- Parsing
- Hover requests
- Completion requests
- Signature help
- Semantic token extraction

**Log Format:**
- `[INFO]` - Initialization, shutdown
- `[WARN]` - Diagnostics found
- `[PERF]` - Performance metrics
- `[LOG]` - Debug information

## Configuration Reloading

When workspace folders change during `initialize`:

```rust
if let Some(folders) = params.workspace_folders {
    *self.workspace_folders.write().unwrap() = paths.clone();
    
    if let Some(config) = load_forge_config_full(&paths) {
        // Create new manager with workspace config
        let manager = MetadataManager::new("./.cache", config.urls).await?;
        manager.load_all().await?;
        
        // Add custom functions
        if let Some(custom_funcs) = config.custom_functions {
            manager.add_custom_functions(custom_funcs)?;
        }
        
        // Replace old manager
        *self.manager.write().unwrap() = Arc::new(manager);
        
        // Apply settings
        if let Some(use_colors) = config.multiple_function_colors {
            *self.multiple_function_colors.write().unwrap() = use_colors;
        }
    }
}
```

This allows workspace-specific configurations without restarting the server.

## Future Enhancements

- **Incremental Sync:** Use `TextDocumentSyncKind::INCREMENTAL`
- **Go to Definition:** Navigate to function source
- **References:** Find all usages of a function
- **Rename:** Rename function across workspace
- **Code Actions:** Quick fixes for common errors
- **Document Formatting:** Auto-format ForgeScript code
