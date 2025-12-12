# main.rs

## Overview

The `main.rs` file serves as the entry point for the ForgeLSP server. It handles initialization of the metadata manager, configuration loading, and LSP service setup.

## Main Function Flow

The `main` function is marked with `#[tokio::main]` to run asynchronously using the Tokio runtime.

### Initialization Sequence

#### 1. Workspace Detection

```rust
let workspace_folders = vec![std::env::current_dir().unwrap()];
```

Initially sets workspace to the current directory. This is later updated during LSP `initialize` if the client provides workspace folders.

#### 2. Configuration Loading

```rust
let fetch_urls = load_forge_config(&workspace_folders).unwrap_or_else(|| {
    vec!["https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"]
        .into_iter()
        .map(String::from)
        .collect()
});
```

**Fallback Strategy:**
- Attempts to load `forgeconfig.json` from workspace folders
- If not found, defaults to ForgeScript's dev branch metadata URL
- This ensures the LSP works out-of-the-box without configuration

#### 3. Metadata Manager Initialization

```rust
let manager = Arc::new(
    MetadataManager::new("./.cache", fetch_urls)
        .await
        .expect("Failed to initialize metadata manager"),
);
```

**Key Points:**
- Wrapped in `Arc` for shared ownership across async tasks
- Cache directory: `./.cache` (relative to workspace)
- Asynchronously initializes HTTP client and cache directory

#### 4. Metadata Loading

```rust
manager
    .load_all()
    .await
    .expect("Failed to load metadata sources");
```

Fetches function metadata from all configured URLs (with caching fallback).

#### 5. Custom Functions Integration

```rust
if let Some(config) = load_forge_config_full(&workspace_folders) {
    if let Some(custom_funcs) = config.custom_functions {
        if !custom_funcs.is_empty() {
            manager
                .add_custom_functions(custom_funcs)
                .expect("Failed to add custom functions");
        }
    }
}
```

Loads user-defined custom functions from `forgeconfig.json` if available.

#### 6. Thread-Safe Wrapping

```rust
let manager_wrapped = Arc::new(RwLock::new(manager));
```

Wraps the manager in `RwLock` to allow dynamic updates during LSP operation (e.g., when workspace configuration changes).

#### 7. LSP Server Initialization

```rust
let (service, socket) = LspService::new(|client| ForgeScriptServer {
    client,
    manager: manager_wrapped.clone(),
    documents: Arc::new(RwLock::new(HashMap::new())),
    parsed_cache: Arc::new(RwLock::new(HashMap::new())),
    workspace_folders: Arc::new(RwLock::new(workspace_folders.clone())),
    multiple_function_colors: Arc::new(RwLock::new(true)),
});
```

**Server State:**
- `client` - LSP client for sending notifications/requests
- `manager` - Metadata manager with function definitions
- `documents` - Cache of opened document contents
- `parsed_cache` - Cached parse results for performance
- `workspace_folders` - Active workspace paths
- `multiple_function_colors` - Semantic highlighting preference

#### 8. Start LSP Server

```rust
Server::new(stdin(), stdout(), socket).serve(service).await;
```

Binds the LSP server to stdio channels and starts serving requests.

## Module Imports

```rust
mod diagnostics;    // Diagnostic publishing
mod hover;          // Hover provider
mod metadata;       // Function metadata management
mod parser;         // ForgeScript parser
mod semantic;       // Semantic token extraction
mod server;         // LSP server implementation
mod utils;          // Helper functions
```

## Dependencies

### Tower LSP
- `LspService` - LSP service builder
- `Server` - LSP protocol handler

### Tokio
- `stdin()` / `stdout()` - Async I/O for LSP communication
- `#[tokio::main]` - Async runtime macro

### Standard Library
- `Arc` - Thread-safe reference counting
- `RwLock` - Read-write lock for concurrent access
- `HashMap` - Document and parse result caches

## Configuration File: `forgeconfig.json`

Example configuration:

```json
{
  "urls": [
    "https://raw.githubusercontent.com/tryforge/forgescript/main/metadata/functions.json",
    "github:myorg/custom-functions#main"
  ],
  "multiple_function_colors": true,
  "custom_functions": [
    {
      "name": "$myCustomFunc",
      "description": "My custom function",
      "params": [
        {
          "name": "arg1",
          "type": "String",
          "required": true
        }
      ]
    }
  ]
}
```

## Error Handling

The initialization uses `.expect()` for critical failures:
- Metadata manager creation failure → Server cannot function
- Metadata loading failure → No function definitions available
- Custom function parsing errors → Invalid configuration

Future improvements could provide more graceful degradation.

## Why This Architecture?

1. **Async-First:** Tokio enables non-blocking I/O for LSP communication
2. **Shared State:** `Arc<RwLock<>>` allows safe concurrent access from multiple LSP handlers
3. **Lazy Loading:** Documents parsed on-demand and cached
4. **Flexible Config:** Supports both default and custom function sources
5. **Modular:** Clean separation of concerns across modules
