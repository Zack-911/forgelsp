# semantic.rs

## Overview

The `semantic.rs` module extracts semantic tokens from ForgeScript code and converts them to LSP format for syntax highlighting in editors. It provides intelligent highlighting based on function metadata validation and supports multi-color function highlighting.

## Token Type Mapping

LSP semantic token types (as defined in `server.rs`):

| Index | Type | Description | Examples |
|-------|------|-------------|----------|
| 0 | FUNCTION | Normal functions | `$ping`, `$ban` |
| 1 | KEYWORD | Booleans, semicolons | `true`, `false`, `;` |
| 2 | NUMBER | Numeric literals | `123`, `45.67` |
| 3 | PARAMETER | Alternating function color | `$random` (when multi-color enabled) |
| 4 | STRING | Escape function content | Content inside `$esc[...]` |
| 5 | COMMENT | Comments | `$c[This is a comment]` |

## Main Function

### `extract_semantic_tokens_with_colors`

Public API for semantic token extraction:

```rust
pub fn extract_semantic_tokens_with_colors(
    source: &str,                        // Source code
    use_function_colors: bool,           // Enable multi-color highlighting
    manager: Arc<MetadataManager>,       // Function metadata
) -> Vec<SemanticToken>
```

**Process:**
1. Extract code blocks from `code:` sections
2. For each code block, call `extract_tokens_from_code`
3. Convert absolute tokens to relative LSP format
4. Return semantic tokens for the entire document

## Code Block Extraction

Similar to parser, extracts ForgeScript from backtick-delimited blocks:

```rust
while i < bytes.len() {
    if &source[i..i + 5] == "code:" {
        // Skip whitespace
        // Find opening backtick
        // Extract until closing backtick (handling \` escapes)
        let code = &source[content_start..j];
        let tokens = extract_tokens_from_code(code, content_start, ...);
    }
}
```

**Why Extract?**
- ForgeScript code is embedded in `code:` blocks in Discord bot files
- Only content within backticks should be highlighted
- Prevents highlighting of non-code text

## Token Extraction

### `extract_tokens_from_code`

Core tokenization logic:

```rust
fn extract_tokens_from_code(
    code: &str,
    code_start: usize,                   // Offset in original document
    use_function_colors: bool,
    manager: Arc<MetadataManager>,
) -> Vec<(usize, usize, u32)>           // (start, end, token_type)
```

**Token Format:**
- `(start, end, token_type)` - Absolute byte offsets and type index
- Offsets adjusted by `code_start` for document coordinates

## Special Function Detection

### Comment Functions: `$c[...]`

```rust
if c == b'$' && bytes[i + 1] == b'c' && bytes[i + 2] == b'[' {
    if let Some(end_idx) = find_matching_bracket_raw(bytes, i + 2) {
        // Highlight entire $c[...] as type 5 (COMMENT)
        found.push((i + code_start, end_idx + 1 + code_start, 5));
        i = end_idx + 1;
        continue;
    }
}
```

**Why Raw Bracket Matching?**
- Comment content should be treated as literal
- No need to handle escape sequences inside comments
- Faster execution

### Escape Functions: `$esc[...]`, `$escapeCode[...]`

```rust
if let Some(esc_end) = check_escape_function(bytes, i) {
    // Highlight function name as type 0 (FUNCTION)
    found.push((i + code_start, name_end + code_start, 0));
    // Highlight content as type 4 (STRING)
    found.push((name_end + code_start, esc_end + code_start, 4));
    // Highlight closing bracket as type 0 (FUNCTION)
    found.push((esc_end + code_start, esc_end + 1 + code_start, 0));
}
```

**Three-Part Highlighting:**
1. Function name (`$esc` or `$escapeCode`) → FUNCTION
2. Bracket content → STRING (escaped text)
3. Closing bracket → FUNCTION

### `check_escape_function`

Detects escape functions and returns closing bracket position:

```rust
fn check_escape_function(bytes: &[u8], dollar_idx: usize) -> Option<usize> {
    // Skip $ and modifiers (!, #)
    // Check for "esc[" or "escapeCode["
    // Return closing bracket position using raw matching
}
```

## Metadata-Based Function Highlighting

### Incremental Matching Algorithm

```rust
// Try incremental matching against metadata
let mut best_match_len = 0;
let mut j = i + 1;

// Skip modifiers (!, #)
while j < bytes.len() && (bytes[j] == b'!' || bytes[j] == b'#') {
    j += 1;
}

// Try matching character by character
while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
    let candidate = &code[i..=j];
    if manager.get(candidate).is_some() {
        best_match_len = j - i + 1;  // Update best match
    }
    j += 1;
}
```

**Key Points:**
- Only highlights functions that exist in metadata
- Ensures `$pingMS` doesn't partially highlight as `$ping`
- Supports modifiers: `$!ban`, `$#if`

