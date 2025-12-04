# ForgeLSP

<div align="center">

![Rust](https://img.shields.io/badge/Rust-2024-orange?style=for-the-badge&logo=rust)
![Tower LSP](https://img.shields.io/badge/Tower--LSP-0.20-purple?style=for-the-badge)
![Tokio](https://img.shields.io/badge/Tokio-Async-blue?style=for-the-badge)
![GitHub License](https://img.shields.io/github/license/Zack-911/forgelsp)

**High-performance Language Server Protocol implementation for ForgeScript**

[![wakatime](https://wakatime.com/badge/user/50d24838-6599-44fc-9e61-1794cf26b2b9/project/8668e791-ed92-44ff-a710-f5f32578753e.svg)](https://wakatime.com/badge/user/50d24838-6599-44fc-9e61-1794cf26b2b9/project/8668e791-ed92-44ff-a710-f5f32578753e)

</div>

---

## ✨ Features

### 🔍 Language Intelligence
- **Hover Documentation**: Rich markdown tooltips with function signatures, descriptions, and examples
- **Auto-Completion**: Context-aware suggestions triggered by `$` or `.`
- **Signature Help**: Real-time parameter hints with active parameter highlighting
- **Semantic Tokens**: Syntax highlighting for functions, strings, numbers, and keywords
- **Diagnostics**: Real-time error detection with helpful messages

### ⚡ Performance
- **Trie-based Lookup**: O(k) function name matching for instant completions
- **Async I/O**: Non-blocking operations using Tokio runtime
- **Smart Caching**: Metadata caching with network fallback
- **Incremental Parsing**: Efficient document re-parsing on changes

### 🌐 Flexible Configuration
- **GitHub Shorthand**: Use `github:owner/repo#branch` syntax for metadata URLs
- **Multi-Source Support**: Load function metadata from multiple URLs
- **Workspace Config**: Configure per-project via `forgeconfig.json`

---

## 📁 Project Structure

```
forgelsp/
├── src/
│   ├── main.rs          # Entry point, LSP service initialization
│   ├── server.rs        # LanguageServer trait implementation
│   ├── hover.rs         # Hover provider logic
│   ├── parser.rs        # ForgeScript parser with diagnostics
│   ├── metadata.rs      # Metadata fetching, caching, and Trie
│   ├── diagnostics.rs   # Diagnostic publishing utilities
│   ├── semantic.rs      # Semantic token extraction
│   └── utils.rs         # Helper functions and config loading
├── .github/
│   └── workflows/       # CI/CD pipelines
├── Cargo.toml           # Rust dependencies
└── Cargo.lock           # Dependency lockfile
```

---

## 📦 Source Files

### `main.rs`
Entry point for the LSP server. Initializes the metadata manager, loads configuration from `forgeconfig.json`, and starts the Tower LSP service over stdio.

### `server.rs`
Implements the `LanguageServer` trait from Tower LSP:
- **initialize**: Registers capabilities (hover, completion, signature help, semantic tokens)
- **did_open/did_change**: Processes document changes and triggers diagnostics
- **hover**: Delegates to hover handler
- **completion**: Returns function suggestions based on cursor context
- **signature_help**: Provides parameter hints for function calls
- **semantic_tokens_full**: Returns semantic highlighting data

### `hover.rs`
Handles hover requests by:
1. Locating the token under the cursor
2. Looking up function metadata in the Trie
3. Generating markdown documentation with signatures, descriptions, and examples

### `parser.rs`
Custom ForgeScript parser that:
- Tokenizes `$functionName[args]` syntax
- Supports nested function calls
- Validates argument counts against metadata
- Reports diagnostics for unknown functions and syntax errors

### `metadata.rs`
Manages function metadata with three key components:

| Component | Purpose |
|-----------|---------|
| **Fetcher** | HTTP client with file-based caching |
| **FunctionTrie** | Prefix-tree for O(k) function lookup |
| **MetadataManager** | Coordinates fetching and indexing |

### `diagnostics.rs`
Converts parser diagnostics to LSP format and publishes them to the client.

### `semantic.rs`
Extracts semantic tokens from code blocks using regex patterns:
- Functions (`$name`)
- Strings (single/double quoted)
- Numbers (integers/floats)
- Keywords (`true`, `false`)

### `utils.rs`
Utility functions:
- `load_forge_config`: Parses `forgeconfig.json`
- `resolve_github_shorthand`: Expands `github:owner/repo` to raw URLs
- `spawn_log`: Async logging helper

---

## 🔧 Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `tower-lsp` | 0.20.0 | LSP framework |
| `tokio` | 1.x | Async runtime |
| `reqwest` | 0.12 | HTTP client |
| `serde` | 1.x | Serialization |
| `serde_json` | 1.x | JSON parsing |
| `nom` | 8.0.0 | Parser combinators |
| `regex` | 1.12.2 | Pattern matching |
| `smallvec` | 1.15.1 | Stack-allocated vectors |
| `anyhow` | 1.x | Error handling |
| `base64` | 0.22.1 | URL-safe cache keys |
| `futures` | 0.3.31 | Async utilities |
| `tracing` | 0.1 | Structured logging |

---

## 🚀 Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run directly
cargo run
```

### Release Profile Optimizations

```toml
[profile.release]
opt-level = 3      # Maximum optimization
lto = "fat"        # Link-time optimization
codegen-units = 1  # Single codegen unit for better optimization
panic = "abort"    # Smaller binary size
strip = true       # Remove debug symbols
```

---

## 📊 LSP Capabilities

| Capability | Status | Description |
|------------|--------|-------------|
| Text Document Sync | ✅ Full | Complete document sync on each change |
| Hover | ✅ | Function documentation on hover |
| Completion | ✅ | Triggered by `$` and `.` |
| Signature Help | ✅ | Triggered by `$`, `[`, `;` |
| Semantic Tokens | ✅ Full | Full document semantic highlighting |
| Diagnostics | ✅ | Real-time error reporting |

---

## 🔗 Related Projects

- **[ForgeLSP VS Code Extension](../forgevsc)** - The VS Code client for this LSP
- **[ForgeScript](https://github.com/tryforge/forgescript)** - The ForgeScript language

---

## Contributors

- **Striatp** ([@striatp](https://github.com/striatp)): Helping me with the github build scripts to build the lsp for all devices

---

## 📄 License

GPL-3 License - See [LICENSE](LICENSE) for details.
