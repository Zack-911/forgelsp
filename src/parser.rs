//! # ForgeScript Parser Module
//!
//! Custom parser for ForgeScript syntax that handles:
//! - Code block extraction from `code:` sections
//! - Function call tokenization (`$functionName[args]`)
//! - Complex escape sequence handling (backticks and special characters)
//! - Nested bracket matching with escape function support
//! - Argument parsing with nested function calls
//! - Diagnostic generation for syntax errors
//!
//! The parser validates function calls against metadata and generates detailed
//! error messages for invalid syntax or unknown functions.

use crate::metadata::{Function, MetadataManager};
use smallvec::{SmallVec, smallvec};
use std::borrow::Cow;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    #[allow(dead_code)]
    pub start: usize,
    #[allow(dead_code)]
    pub end: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Text,
    FunctionName,
    Escaped,
    JavaScript,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Token<'a> {
    #[allow(dead_code)]
    pub kind: TokenKind,
    #[allow(dead_code)]
    pub text: &'a str,
    #[allow(dead_code)]
    pub start: usize,
    #[allow(dead_code)]
    pub end: usize,
}

#[derive(Debug, Clone)]
pub struct ParsedFunction {
    #[allow(dead_code)]
    pub name: String,
    #[allow(dead_code)]
    pub matched: String,
    #[allow(dead_code)]
    pub args: Option<Vec<SmallVec<[ParsedArg; 8]>>>,
    #[allow(dead_code)]
    pub span: (usize, usize),
    #[allow(dead_code)]
    pub silent: bool,
    #[allow(dead_code)]
    pub negated: bool,
    #[allow(dead_code)]
    pub count: Option<usize>,
    #[allow(dead_code)]
    pub meta: Arc<Function>,
}

#[derive(Debug, Clone)]
pub struct ParseResult {
    #[allow(dead_code)]
    pub tokens: Vec<Token<'static>>,
    pub diagnostics: Vec<Diagnostic>,
    pub functions: Vec<ParsedFunction>,
}

#[derive(Debug, Clone)]
pub enum ParsedArg {
    Literal {
        #[allow(dead_code)]
        text: Cow<'static, str>,
    },
    #[allow(dead_code)]
    Function { func: Box<ParsedFunction> },
}

