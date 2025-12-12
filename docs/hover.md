# hover.rs

## Overview

The `hover.rs` module implements LSP hover functionality for ForgeScript, providing rich markdown documentation when users hover over function names in their code.

## Key Components

### `handle_hover` Function

Main entry point for processing hover requests from the LSP client.

**Parameters:**
- `server: &ForgeScriptServer` - LSP server instance with access to documents and metadata
- `params: HoverParams` - LSP hover request parameters containing position and document URI

**Returns:**
- `Result<Option<Hover>>` - Hover content or None if no information available

## Token Detection Algorithm

### Finding Token Boundaries

The hover handler uses a two-phase approach to identify the token under the cursor:

#### Phase 1: Calculate Byte Offset

Converts LSP position (line, character) to a byte offset in the document:

```rust
for (line_idx, line) in text.split_inclusive('\n').enumerate() {
    if line_idx == position.line {
        offset += position.character;
        break;
    }
    offset += line.len();
}
```

#### Phase 2: Expand to Token Boundaries

Starting from the cursor position, expands left and right to find token boundaries:

**Left Expansion:**
- Moves backward while encountering alphanumeric, `_`, `.`, or `$` characters
- Stops at escaped `$` characters (detected via `is_escaped()`)
- Stops at whitespace or other delimiters

**Right Expansion:**
- Moves forward while encountering valid identifier characters
- Stops at whitespace, brackets, or other delimiters

**Valid identifier characters:** `a-z`, `A-Z`, `0-9`, `_`, `.`, `$`

## Escape Handling

### `is_escaped` Function

Determines if a character at a given byte index is escaped by backslashes.

**Algorithm:**
1. Start from the character position
2. Count consecutive backslashes before it
3. If odd number of backslashes → character is escaped
4. If even number of backslashes → character is not escaped

**Example:**
```forgescript
\$function   → $ is escaped (1 backslash)
\\$function  → $ is NOT escaped (2 backslashes)
\\\$function → $ is escaped (3 backslashes)
```

## Special Cases

### Excluded Functions

Hover information is **not** provided for:

1. **Escape Functions:** `$esc`, `$escape`
   - These are meta-functions that don't have standard documentation
   - Their content should be treated as literal text

2. **JavaScript Expressions:** `${...}`
   - These are JavaScript code, not ForgeScript functions
   - Should be handled by JavaScript language features

## Markdown Generation

The hover content is generated in GitHub Flavored Markdown format:

### Function Signature

```markdown
```forgescript
$functionName[arg1; arg2; ...arg3?] -> OutputType
```
```

**Formatting Rules:**
- `brackets: Some(true)` → Show brackets as required
- `brackets: Some(false)` → Show brackets with "Note: brackets are optional"
- `brackets: None` → No brackets in signature
- Rest parameters prefixed with `...`
- Optional parameters suffixed with `?`

### Description

The function's description from metadata is included verbatim.

### Examples

Up to 2 examples are shown in code blocks:

```markdown
**Examples:**

```forgescript
$functionName[example1]
```

```forgescript
$functionName[example2]
```
```

## Metadata Lookup

The function metadata is retrieved from the `MetadataManager` via the Trie:

```rust
let mgr = server.manager.read().unwrap();
if let Some(func_ref) = mgr_inner.get(&token) {
    // Use func_ref.name, func_ref.description, etc.
}
```

## Performance Logging

Every hover request logs performance metrics:

```rust
spawn_log(
    server.client.clone(),
    MessageType::LOG,
    format!("[PERF] hover: {} in {:?}", func_name, start.elapsed())
);
```

This helps identify slow hover responses for optimization.

## Example Hover Flow

1. User hovers over `$ping` in `$ping[example.com]`
2. LSP sends hover request with position
3. `handle_hover` calculates byte offset
4. Token boundaries expanded: `$ping`
5. Metadata lookup finds `ping` function
6. Markdown generated with signature, description, examples
7. Hover content sent to client
8. Editor displays rich tooltip

## Integration Points

- **Documents Cache:** `server.documents` stores document content
- **Metadata Manager:** `server.manager` provides function metadata
- **LSP Client:** `server.client` used for logging
- **Utils:** `spawn_log` for async logging

## Future Enhancements

- Cache hover results for frequently accessed functions
- Support for hovering over function arguments
- Link to online documentation
- Show function source code location
