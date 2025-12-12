# CODING_STYLE.md Update

## Wildcard Imports - Exception for LSP Types

**General Rule:** Avoid wildcard imports (`use module::*`)

**Exception:** LSP type imports are permitted due to extensive usage:

```rust
// ✅ ACCEPTABLE for LSP types
use tower_lsp::lsp_types::*;
```

**Justification:**
- LSP protocol requires 50+ types
- All types clearly namespaced under `lsp_types`
- Explicit imports would be 50+ lines
- No naming conflicts in practice
- Common pattern in LSP implementations

**Rationale:** This exception balances code readability with the CODING_STYLE.md principle of explicit imports. The `lsp_types` module is extensive and all types are clearly LSP-specific, minimizing confusion.