/// Check if a character at the given byte index is escaped.
/// For backtick: 1 backslash escapes it (\`)
/// For special chars ($, ;, [, ]): 2 backslashes escape it (\\$, \\;, etc.)
fn is_escaped(code: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 {
        return false;
    }

    let bytes = code.as_bytes();
    let c = bytes[byte_idx];

    // For backtick, check if there's exactly 1 backslash before it
    if c == b'`' {
        if byte_idx >= 1 && bytes[byte_idx - 1] == b'\\' {
            // Check if the backslash itself is escaped (even number of backslashes before it)
            let mut backslash_count = 1;
            let mut pos = byte_idx - 1;
            while pos > 0 {
                pos -= 1;
                if bytes[pos] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            // If odd number of total backslashes, the backtick is escaped
            return backslash_count % 2 == 1;
        }
        return false;
    }

    // For special chars ($, ;, [, ]), check if there are exactly 2 backslashes before it
    if matches!(c, b'$' | b';' | b'[' | b']') {
        if byte_idx >= 2 && bytes[byte_idx - 1] == b'\\' && bytes[byte_idx - 2] == b'\\' {
            // Check if the backslashes themselves are escaped
            let mut backslash_count = 2;
            let mut pos = byte_idx - 2;
            while pos > 0 {
                pos -= 1;
                if bytes[pos] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            // If exactly 2 backslashes (even total), the char is escaped
            return backslash_count == 2 || backslash_count % 2 == 0;
        }
        return false;
    }

    false
}

/// Process escape sequences in a string, returning the unescaped version.
/// Handles: \` -> `, \\$ -> $, \\[ -> [, \\] -> ], \\; -> ;, \\\\ -> \\
#[allow(dead_code)]
pub fn unescape_string(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if i + 1 < bytes.len() {
                let next = bytes[i + 1];
                // Check for backtick escape: \`
                if next == b'`' {
                    result.push('`');
                    i += 2;
                    continue;
                }
                // Check for double backslash escapes: \\$, \\;, \\[, \\], \\\\
                if i + 2 < bytes.len() && next == b'\\' {
                    let third = bytes[i + 2];
                    if matches!(third, b'$' | b';' | b'[' | b']') {
                        result.push(third as char);
                        i += 3;
                        continue;
                    }
                    if third == b'\\' {
                        result.push('\\');
                        i += 3;
                        continue;
                    }
                }
            }
            // Keep the backslash if it's not a recognized escape
            result.push('\\');
            i += 1;
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }

    result
}

/// Check if the function name is an escape function ($esc or $escape)
fn is_escape_function(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "esc" || lower == "escape" || lower == "escapecode"
}

/// Detect if we're at the start of an escape function and return its end position.
/// Returns None if not at an escape function.
/// This helps bracket matchers skip escape function contents entirely.
fn find_escape_function_end(code: &str, dollar_idx: usize) -> Option<usize> {
    let bytes = code.as_bytes();

    // Check if we're at a $ that's not escaped
    if dollar_idx >= code.len() || bytes[dollar_idx] != b'$' {
        return None;
    }

    if is_escaped(code, dollar_idx) {
        return None;
    }

    // Skip $ and any modifiers (!, #)
    let mut pos = dollar_idx + 1;
    while pos < bytes.len() && (bytes[pos] == b'!' || bytes[pos] == b'#') {
        pos += 1;
    }

    // Read function name
    let name_start = pos;
    while pos < bytes.len() && (bytes[pos].is_ascii_alphanumeric() || bytes[pos] == b'_') {
        pos += 1;
    }

    if pos == name_start {
        return None; // No function name
    }

    let name = &code[name_start..pos];
    if !is_escape_function(name) {
        return None; // Not an escape function
    }

    // Check for opening bracket
    if pos >= bytes.len() || bytes[pos] != b'[' {
        return None; // Escape function must have brackets
    }

    // Find the matching bracket using raw matching (no escape handling)
    find_matching_bracket_raw(code, pos)
}

pub struct ForgeScriptParser<'a> {
    manager: Arc<MetadataManager>,
    code: &'a str,
    skip_extraction: bool,
}

impl<'a> ForgeScriptParser<'a> {
    pub fn new(manager: Arc<MetadataManager>, code: &'a str) -> Self {
        Self {
            manager,
            code,
            skip_extraction: false,
        }
    }

    fn new_internal(manager: Arc<MetadataManager>, code: &'a str) -> Self {
        Self {
            manager,
            code,
            skip_extraction: true,
        }
    }

    pub fn parse(&self) -> ParseResult {
        // If we're already inside extracted code, skip extraction and go straight to parsing
        if self.skip_extraction {
            return self.parse_internal();
        }

        // Manually extract code blocks, handling escaped backticks properly
        // Pattern: code: ` ... ` where backticks can be escaped with \`
        let mut code_to_parse = String::new();
        let mut offsets: Vec<usize> = Vec::new();
        let mut block_count = 0;

        let bytes = self.code.as_bytes();
        let mut i = 0;

        while i < self.code.len() {
            // Look for "code:" pattern
            if i + 5 <= self.code.len() && &self.code[i..i + 5] == "code:" {
                // Skip "code:" and any whitespace
                let mut j = i + 5;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                    j += 1;
                }

                // Check for opening backtick
                if j < bytes.len() && bytes[j] == b'`' {
                    j += 1; // Skip the opening backtick
                    let content_start = j;

                    // Find the matching closing backtick, respecting escapes
                    let mut found_end = false;
                    while j < bytes.len() {
                        if bytes[j] == b'\\' && j + 1 < bytes.len() {
                            // Skip backslash and the next character (escaped)
                            j += 2;
                            continue;
                        }

                        if bytes[j] == b'`' {
                            // Found unescaped closing backtick
                            found_end = true;
                            break;
                        }

                        j += 1;
                    }

                    if found_end {
                        block_count += 1;
                        let content = &self.code[content_start..j];
                        offsets.push(content_start);
                        code_to_parse.push_str(content);
                        code_to_parse.push('\n');
                        i = j + 1; // Move past the closing backtick
                        continue;
                    }
                }
            }

            i += 1;
        }

