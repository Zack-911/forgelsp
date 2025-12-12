# diagnostics.rs

## Overview

The `diagnostics.rs` module handles the conversion and publishing of parser-generated diagnostics to the LSP client. It bridges the gap between ForgeLSP's internal error representation and the Language Server Protocol's diagnostic format.

## Key Components

### `publish_diagnostics` Function

Asynchronous function that converts parser diagnostics to LSP format and publishes them to the client.

**Parameters:**
- `server: &ForgeScriptServer` - Reference to the LSP server instance
- `uri: &Url` - Document URI where diagnostics should appear
- `text: &str` - Full document text for position calculation
- `diagnostics_data: &[ParseDiagnostic]` - Array of parser diagnostics to convert

**Process:**
1. Iterates through each `ParseDiagnostic` from the parser
2. Converts byte offsets to LSP `Position` objects
3. Creates LSP `Diagnostic` with severity set to `ERROR`
4. Publishes all diagnostics to the client via `client.publish_diagnostics()`

## Position Conversion

### `offset_to_position` Function

Converts byte offsets in the source text to LSP line/character positions.

**Algorithm:**
1. Counts newline characters (`\n`) before the offset to determine line number
2. Finds the last newline before the offset using `rfind('\n')`
3. Calculates character position as: `offset - last_newline_position`
4. Returns `Position { line, character }`

**Why This Approach:**
- LSP positions are 0-indexed (line, character) tuples
- Line numbers are determined by counting newlines
- Character positions are measured from the start of the current line
- This is more efficient than iterating character-by-character for small offsets

## Integration with Parser

The diagnostics flow:

```
ForgeScriptParser::parse()
    ↓
ParseResult { diagnostics: Vec<Diagnostic> }
    ↓
publish_diagnostics()
    ↓
LSP Client (displays errors in editor)
```

## Diagnostic Severity

Currently, all diagnostics are published with `DiagnosticSeverity::ERROR`. Future enhancements could include:
- **WARNING** - For deprecated functions or style issues
- **INFORMATION** - For optimization suggestions
- **HINT** - For code improvements

## Performance Considerations

- Position conversion is O(n) where n is the number of characters before the offset
- For large files, this could be optimized with a line index cache
- Diagnostics are published asynchronously to avoid blocking the LSP server

## Example

For the ForgeScript code:
```forgescript
$unknownFunc[arg1;arg2]
```

The parser generates:
```rust
Diagnostic {
    message: "Unknown function `$unknownFunc`",
    start: 0,   // byte offset
    end: 23     // byte offset
}
```

Which is converted to:
```rust
Diagnostic {
    range: Range {
        start: Position { line: 0, character: 0 },
        end: Position { line: 0, character: 23 }
    },
    severity: Some(DiagnosticSeverity::ERROR),
    message: "Unknown function `$unknownFunc`",
    ..Default::default()
}
```
