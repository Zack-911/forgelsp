# parser.rs

## Overview

The `parser.rs` module implements a custom parser for ForgeScript syntax. It tokenizes ForgeScript code, validates function calls against metadata, handles complex escape sequences, and generates detailed diagnostics for syntax errors.

## Core Data Structures

### `ParseResult`

Output of the parsing process:

```rust
pub struct ParseResult {
    pub tokens: Vec<Token<'static>>,      // Tokenized code
    pub diagnostics: Vec<Diagnostic>,      // Syntax errors
    pub functions: Vec<ParsedFunction>,    // Parsed functions
}
```

### `ParsedFunction`

Represents a parsed function call:

```rust
pub struct ParsedFunction {
    pub name: String,                      // Function name (without $)
    pub matched: String,                   // Original matched text
    pub args: Option<Vec<SmallVec<[ParsedArg; 8]>>>, // Arguments
    pub span: (usize, usize),             // Byte range in source
    pub silent: bool,                      // $! modifier
    pub negated: bool,                     // $# modifier
    pub count: Option<usize>,             // Execution count
    pub meta: Arc<Function>,              // Metadata reference
}
```

### `ParsedArg`

Function arguments can be literals or nested functions:

```rust
pub enum ParsedArg {
    Literal { text: Cow<'static, str> },
    Function { func: Box<ParsedFunction> },
}
```

## ForgeScriptParser

### Initialization

```rust
pub struct ForgeScriptParser<'a> {
    manager: Arc<MetadataManager>,  // Function metadata
    code: &'a str,                  // Source code
    skip_extraction: bool,          // Internal parsing flag
}
```

**Two Constructors:**
- `new()` - Standard parser, extracts `code:` blocks
- `new_internal()` - Direct parsing, skips extraction

### Parsing Pipeline

```
Source Code
    ↓
Code Block Extraction (if not internal)
    ↓
Tokenization
    ↓
Function Detection & Validation
    ↓
Argument Parsing
    ↓
Diagnostic Generation
    ↓
ParseResult
```

## Code Block Extraction

ForgeScript code is embedded in `code:` blocks with backticks:

```
code: `$ping[example.com]`
```

### Extraction Algorithm

```rust
while i < self.code.len() {
    // Look for "code:" pattern
    if &self.code[i..i+5] == "code:" {
        // Skip whitespace
        // Find opening backtick
        // Extract content until closing backtick (respecting escapes)
        // Parse extracted content
    }
}
```

**Escape Handling:**
- `\`` → Escaped backtick, don't treat as delimiter
- Content between backticks sent to internal parser

## Escape Sequences

ForgeScript supports two escape patterns:

### Single Backslash (Backticks Only)