        if block_count > 0 {
            // Parse only the extracted code block contents
            let parser = ForgeScriptParser::new_internal(self.manager.clone(), &code_to_parse);
            let mut result = parser.parse();

            // Adjust positions back to original file coordinates
            if !offsets.is_empty() {
                let offset = offsets[0]; // Use first block's offset for now
                for diag in &mut result.diagnostics {
                    diag.start += offset;
                    diag.end += offset;
                }
                for func in &mut result.functions {
                    func.span.0 += offset;
                    func.span.1 += offset;
                }
            }

            return result;
        }
        ParseResult {
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            functions: Vec::new(),
        }
    }

    fn parse_internal(&self) -> ParseResult {
        let mut tokens: Vec<Token> = Vec::new();
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut functions: Vec<ParsedFunction> = Vec::new();

        let mut iter = self.code.char_indices().peekable();
        let mut last_idx = 0;

        while let Some((idx, c)) = iter.next() {
            // Handle backslash escaping
            if c == '\\'
                && let Some(&(_next_idx, next_c)) = iter.peek()
            {
                // Check for backtick escape: \`
                if next_c == '`' {
                    // Push text before the backslash
                    if last_idx < idx {
                        tokens.push(Token {
                            kind: TokenKind::Text,
                            text: Box::leak(self.code[last_idx..idx].to_string().into_boxed_str()),
                            start: last_idx,
                            end: idx,
                        });
                    }
                    iter.next(); // consume the backtick
                    last_idx = idx; // Start from backslash
                    continue;
                }
                // Check for double backslash escapes: \\$, \\;, \\[, \\]
                if next_c == '\\' {
                    // Look ahead one more character
                    iter.next(); // consume second backslash
                    if let Some(&(_third_idx, third_c)) = iter.peek()
                        && matches!(third_c, '$' | '[' | ']' | ';' | '\\')
                    {
                        // Push text before the first backslash
                        if last_idx < idx {
                            tokens.push(Token {
                                kind: TokenKind::Text,
                                text: Box::leak(
                                    self.code[last_idx..idx].to_string().into_boxed_str(),
                                ),
                                start: last_idx,
                                end: idx,
                            });
                        }
                        iter.next(); // consume the escaped character
                        last_idx = idx; // Start from first backslash
                        continue;
                    }
                    continue; // Skip the double backslash if not escaping anything
                }
            }

            if c == '$' && !is_escaped(self.code, idx) {
                // push previous text as a token
                if last_idx < idx {
                    tokens.push(Token {
                        kind: TokenKind::Text,
                        text: Box::leak(self.code[last_idx..idx].to_string().into_boxed_str()),
                        start: last_idx,
                        end: idx,
                    });
                }

                let start = idx;

                // Check for JavaScript expression ${...}
                if let Some(&(brace_idx, '{')) = iter.peek() {
                    if let Some(end_idx) = find_matching_brace(self.code, brace_idx) {
                        // Everything inside is JavaScript code
                        let js_content = &self.code[brace_idx + 1..end_idx];

                        // Advance iterator past the closing brace
                        while let Some(&(j, _)) = iter.peek() {
                            if j <= end_idx {
                                iter.next();
                            } else {
                                break;
                            }
                        }
                        last_idx = end_idx + 1;

                        // Create a JavaScript token
                        tokens.push(Token {
                            kind: TokenKind::JavaScript,
                            text: Box::leak(js_content.to_string().into_boxed_str()),
                            start,
                            end: last_idx,
                        });
                        continue;
                    } else {
                        diagnostics.push(Diagnostic {
                            message: "Unclosed '{' for JavaScript expression `${...}`".to_string(),
                            start,
                            end: self.code.len(),
                        });
                        last_idx = self.code.len();
                        continue;
                    }
                }

                let mut silent = false;
                let mut negated = false;

                if let Some(&(_, next_c)) = iter.peek() {
                    if next_c == '!' {
                        silent = true;
                        iter.next();
                    } else if next_c == '#' {
                        negated = true;
                        iter.next();
                    }
                }

                // read function name
                let mut name_end = idx;
                let mut name_chars = vec![];
                while let Some(&(i, ch)) = iter.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        name_chars.push(ch);
                        name_end = i + ch.len_utf8();
                        iter.next();
                    } else {
                        break;
                    }
                }
                let name = name_chars.iter().collect::<String>();
                last_idx = name_end;

                // Check if this is an escape function ($esc or $escape)
                if is_escape_function(&name) {
                    // Handle $esc[...] and $escape[...]
                    if let Some(&(bracket_idx, '[')) = iter.peek() {
                        if let Some(end_idx) = find_matching_bracket_raw(self.code, bracket_idx) {
                            // Everything inside is treated as literal escaped text
                            let escaped_content = &self.code[bracket_idx + 1..end_idx];

                            // Advance iterator past the closing bracket
                            while let Some(&(j, _)) = iter.peek() {
                                if j <= end_idx {
                                    iter.next();
                                } else {
                                    break;
                                }
                            }
                            last_idx = end_idx + 1;

                            // Create an escaped text token
                            tokens.push(Token {
                                kind: TokenKind::Escaped,
                                text: Box::leak(escaped_content.to_string().into_boxed_str()),
                                start,
                                end: last_idx,
                            });
                            continue;
                        } else {
                            diagnostics.push(Diagnostic {
                                message: format!("Unclosed '[' for escape function `${}`", name),
                                start,
                                end: self.code.len(),
                            });
                            last_idx = self.code.len();
                            continue;
                        }
                    } else {
                        // $esc or $escape without brackets - treat as unknown function
                        diagnostics.push(Diagnostic {
                            message: format!(
                                "${} expects brackets `[...]` containing content to escape",
                                name
                            ),
                            start,
                            end: last_idx,
                        });
                        tokens.push(Token {
                            kind: TokenKind::Unknown,
                            text: Box::leak(
                                self.code[start..last_idx].to_string().into_boxed_str(),
                            ),
                            start,
                            end: last_idx,
                        });
                        continue;
                    }
                }

                // parse args if any
                let mut args_text: Option<&str> = None;
                if let Some(&(i, '[')) = iter.peek() {
                    if let Some(end_idx) = find_matching_bracket(self.code, i) {
                        args_text = Some(&self.code[i + 1..end_idx]);
                        while let Some(&(j, _)) = iter.peek() {
                            if j <= end_idx {
                                iter.next();
                            } else {
                                break;
                            }
                        }
                        last_idx = end_idx + 1;
                    } else {
                        diagnostics.push(Diagnostic {
                            message: format!("Unclosed '[' for function `${}`", name),
                            start,
                            end: self.code.len(),
                        });
                        last_idx = self.code.len();
                    }
                }

                if let Some(meta) = self.manager.get(&format!("${}", name)) {
                    let (min_args, max_args) = compute_arg_counts(&meta);
                    let mut parsed_args: Option<Vec<SmallVec<[ParsedArg; 8]>>> = None;

                    if let Some(inner) = args_text {
                        // brackets: true -> required, brackets: false -> optional, brackets: None -> not allowed
                        if meta.brackets.is_some() {
                            match parse_nested_args(inner, self.manager.clone()) {
                                Ok(args_vec) => {
                                    parsed_args = Some(args_vec.clone());
                                    validate_arg_count(
                                        &name,
                                        args_vec.len(),
                                        min_args,
                                        max_args,
                                        meta.args
                                            .as_ref()
                                            .map(|v| v.iter().any(|a| a.rest))
                                            .unwrap_or(false),
                                        &mut diagnostics,
                                        (start, last_idx),
                                        self.code,
                                    );
                                }
                                Err(_) => {
                                    diagnostics.push(Diagnostic {
                                        message: format!("Failed to parse args for `${}`", name),
                                        start,
                                        end: last_idx,
                                    });
                                }
                            }
                        } else {
                            // brackets: None means no brackets allowed
                            diagnostics.push(Diagnostic {
                                message: format!("${} does not accept brackets", name),
                                start,
                                end: last_idx,
                            });
                        }
                    } else if meta.brackets == Some(true) {
                        // Only error if brackets are required (Some(true))
                        // brackets: false (optional) or None (no brackets) don't need brackets
                        diagnostics.push(Diagnostic {
                            message: format!("${} expects brackets `[...]`", name),
                            start,
                            end: last_idx,
                        });
                    }

                    tokens.push(Token {
                        kind: TokenKind::FunctionName,
                        text: Box::leak(self.code[start..last_idx].to_string().into_boxed_str()),
                        start,
                        end: last_idx,
                    });

                    functions.push(ParsedFunction {
                        name: name.clone(),
                        matched: self.code[start..last_idx].to_string(),
                        args: parsed_args,
                        span: (start, last_idx),
                        silent,
                        negated,
                        count: None,
                        meta,
                    });
                } else {
                    diagnostics.push(Diagnostic {
                        message: format!("Unknown function `${}`", name),
                        start,
                        end: last_idx,
                    });
                    tokens.push(Token {
                        kind: TokenKind::Unknown,
                        text: Box::leak(self.code[start..last_idx].to_string().into_boxed_str()),
                        start,
                        end: last_idx,
                    });
                }
            }
        }

        // remaining text
        if last_idx < self.code.len() {
            tokens.push(Token {
                kind: TokenKind::Text,
                text: Box::leak(self.code[last_idx..].to_string().into_boxed_str()),
                start: last_idx,
                end: self.code.len(),
            });
        }

        ParseResult {
            tokens,
            diagnostics,
            functions,
        }
    }
}

