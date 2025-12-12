# ForgeLSP Architecture Guide

## Project Overview

ForgeLSP is a Language Server Protocol (LSP) implementation for ForgeScript, providing intelligent code completion, diagnostics, and semantic highlighting for Discord bot development.

## Architecture Diagram

```
┌─────────────────────────────────────────────────────────┐
│                    LSP Client (IDE)                     │
└─────────────────────┬───────────────────────────────────┘
                      │ JSON-RPC over stdio
┌─────────────────────▼───────────────────────────────────┐
│                  server.rs (LSP Server)                 │
│  ┌──────────────────────────────────────────────────┐  │
│  │ LanguageServer Trait Implementation              │  │
│  │ - initialize/initialized                         │  │
│  │ - did_open/did_change                            │  │
│  │ - hover/completion/signature_help                │  │
│  │ - semantic_tokens_full                           │  │
│  └──────────────────────────────────────────────────┘  │
└──┬────────┬──────────┬──────────┬──────────┬───────────┘
   │        │          │          │          │
   ▼        ▼          ▼          ▼          ▼
┌──────┐ ┌─────┐ ┌──────────┐ ┌────────┐ ┌──────────┐
│hover │ │diag │ │ parser   │ │metadata│ │semantic  │
│.rs   │ │.rs  │ │ .rs      │ │.rs     │ │.rs       │
└──────┘ └─────┘ └──────────┘ └────────┘ └──────────┘
                      │             │
                      │             ▼
                      │      ┌────────────┐
                      │      │ HTTP Cache │
                      │      │ (.cache/)  │
                      ▼      └────────────┘
                 ┌──────────┐
                 │utils.rs  │
                 │(config)  │
                 └──────────┘
```

## Module Responsibilities

### `main.rs` - Entry Point
**Purpose:** Initialize and start the LSP server

**Key Functions:**
- Load configuration from `forgeconfig.json`
- Initialize `MetadataManager` with function definitions
- Create LSP service with shared state
- Bind to stdin/stdout for communication

**State Initialization:**
```rust
let manager = Arc::new(MetadataManager::new(...).await?);
let service = LspService::new(|client| ForgeScriptServer {
    client,
    manager: Arc::new(RwLock::new(manager)),
    documents: Arc::new(RwLock::new(HashMap::new())),
    parsed_cache: Arc::new(RwLock::new(HashMap::new())),
    // ...
});
```

### `server.rs` - LSP Implementation
**Purpose:** Implement `LanguageServer` trait

**Responsibilities:**
- Handle LSP lifecycle (initialize, initialized, shutdown)
- Manage document synchronization (did_open, did_change)
- Provide language features (hover, completion, signature help)
- Generate semantic tokens for highlighting

**Shared State:**
- `documents`: Document content cache
- `parsed_cache`: Parse result cache
- `manager`: Function metadata (dynamic reload)
- `workspace_folders`: Active workspace paths
- `multiple_function_colors`: Highlighting config

**Threading Model:**
- All state wrapped in `Arc<RwLock<>>`
- Multiple readers, single writer pattern
- LSP handlers can execute concurrently

### `parser.rs` - ForgeScript Parser
**Purpose:** Parse ForgeScript syntax into structured data

**Key Components:**
- `ForgeScriptParser`: Main parser struct
- `ParseResult`: Output (tokens, diagnostics, functions)
- `ParsedFunction`: Parsed function call with metadata
- `Token`: Tokenized code segment

**Parsing Flow:**
1. Extract `code:` blocks from source
2. Tokenize with escape sequence handling
3. Detect function calls (`$functionName[args]`)
4. Parse arguments (semicolon-separated, nested)
5. Validate against metadata
6. Generate diagnostics for errors

**Special Handling:**
- Escape sequences: `\`` (backtick), `\\$` (dollar), `\\;` (semicolon)
- Escape functions: `$esc[...]`, `$escapeCode[...]` (literal content)
- JavaScript expressions: `${...}`
- Nested brackets with depth tracking

### `metadata.rs` - Function Metadata
**Purpose:** Manage ForgeScript function definitions

**Architecture:**
```
MetadataManager
├── Fetcher (HTTP + file cache)
│   ├── Download from URLs
│   └── Base64-encoded cache files
├── FunctionTrie (O(k) lookup)
│   ├── Prefix tree structure
│   └── Alias support
└── fetch_urls (configured sources)
```

**Components:**

**Fetcher:**
- HTTP client with caching
- Cache key: base64(URL)
- Cached data: JSON response

**FunctionTrie:**
- Trie for efficient prefix matching
- O(k) lookup where k = key length
- Supports aliases as separate entries

**MetadataManager:**
- Orchestrates fetching and indexing
- Thread-safe via `Arc<RwLock<>>`
- Supports custom user functions