```forgescript
\` → ` (literal backtick)
```

### Double Backslash (Special Characters)

```forgescript
\\$ → $ (literal dollar sign)
\\[ → [ (literal bracket)
\\] → ] (literal bracket)
\\; → ; (literal semicolon)
\\\\ → \ (literal backslash)
```

### `is_escaped` Function

Determines if a character is escaped:

```rust
fn is_escaped(code: &str, byte_idx: usize) -> bool {
    // For backtick: check for 1 backslash
    if c == b'`' {
        // Count backslashes before position
        // Odd count → escaped
    }
    
    // For special chars: check for 2 backslashes
    if matches!(c, b'$' | b';' | b'[' | b']') {
        // Count backslashes before position
        // Exactly 2 or even count → escaped
    }
}
```

### `unescape_string` Function

Converts escaped sequences back to literals:

```rust
pub fn unescape_string(input: &str) -> String {
    // Process: \` → `
    // Process: \\$ → $, \\; → ;, etc.
    // Keep unrecognized backslashes as-is
}
```

## Special Function Handling

### Escape Functions: `$esc`, `$escape`, `$escapeCode`

These functions treat their bracket content as **literal text**, ignoring all ForgeScript syntax:

```rust
fn is_escape_function(name: &str) -> bool {
    lower == "esc" || lower == "escape" || lower == "escapeCode"
}
```

**Special Handling:**
- Brackets matched **without** escape sequence processing
- Content not parsed for nested functions
- Uses `find_matching_bracket_raw()` instead of `find_matching_bracket()`

### JavaScript Expressions: `${...}`

Detected and tokenized as `TokenKind::JavaScript`:

```rust
if c == '$' && next_char == '{' {
    // Find matching brace
    // Extract content
    // Create JavaScript token
}
```

## Bracket Matching

Two bracket matching algorithms:

### `find_matching_bracket_raw`

Raw bracket matching (no escape handling):

```rust
fn find_matching_bracket_raw(code: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in code.char_indices().skip_while(...) {
        if c == '[' { depth += 1; }
        else if c == ']' {
            depth -= 1;
            if depth == 0 { return Some(i); }
        }
    }
    None
}
```

Used for: `$esc[...]`, `$escape[...]`, `$escapeCode[...]`

### `find_matching_bracket`

Smart bracket matching with escape and nested function handling:

```rust
fn find_matching_bracket(code: &str, open_idx: usize) -> Option<usize> {
    // Skip escaped brackets (\\[ and \\])
    // Skip entire escape functions
    // Track depth for nested brackets
}
```

**Escape Function Detection:**
```rust
if c == '$' {
    if let Some(escape_end) = find_escape_function_end(code, i) {
        // Jump past entire $esc[...] structure
        i = escape_end + 1;
        continue;
    }
}
```

Used for: Regular functions like `$ping[...]`

## Argument Parsing

### `parse_nested_args`

Parses semicolon-separated arguments with nesting support:

```rust
fn parse_nested_args(input: &str, manager: Arc<MetadataManager>)
    -> Result<Vec<SmallVec<[ParsedArg; 8]>>, nom::Err<()>>
{
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;  // Track nested brackets
    
    for c in input.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            ';' if depth == 0 => {
                // Argument separator at top level
                args.push(parse_single_arg(&current, manager)?);
                current.clear();
            }
            _ => current.push(c),
        }
    }
    
    Ok(args)
}
```

**Key Features:**
- Semicolons only split arguments at depth 0
- Nested brackets don't trigger splits
- Escape sequences handled during character iteration
- Escape functions skipped entirely

### `parse_single_arg`

Determines if argument is a function or literal:

```rust
fn parse_single_arg(input: &str, manager: Arc<MetadataManager>, force_literal: bool)
    -> Result<SmallVec<[ParsedArg; 8]>, nom::Err<()>>
{
    if !force_literal && input.starts_with('$') {
        // Parse as nested function
        let parser = ForgeScriptParser::new_internal(manager, input);
        let res = parser.parse_internal();
        if let Some(f) = res.functions.first() {
            return Ok(smallvec![ParsedArg::Function { func: Box::new(f.clone()) }]);
        }
    }
    
    // Parse as literal
    Ok(smallvec![ParsedArg::Literal { text: Cow::Owned(input.to_string()) }])
}
```

## Validation

### Argument Count Validation

```rust
fn validate_arg_count(
    name: &str,
    total: usize,
    min: usize,
    max: usize,
    has_rest: bool,
    diagnostics: &mut Vec<Diagnostic>,
    span: (usize, usize),
    _source: &str,
) {
    if total < min {
        diagnostics.push(Diagnostic {
            message: format!("${} expects at least {} args, got {}", name, min, total),
            ...
        });
    } else if !has_rest && total > max {
        diagnostics.push(Diagnostic {
            message: format!("${} expects at most {} args, got {}", name, max, total),
            ...
        });
    }
}
```

### `compute_arg_counts`

Calculates min/max argument requirements from metadata:

```rust
fn compute_arg_counts(meta: &Function) -> (usize, usize) {
    let min = meta.args
        .as_ref()
        .map(|v| v.iter().filter(|a| a.required.unwrap_or(false)).count())
        .unwrap_or(0);
    
    let max = if has_rest_param {
        usize::MAX
    } else {
        meta.args.as_ref().map(|v| v.len()).unwrap_or(0)
    };
    
    (min, max)
}
```

## Diagnostic Generation

Diagnostics are generated for:

1. **Unknown Functions**
   ```
   Unknown function `$unknownFunc`
   ```

2. **Unclosed Brackets**
   ```
   Unclosed '[' for function `$ping`
   ```

3. **Invalid Argument Counts**
   ```
   $ping expects at least 1 args, got 0
   ```

4. **Bracket Misuse**
   ```
   $randomString does not accept brackets
   ```

5. **Escape Function Errors**
   ```
   $esc expects brackets `[...]` containing content to escape
   ```

## Performance Optimizations

1. **SmallVec:** Stack-allocated for args (up to 8 items)
2. **Cow Strings:** Avoid copying when possible
3. **Arc Metadata:** Shared ownership, no cloning
4. **Lazy Parsing:** Only parse on document changes
5. **Cache:** `parsed_cache` in server state

## Token Types

```rust
pub enum TokenKind {
    Text,          // Plain text
    FunctionName,  // $functionName
    Escaped,       // Content of $esc[...]
    JavaScript,    // Content of ${...}
    Unknown,       // Unknown function
}
```

## Example Parse Flow

Input:
```forgescript
$ping[example.com;$random[1;10]]
```

Parse Steps:
1. Detect `$ping` function
2. Find opening `[`
3. Parse args: `example.com` and `$random[1;10]`
4. Recursively parse `$random` as nested function
5. Validate arg counts against metadata
6. Generate `ParsedFunction` with nested structure

Output:
```rust
ParsedFunction {
    name: "ping",
    args: Some(vec![
        smallvec![ParsedArg::Literal { text: "example.com" }],
        smallvec![ParsedArg::Function {
            func: Box::new(ParsedFunction {
                name: "random",
                args: Some(vec![
                    smallvec![ParsedArg::Literal { text: "1" }],
                    smallvec![ParsedArg::Literal { text: "10" }]
                ]),
                ...
            })
        }]
    ]),
    ...
}
```

## Directives

### Ignore Error Directive

The parser supports a special directive to suppress errors and function registration for the **next logical line**. This is useful for suppressing false positives or intentionally invalid code.

**Syntax:**
```forgescript
$c[fs@ignore-error]
```

**Behavior:**
- Suppresses all diagnostics (unknown function, arg count, etc.) for the next line.
- Prevents functions on the next line from being registered in the `functions` list.
- Resets automatically after the next newline.
- Does **not** affect tokenization (tokens are still emitted).

**Example:**

```forgescript
$c[fs@ignore-error]
$doesNotExist[a;b]  // No error, function not registered
$ping               // Parsed normally
```