fn compute_arg_counts(meta: &Function) -> (usize, usize) {
    let min = meta
        .args
        .as_ref()
        .map(|v| v.iter().filter(|a| a.required.unwrap_or(false)).count())
        .unwrap_or(0);

    let max = if meta
        .args
        .as_ref()
        .map(|v| v.iter().any(|a| a.rest))
        .unwrap_or(false)
    {
        usize::MAX
    } else {
        meta.args.as_ref().map(|v| v.len()).unwrap_or(0)
    };
    (min, max)
}

/// Find matching bracket for escape functions - does NOT respect backslash escapes.
/// This is used for $esc[...] and $escape[...] where we want to find the raw matching bracket.
fn find_matching_bracket_raw(code: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in code.char_indices().skip_while(|&(i, _)| i < open_idx) {
        if c == '[' {
            depth += 1;
        } else if c == ']' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Find matching brace for JavaScript expressions ${...}.
/// Handles nested braces properly.
fn find_matching_brace(code: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in code.char_indices().skip_while(|&(i, _)| i < open_idx) {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Find matching bracket respecting backslash escapes.
/// Escaped brackets (\[ and \]) are not counted.
/// Also skips over escape functions ($esc, $escape, $escapeCode) entirely.
fn find_matching_bracket(code: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    let bytes = code.as_bytes();
    let mut i = open_idx;

    while i < code.len() {
        let c = bytes[i] as char;

        // Check if this character is escaped by a backslash
        let is_esc = i > 0 && {
            let mut backslash_count = 0;
            let mut pos = i;
            while pos > 0 {
                pos -= 1;
                if bytes[pos] == b'\\' {
                    backslash_count += 1;
                } else {
                    break;
                }
            }
            backslash_count % 2 == 1
        };

        // Check if we're at the start of an escape function
        if !is_esc
            && c == '$'
            && let Some(escape_end) = find_escape_function_end(code, i)
        {
            // Skip the entire escape function including its closing bracket
            i = escape_end + 1;
            continue;
        }

        if !is_esc {
            if c == '[' {
                depth += 1;
            } else if c == ']' {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }

        i += 1;
    }

    None
}

fn parse_nested_args(
    input: &str,
    manager: Arc<MetadataManager>,
) -> Result<Vec<SmallVec<[ParsedArg; 8]>>, nom::Err<()>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut seen_separator = false;
    let mut first_char_escaped = false;
    let mut is_start_of_arg = true;
    let bytes = input.as_bytes();
    let mut idx = 0;

    while idx < input.len() {
        let c = bytes[idx] as char;

        // Check if we're at an escape function
        if c == '$' && depth == 0 {
            // Check if this $ starts an escape function
            let remaining = &input[idx..];
            if let Some(escape_end_relative) = find_escape_function_end(remaining, 0) {
                // Copy the entire escape function to current including the $esc[...] structure
                let escape_function = &remaining[..=escape_end_relative];
                current.push_str(escape_function);
                idx += escape_end_relative + 1;
                is_start_of_arg = false;
                continue;
            }
        }

        match c {
            '\\' => {
                // Check for backtick escape: \`
                if idx + 1 < input.len() && bytes[idx + 1] == b'`' {
                    current.push('`');
                    idx += 2;
                    is_start_of_arg = false;
                    continue;
                }
                // Check for double backslash escapes: \\$, \\;, \\[, \\], \\\\
                if idx + 2 < input.len() && bytes[idx + 1] == b'\\' {
                    let third = bytes[idx + 2] as char;
                    if matches!(third, '$' | '[' | ']' | ';' | '\\') {
                        if is_start_of_arg && third == '$' {
                            first_char_escaped = true;
                        }
                        current.push(third);
                        idx += 3; // Skip both backslashes and escaped char
                        is_start_of_arg = false;
                        continue;
                    }
                }
                // Not a recognized escape, keep the backslash
                current.push('\\');
                idx += 1;
            }
            '[' => {
                depth += 1;
                current.push(c);
                idx += 1;
            }
            ']' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(c);
                idx += 1;
            }
            ';' if depth == 0 => {
                seen_separator = true;
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    args.push(parse_single_arg(
                        trimmed,
                        manager.clone(),
                        first_char_escaped,
                    )?);
                } else {
                    args.push(smallvec![ParsedArg::Literal {
                        text: Cow::Owned(String::new())
                    }]);
                }
                current.clear();
                first_char_escaped = false;
                is_start_of_arg = true;
                idx += 1;
            }
            _ => {
                current.push(c);
                idx += 1;
            }
        }

        if c != ';' || depth != 0 {
            is_start_of_arg = false;
        }
    }

    // Always add the last argument if we saw a separator (e.g., "user;" should have 2 args)
    // or if there's any content remaining
    if seen_separator || !current.is_empty() {
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            args.push(parse_single_arg(
                trimmed,
                manager.clone(),
                first_char_escaped,
            )?);
        } else {
            args.push(smallvec![ParsedArg::Literal {
                text: Cow::Owned(String::new())
            }]);
        }
    }

    Ok(args)
}

fn parse_single_arg(
    input: &str,
    manager: Arc<MetadataManager>,
    force_literal: bool,
) -> Result<SmallVec<[ParsedArg; 8]>, nom::Err<()>> {
    if !force_literal && input.starts_with('$') {
        // Use new_internal to parse directly without code block extraction
        let parser = ForgeScriptParser::new_internal(manager.clone(), input);
        let res = parser.parse_internal();
        if let Some(f) = res.functions.first() {
            Ok(smallvec![ParsedArg::Function {
                func: Box::new(f.clone())
            }])
        } else {
            Ok(smallvec![ParsedArg::Literal {
                text: Cow::Owned(input.to_string())
            }])
        }
    } else {
        Ok(smallvec![ParsedArg::Literal {
            text: Cow::Owned(input.to_string())
        }])
    }
}

#[allow(clippy::too_many_arguments)]
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
            start: span.0,
            end: span.1,
        });
    } else if !has_rest && total > max {
        diagnostics.push(Diagnostic {
            message: format!("${} expects at most {} args, got {}", name, max, total),
            start: span.0,
            end: span.1,
        });
    }
}