### `semantic.rs` - Syntax Highlighting
**Purpose:** Extract semantic tokens for LSP

**Token Types:**
```rust
0 = FUNCTION    // $functionName
1 = KEYWORD     // true, false, ;
2 = NUMBER      // 123, 45.67
3 = PARAMETER   // Alternate function color
4 = STRING      // Escape function content
5 = COMMENT     // $c[...]
```

**Features:**
- Metadata-validated function highlighting
- Multi-color function support (alternating colors)
- Special handling for comments and escape functions
- Relative delta encoding for LSP

### `hover.rs` - Hover Provider
**Purpose:** Show function documentation on hover

**Process:**
1. Detect token under cursor
2. Handle escape characters
3. Lookup function metadata
4. Format as Markdown
5. Return `Hover` response

**Markdown Format:**
```markdown
# $functionName

Description of function

**Arguments:**
- arg1 (Type): Description
- arg2 (Type, optional): Description

**Category:** category_name

**Examples:**
```forgescript
$example[...]
```
```

### `diagnostics.rs` - Error Reporting
**Purpose:** Convert parser diagnostics to LSP format

**Process:**
1. Receive parser diagnostics (byte offsets)
2. Convert to line/character positions
3. Format as LSP `Diagnostic`
4. Publish to client

### `utils.rs` - Utilities
**Purpose:** Helper functions and configuration

**Key Functions:**
- `load_forge_config()`: Load configuration
- `resolve_github_shorthand()`: Expand GitHub URLs
- `spawn_log()`: Async logging helper

**GitHub Shorthand:**
```
github:owner/repo#branch → https://raw.githubusercontent.com/owner/repo/branch/metadata/functions.json
```

## Data Flow

### Document Change Flow
```
User edits file
    ↓
IDE sends did_change notification
    ↓
Server receives notification
    ↓
Update documents cache
    ↓
Parse with ForgeScriptParser
    ↓
Cache ParseResult
    ↓
Publish diagnostics to client
    ↓
IDE shows errors/warnings
```

### Hover Request Flow
```
User hovers over $functionName
    ↓
IDE sends hover request
    ↓
Server identifies token at position
    ↓
Lookup function in MetadataManager
    ↓
Format documentation as Markdown
    ↓
Return Hover response
    ↓
IDE displays tooltip
```

### Completion Flow
```
User types $
    ↓
IDE sends completion request
    ↓
Server gets all function names
    ↓
Apply modifier if present ($!, $.)
    ↓
Generate CompletionItem list
    ↓
Return to IDE
    ↓
IDE shows suggestion list
```

## Configuration

### `forgeconfig.json`
```json
{
  "urls": [
    "github:tryforge/forgescript#main",
    "https://example.com/custom-functions.json"
  ],
  "multiple_function_colors": true,
  "custom_functions": [
    {
      "name": "$customFunc",
      "description": "Custom function",
      "params": ["arg1", "arg2"]
    }
  ]
}
```

**Search Order:**
1. Check each workspace folder for `forgeconfig.json`
2. Return first found
3. Fallback to default URL if not found

## Threading & Concurrency

### Shared State Pattern
```rust
Arc<RwLock<T>>
```

**Why:**
- `Arc`: Share ownership across async tasks
- `RwLock`: Multiple readers OR one writer
- Safe concurrent access to state

### Document Cache
```rust
Arc<RwLock<HashMap<Url, String>>>
```
- Read: LSP handlers
- Write: did_open, did_change

### Metadata Manager
```rust
Arc<RwLock<Arc<MetadataManager>>>
```
- Read: All LSP features
- Write: initialize (workspace config reload)

## Performance Considerations

1. **Caching:**
   - HTTP responses cached to disk
   - Parse results cached per document
   - Metadata shared via Arc

2. **Lazy Parsing:**
   - Only parse on document changes
   - Not on every LSP request

3. **Trie Lookup:**
   - O(k) function lookup
   - Faster than HashMap for prefix matching

4. **Async Operations:**
   - Non-blocking I/O for HTTP
   - Fire-and-forget logging

## Extension Points

### Adding New LSP Features
1. Add handler in `server.rs`
2. Register capability in `initialize()`
3. Implement logic using existing modules

### Supporting New Metadata Sources
1. Add URL to `forgeconfig.json`
2. Ensure JSON format matches `Function` struct
3. MetadataManager handles the rest

### Custom Functions
1. Add to `custom_functions` in config
2. Converted to standard `Function` format
3. Inserted into FunctionTrie

## Testing Strategy

### Unit Tests
- Parser: Escape sequences, bracket matching
- Metadata: Trie operations, caching
- Utilities: GitHub URL expansion

### Integration Tests
- LSP protocol compliance
- End-to-end workflows

### Manual Testing
- Real IDE integration
- ForgeScript example files