**Example:**
- Input: `$pingserver`
- Check: `$p`, `$pi`, `$pin`, `$ping` ✓, `$pings`, `$pingse`, ...
- If only `$ping` in metadata → highlight `$ping` only (4 chars)
- If `$pingserver` in metadata → highlight all 12 chars

### Multi-Color Function Support

When `use_function_colors` is enabled:

```rust
let token_type = if use_function_colors {
    let colors = [0, 3];  // Alternate between FUNCTION and PARAMETER
    let color = colors[(function_color_index as usize) % colors.len()];
    function_color_index += 1;
    color
} else {
    0  // All functions use type 0 (FUNCTION)
};
```

**Visual Effect:**
```forgescript
$function1[...]  // Color 0 (FUNCTION)
$function2[...]  // Color 3 (PARAMETER)
$function3[...]  // Color 0 (FUNCTION)
```

Helps distinguish different functions visually.

## Other Token Types

### Numbers

```rust
if c.is_ascii_digit() {
    let start = i;
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
        i += 1;
    }
    found.push((start + code_start, i + code_start, 2));  // Type 2 (NUMBER)
}
```

Supports: `123`, `45.67`, `0.5`

### Booleans

```rust
if &code[i..i + 4] == "true" {
    found.push((i + code_start, i + 4 + code_start, 1));  // Type 1 (KEYWORD)
    i += 4;
}
if &code[i..i + 5] == "false" {
    found.push((i + code_start, i + 5 + code_start, 1));  // Type 1 (KEYWORD)
    i += 5;
}
```

### Semicolons

```rust
if c == b';' && !is_char_escaped(bytes, i) {
    found.push((i + code_start, i + 1 + code_start, 1));  // Type 1 (KEYWORD)
}
```

Only unescaped semicolons (not `\\;`).

## Escape Character Handling

### `is_char_escaped`

Same logic as parser's `is_escaped`:

```rust
fn is_char_escaped(bytes: &[u8], idx: usize) -> bool {
    // For backtick: 1 backslash → escaped
    // For special chars ($, ;, [, ]): 2 backslashes → escaped
}
```

**Why Important:**
- `\\$function` should NOT be highlighted as a function
- `\\;` should NOT be highlighted as a keyword
- Ensures highlighting matches actual parsing behavior

## LSP Token Conversion

### `to_relative_tokens`

Converts absolute byte offsets to LSP's relative delta format:

```rust
fn to_relative_tokens(found: &[(usize, usize, u32)], source: &str) -> Vec<SemanticToken> {
    let mut last_line = 0u32;
    let mut last_col = 0u32;

    for &(start, end, token_type) in found {
        let start_pos = offset_to_position(source, start);
        let end_pos = offset_to_position(source, end);

        let delta_line = start_pos.line.saturating_sub(last_line);
        let delta_start = if delta_line == 0 {
            start_pos.character.saturating_sub(last_col)
        } else {
            start_pos.character
        };

        tokens.push(SemanticToken {
            delta_line,      // Lines since last token
            delta_start,     // Columns since last token (or from line start)
            length: (end_pos.character - start_pos.character).max(1),
            token_type,
            token_modifiers_bitset: 0,
        });

        last_line = start_pos.line;
        last_col = start_pos.character;
    }
}
```

**LSP Format:**
- Tokens are relative to previous token position
- First token relative to document start (0, 0)
- Reduces data transmission size

### `offset_to_position`

Converts byte offset to line/character position:

```rust
fn offset_to_position(text: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, ch) in text.char_indices() {
        if i >= offset { break; }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }

    Position::new(line, col)
}
```

**Character-Based:**
- Uses `char_indices()` for UTF-8 safety
- Counts characters, not bytes (LSP requirement)
- Handles multi-byte characters correctly

## Performance Considerations

1. **Single Pass:** Code scanned once per change
2. **Sorted Output:** Tokens sorted by start position
3. **Early Exit:** Special functions detected early
4. **No Regex:** Direct byte comparisons for speed
5. **SmallVec Opportunity:** Could use SmallVec for token buffer

## Integration with Server

Called from `server.rs::semantic_tokens_full`:

```rust
let use_colors = *self.multiple_function_colors.read().unwrap();
let mgr = self.manager.read().unwrap().clone();
let tokens = extract_semantic_tokens_with_colors(text, use_colors, mgr);

Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
    result_id: None,
    data: tokens,
})))
```

## Example Highlighting

Input:
```forgescript
code: `$ping[example.com]$c[This is a comment]`
```

Tokens Generated:
```
$ping         → (0, 5, 0)   FUNCTION
example.com   → (not highlighted)
$c[...]       → (21, 47, 5) COMMENT
```

Visual Result:
- `$ping` in function color
- `[example.com]` in default color
- `$c[This is a comment]` in comment color
