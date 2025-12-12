# ForgeLSP Coding Style Guide

## Rust Edition

**Edition:** 2024
- Use modern Rust 2024 features (let chains, inline format strings, etc.)
- Keep up-to-date with stable Rust releases

## Code Formatting

### Automated Formatting
Always run `cargo fmt` before committing:
```bash
cargo fmt
```

### Import Organization
```rust
// 1. Standard library
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

// 2. External crates (alphabetical)
use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use tower_lsp::{Client, LanguageServer};

// 3. Internal modules (alphabetical)
use crate::diagnostics::publish_diagnostics;
use crate::metadata::MetadataManager;
use crate::parser::ForgeScriptParser;
```

**✅ DO:**
- Group imports by category
- Use explicit imports over wildcards (except for LSP types where needed)
- Alphabetize within groups

**❌ DON'T:**
- Use wildcard imports (`use module::*`) unless absolutely necessary
- Place imports inside functions

## Naming Conventions

### Variables & Functions
```rust
// Snake case for variables and functions
let metadata_manager = MetadataManager::new(...);
pub fn load_forge_config() -> Option<ForgeConfig> { ... }
```

### Types & Traits
```rust
// PascalCase for types, structs, enums, traits
pub struct MetadataManager { ... }
pub enum TokenKind { ... }
pub trait Parser { ... }
```

### Constants
```rust
// SCREAMING_SNAKE_CASE for constants
const DEFAULT_CACHE_DIR: &str = "./.cache";
const MAX_RETRY_ATTEMPTS: usize = 3;
```

### Files & Modules
```rust
// snake_case for file names
// metadata.rs, not Metadata.rs or metadata-rs
```

## Error Handling

### Use Modern Patterns

**✅ Prefer `let...else`:**
```rust
let Ok(data) = fs::read_to_string(&path) else {
    return None;
};
```

**❌ Avoid verbose match:**
```rust
let data = match fs::read_to_string(&path) {
    Ok(d) => d,
    Err(_) => return None,
};
```

### Error Propagation
```rust
// Use ? operator for error propagation
pub async fn load_all(&self) -> Result<Vec<Function>> {
    let data = self.fetcher.fetch(&url).await?;
    Ok(parse_functions(data)?)
}
```

### Critical Errors
```rust
// Use .expect() with descriptive messages for initialization
MetadataManager::new("./.cache", urls)
    .await
    .expect("Failed to initialize metadata manager")
```

## String Formatting

### Modern Format Syntax

**✅ Inline variables (Rust 2024):**
```rust
format!("ForgeLSP initialized with {count} functions")
```

**❌ Old style:**
```rust
format!("ForgeLSP initialized with {} functions", count)
```

### Documentation
```rust
/// Converts byte offset to LSP Position.
/// 
/// # Arguments
/// * `text` - Source code
/// * `offset` - Byte offset
/// 
/// # Returns
/// LSP Position with line and character
```

## Comments

### Module-Level Documentation
```rust
//! # Module Name
//!
//! Brief description of module purpose.
//!
//! Detailed explanation of:
//! - Key responsibilities
//! - Important algorithms
//! - Integration points
```

### Inline Comments
```rust
// Explain WHY, not WHAT
// ✅ Good: Skip escaped characters to avoid false matches
// ❌ Bad: Check if character is escaped

// Comment before the code it describes
let parsed = parser.parse(); // Not after
```

## Pattern Matching

### Explicit Unit Patterns
```rust
// ✅ Explicit
let () = client.log_message(msg).await;

// ❌ Implicit
let _ = client.log_message(msg).await;
```

### Let Chains (Rust 2024)
```rust
// ✅ Modern
if let Some(config) = load_config()
    && let Some(funcs) = config.custom_functions
    && !funcs.is_empty() {
    // ...
}

// ❌ Old style (nested ifs)
if let Some(config) = load_config() {
    if let Some(funcs) = config.custom_functions {
        if !funcs.is_empty() {
            // ...
        }
    }
}
```

## Memory & Performance

### Clone Optimization
```rust
// ✅ Use clone_from when updating existing data
self.data.write().unwrap().clone_from(&new_data);

// ❌ Unnecessary allocation
*self.data.write().unwrap() = new_data.clone();
```

### Arc for Shared State
```rust
// Share metadata across async tasks
pub struct ForgeScriptServer {
    pub manager: Arc<RwLock<Arc<MetadataManager>>>,
    // ...
}
```

## Async Patterns

### Only Use Async When Needed
```rust
// ❌ DON'T add async if no await
pub async fn new() -> Self { // No await inside!
    Self { ... }
}

// ✅ DO remove unnecessary async
pub fn new() -> Self {
    Self { ... }
}
```

### Spawn for Fire-and-Forget
```rust
pub fn spawn_log(client: Client, msg: String) {
    tokio::spawn(async move {
        let () = client.log_message(MessageType::INFO, msg).await;
    });
}
```

## Documentation URLs

### Wrap in Angle Brackets
```rust
/// See documentation at <https://example.com/docs>
/// 
/// Example URL: <https://raw.githubusercontent.com/owner/repo>
```

## Type Annotations

### Explicit When Helpful
```rust
// ✅ Explicit for clarity
work_done_progress_options: WorkDoneProgressOptions::default()

// ❌ Ambiguous
work_done_progress_options: Default::default()
```

## Linting

### Required Checks
All code must pass:
```bash
# Standard clippy (zero warnings required)
cargo clippy --all-targets -- -D warnings

# Format check
cargo fmt -- --check

# Build verification
cargo build --release
```

### Pedantic Warnings
Run periodically and address reasonable suggestions:
```bash
cargo clippy --all-targets -- -W clippy::pedantic
```

## Summary

**Key Principles:**
1. **Modern Rust:** Use Rust 2024 features
2. **Clarity:** Explicit over implicit
3. **Performance:** Optimize allocations
4. **Safety:** Proper error handling
5. **Consistency:** Follow conventions strictly
6. **Documentation:** Comprehensive comments

**Before Committing:**
```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```
