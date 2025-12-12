# AI Assistant Guidelines

This document provides specific guidance for AI assistants when working with the ForgeLSP codebase.

## Quick Reference

### File Purposes
```
src/
├── main.rs          → Entry point, LSP initialization
├── server.rs        → LSP trait implementation, all handlers
├── parser.rs        → ForgeScript syntax parsing
├── metadata.rs      → Function metadata management (Trie)
├── semantic.rs      → Token extraction for highlighting
├── hover.rs         → Hover tooltip generation
├── diagnostics.rs   → Error reporting
└── utils.rs         → Config loading, helpers
```

### Key Patterns

**Threading:**
```rust
Arc<RwLock<T>>  // All shared state
```

**Error Handling:**
```rust
let Ok(value) = operation() else {
    return None;  // let-else pattern (Rust 2024)
};
```

**Format Strings:**
```rust
format!("{variable}")  // Inline, not format!("{}", variable)
```

## Common Tasks

### Adding an LSP Feature

1. **Handler in server.rs:**
```rust
async fn new_handler(&self, params: Params) -> Result<Response> {
    // 1. Get params
    // 2. Access shared state
    // 3. Call helper modules
    // 4. Return response
}
```

2. **Register capability:**
```rust
// In initialize()
new_capability: Some(true)
```

3. **Add tests:**
```rust
#[tokio::test]
async fn test_new_feature() { ... }
```

### Modifying Parser

**Remember:**
- Escape sequences: `\`` (quote), `\\$` (dollar), `\\;` (semicolon)
- Escape functions: `$esc[...]` content is literal
- Bracket depth tracking required
- Validate against metadata

**Example:**
```rust
// Always check if escaped before treating as special
if c == '$' && !is_escaped(code, idx) {
    // Process function
}
```

### Updating Metadata

**Cache Key:** Base64(URL)
**Trie:** O(k) lookup, supports aliases
**Thread-safe:** Arc<RwLock<>>

```rust
// Add custom function
manager.add_custom_functions(vec![CustomFunction {
    name: "$newFunc".to_string(),
    description: Some("...".to_string()),
    params: Some(...),
}])?;
```

## Coding Conventions

### Imports
```rust
// Order: std → external → internal
use std::sync::Arc;
use regex::Regex;
use crate::parser::Parser;
```

### Error Messages
```rust
// Be specific and actionable
.expect("Failed to initialize metadata manager: check cache directory permissions")
```

### Documentation
```rust
/// Brief description.
///
/// # Arguments
/// * `param` - What it is
///
/// # Returns
/// What comes back
```

## What to Check

### Before Making Changes
- [ ] Read relevant `docs/*.md` file
- [ ] Check `.guidelines/CODING_STYLE.md`
- [ ] Understand the module architecture

### After Making Changes
- [ ] Run `cargo fmt`
- [ ] Run `cargo clippy --all-targets -- -D warnings`
- [ ] Run `cargo test` (if tests exist)
- [ ] Update documentation if API changed
- [ ] Check that examples still work

## Common Mistakes to Avoid

### ❌ DON'T
```rust
// Don't use wildcard imports (except lsp_types)
use crate::parser::*;

// Don't ignore errors silently
let _ = operation();  // Unless explicitly unit type

// Don't use old match for simple cases
let data = match result {
    Ok(d) => d,
    Err(_) => return None,
};

// Don't add async unnecessarily
pub async fn new() -> Self { }  // No await inside!
```

### ✅ DO
```rust
// Use explicit imports
use crate::parser::{Parser, ParseResult};

// Be explicit about unit
let () = client.log_message(...).await;

// Use let-else for early returns
let Ok(data) = result else {
    return None;
};

// Only async when needed
pub fn new() -> Self { }
```

## LSP-Specific Patterns

### Document Synchronization
```rust
// Cache in did_open/did_change
self.documents.write().unwrap().insert(uri, text);

// Parse and cache results
let parsed = parser.parse();
self.parsed_cache.write().unwrap().insert(uri, parsed);

// Publish diagnostics
publish_diagnostics(self, &uri, &text, &parsed.diagnostics).await;
```

### Position Conversion
```rust
// Byte offset → LSP Position
fn offset_to_position(text: &str, offset: usize) -> Position {
    // Count lines by newlines
    // Character position from last newline
}
```

### Threading Pattern
```rust
// Read shared state
let mgr = self.manager.read().unwrap().clone();

// Work with mgr outside lock
let result = mgr.get(&key);

// Write state (minimize lock time)
{
    let mut cache = self.cache.write().unwrap();
    cache.insert(key, value);
} // Lock released
```

## Performance Tips

1. **Clone Early:** Clone Arc'd data before doing work
2. **Lock Shortly:** Minimize RwLock hold time
3. **Cache Results:** Store parse results per document
4. **Async Logging:** Fire-and-forget with spawn
5. **Trie for Lookups:** O(k) is faster than iterating

## Testing Approach

### Unit Tests
Test individual functions:
```rust
#[test]
fn test_escape_handling() {
    assert_eq!(is_escaped("\\$", 1), true);
    assert_eq!(is_escaped("$", 0), false);
}
```

### Integration Tests
Test LSP workflows end-to-end (when you add them)

### Manual Testing
Always test with real IDE:
1. Build: `cargo build --release`
2. Copy binary to extension
3. Test hover, completion, diagnostics

## Debugging Assistance

### Reading LSP Traces
In VS Code: Output → ForgeScript Language Server

### Common Issues

**"No completions showing"**
→ Check MetadataManager loaded functions
→ Verify trigger character is `$`

**"Wrong diagnostics"**
→ Check offset_to_position conversion
→ Verify parser escape handling

**"Hover not working"**
→ Check token detection at cursor
→ Verify metadata lookup

## Documentation Updates

When changing:
- **Public API** → Update module doc comments
- **Architecture** → Update `.guidelines/ARCHITECTURE.md`
- **New feature** → Update `README.md` + `docs/*.md`
- **Style** → Update `.guidelines/CODING_STYLE.md`

## Quick Commands

```bash
# Format code
cargo fmt

# Check compilation
cargo check

# Run clippy
cargo clippy --all-targets -- -D warnings

# Pedantic suggestions
cargo clippy --all-targets -- -W clippy::pedantic

# Build release
cargo build --release

# Run (LSP mode)
cargo run
```

## Questions to Ask Yourself

Before committing code:
1. Does this follow Rust 2024 idioms?
2. Is error handling explicit?
3. Are all public items documented?
4. Does clippy pass with -D warnings?
5. Is the code formatted?
6. Will this break existing functionality?
7. Are there tests (or should there be)?
8. Is the change documented?

## Remember

These Instructions Must Never Be Ignored:
- **Safety First:** Rust's type system is your friend
- **Explicit > Implicit:** Clear code over clever code
- **Document Why:** Code shows what, comments show why
- **Test Changes:** Don't break existing features
- **Ask Questions:** Better to clarify than assume

## Useful Patterns from Codebase

### Async Fire-and-Forget
```rust
pub fn spawn_log(client: Client, msg: String) {
    tokio::spawn(async move {
        let () = client.log_message(MessageType::INFO, msg).await;
    });
}
```

### Configuration Loading
```rust
// Try each workspace folder
for folder in workspace_folders {
    if let Some(config) = try_load_config(folder) {
        return Some(config);
    }
}
None
```

### Trie Insertion
```rust
// Insert function and all aliases
trie.insert(&func.name, Arc::new(func.clone()));
for alias in &aliases {
    trie.insert(alias, Arc::new(func.clone()));
}
```
