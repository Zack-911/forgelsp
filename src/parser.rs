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
use std::sync::Arc;

/// List of function name and argument index pairs that should skip enum validation.
/// Some functions might have dynamic enum-like arguments that aren't strictly defined
/// in the static metadata.
/// Argument index starts from 0 not 1
const ENUM_VALIDATION_EXCEPTIONS: &[(&str, usize); 1] = &[("color", 0)];

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

/// A token identified during the parsing process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The category of the token.
    pub kind: TokenKind,
    /// The text content of the token.
    pub text: String,
    /// Starting byte offset in the source.
    pub start: usize,
    /// Ending byte offset in the source.
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

/// The result of parsing a ForgeScript document.
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// List of tokens identified in the document.
    pub tokens: Vec<Token>,
    /// List of syntax errors or warnings found during parsing.
    pub diagnostics: Vec<Diagnostic>,
    /// List of successfully matched function calls.
    pub functions: Vec<ParsedFunction>,
}

/// Represents a single argument in a function call.
#[derive(Debug, Clone)]
pub enum ParsedArg {
    /// A literal string argument.
    Literal {
        /// The text content of the argument.
        text: String,
    },
    /// A nested function call argument.
    Function {
        #[allow(dead_code)]
        func: Box<ParsedFunction>,
    },
}

/// Map a position in the concatenated code string back to the original block.
/// Returns (block_index, offset_within_block).
/// Each block is separated by a newline (\n) in the concatenated string.
fn map_to_block(position: usize, block_lengths: &[usize]) -> (usize, usize) {
    let mut current_pos = 0;

    for (idx, &length) in block_lengths.iter().enumerate() {
        // Each block contributes: length + 1 (for the newline separator)
        // except the last one might not have consumed the newline yet
        let block_end = current_pos + length;

        if position < block_end {
            // Position falls within this block
            return (idx, position - current_pos);
        }

        // Move past this block and its newline separator
        current_pos = block_end + 1; // +1 for the '\n'
    }

    // If we're past all blocks, return the last block
    let last_idx = block_lengths.len().saturating_sub(1);
    let last_offset = if last_idx < block_lengths.len() {
        block_lengths[last_idx]
    } else {
        0
    };
    (last_idx, last_offset)
}

/// Check if a character at the given byte index is escaped.
/// For backtick: 1 backslash escapes it (\`)
/// For special chars ($, ;, [, ]): 2 backslashes escape it (\\$, \\;, etc.)
fn is_escaped(code: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 {
        return false;
    }

    // Validate that byte_idx is on a character boundary
    if !code.is_char_boundary(byte_idx) {
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

fn is_ignore_error_directive(code: &str, dollar_idx: usize) -> Option<usize> {
    let directive = "$c[fs@ignore-error]";
    if dollar_idx + directive.len() > code.len() {
        return None;
    }
    let rest = &code[dollar_idx..];

    if rest.starts_with(directive) {
        Some(dollar_idx + directive.len())
    } else {
        None
    }
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
        let mut lengths: Vec<usize> = Vec::new();
        let mut block_count = 0;

        let bytes = self.code.as_bytes();
        let mut i = 0;

        while i < self.code.len() {
            // Look for "code:" pattern
            if i + 5 <= self.code.len() && &self.code.as_bytes()[i..i + 5] == b"code:" {
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
                        lengths.push(content.len());
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
            // Map each position to the correct source block
            if !offsets.is_empty() {
                for diag in &mut result.diagnostics {
                    let (block_idx, offset_in_block) = map_to_block(diag.start, &lengths);
                    diag.start = offsets[block_idx] + offset_in_block;
                    let (block_idx_end, offset_in_block_end) = map_to_block(diag.end, &lengths);
                    diag.end = offsets[block_idx_end] + offset_in_block_end;
                }
                for func in &mut result.functions {
                    let (block_idx_start, offset_in_block_start) =
                        map_to_block(func.span.0, &lengths);
                    func.span.0 = offsets[block_idx_start] + offset_in_block_start;
                    let (block_idx_end, offset_in_block_end) = map_to_block(func.span.1, &lengths);
                    func.span.1 = offsets[block_idx_end] + offset_in_block_end;
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
        let mut ignore_next_line = false;
        let mut pending_ignore_next_line = false;

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
                            text: self.code[last_idx..idx].to_string(),
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
                                text: self.code[last_idx..idx].to_string(),
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

            if c == '\n' {
                ignore_next_line = pending_ignore_next_line;
                pending_ignore_next_line = false;
            }

            if c == '$' && !is_escaped(self.code, idx) {
                if let Some(end_idx) = is_ignore_error_directive(self.code, idx) {
                    pending_ignore_next_line = true;

                    tokens.push(Token {
                        kind: TokenKind::Text,
                        text: self.code[idx..end_idx].to_string(),
                        start: idx,
                        end: end_idx,
                    });

                    // Advance iterator to end_idx
                    while let Some(&(j, _)) = iter.peek() {
                        if j < end_idx {
                            iter.next();
                        } else {
                            break;
                        }
                    }

                    last_idx = end_idx;
                    continue;
                }
                // push previous text as a token
                if last_idx < idx {
                    tokens.push(Token {
                        kind: TokenKind::Text,
                        text: self.code[last_idx..idx].to_string(),
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
                            text: js_content.to_string(),
                            start,
                            end: last_idx,
                        });
                        continue;
                    }
                    if !ignore_next_line {
                        diagnostics.push(Diagnostic {
                            message: "Unclosed '{' for JavaScript expression `${...}`".to_string(),
                            start,
                            end: self.code.len(),
                        });
                    }
                    last_idx = self.code.len();
                    continue;
                }

                let mut silent = false;
                let mut negated = false;

                // Loop to handle multiple modifiers (e.g. $!#@[...])
                while let Some(&(_, next_c)) = iter.peek() {
                    if next_c == '!' {
                        silent = true;
                        iter.next();
                    } else if next_c == '#' {
                        negated = true;
                        iter.next();
                    } else if next_c == '@' {
                        let mut lookahead = iter.clone();
                        lookahead.next(); // consume '@'
                        if let Some(&(_, '[')) = lookahead.peek() {
                            iter.next(); // consume '@'
                            iter.next(); // consume '['
                            let mut depth = 1;
                            for (_, c) in iter.by_ref() {
                                if c == '[' {
                                    depth += 1;
                                } else if c == ']' {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                            }
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }

                // read function name
                let name_start_idx = if let Some(&(i, _)) = iter.peek() {
                    i
                } else {
                    self.code.len()
                };

                let mut name_chars = vec![];
                let mut full_name_end = name_start_idx;

                // Clone iterator to read name without consuming if we need to backtrack?
                // Actually we can consume, and if we have a suffix, we just emit it as text.
                while let Some(&(i, ch)) = iter.peek() {
                    if ch.is_alphanumeric() || ch == '_' {
                        name_chars.push(ch);
                        full_name_end = i + ch.len_utf8();
                        iter.next();
                    } else {
                        break;
                    }
                }
                let full_name = name_chars.iter().collect::<String>();

                // If name is empty (e.g. just `$!`), handle gracefully
                if full_name.is_empty() {
                    // Treat as text
                    last_idx = full_name_end;
                    continue;
                }

                // Check if this is an escape function ($esc or $escape)
                if is_escape_function(&full_name) {
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
                                text: escaped_content.to_string(),
                                start,
                                end: last_idx,
                            });
                            continue;
                        }
                        if !ignore_next_line {
                            diagnostics.push(Diagnostic {
                                message: format!(
                                    "Unclosed '[' for escape function `${full_name}`",
                                ),
                                start,
                                end: self.code.len(),
                            });
                        }
                        last_idx = self.code.len();
                        continue;
                    }
                    // $esc or $escape without brackets - treat as unknown function
                    if !ignore_next_line {
                        diagnostics.push(Diagnostic {
                            message: format!(
                                "${full_name} expects brackets `[...]` containing content to escape",
                            ),
                            start,
                            end: full_name_end,
                        });
                    }
                    tokens.push(Token {
                        kind: TokenKind::Unknown,
                        text: self.code[start..full_name_end].to_string(),
                        start,
                        end: full_name_end,
                    });
                    last_idx = full_name_end;
                    continue;
                }

                // Determine the actual function name to use
                let mut matched_function: Option<(String, Arc<Function>)> = None;
                let mut used_name_end = full_name_end;

                // Check for bracket lookahead
                let has_bracket = matches!(iter.peek(), Some(&(_, '[')));

                if has_bracket {
                    // Case 1: Bracketed call - must match exactly
                    let lookup_key = format!("${}", full_name);
                    if let Some(func) = self.manager.get_exact(&lookup_key) {
                        matched_function = Some((full_name.clone(), func));
                    }
                } else {
                    // Case 2: No bracket - find longest prefix match
                    let lookup_key = format!("${}", full_name);
                    // manager.get now returns Option<Arc<Function>> (exact match wrapper? No, wait)
                    // In metadata.rs, I changed get to return Option<Arc<Function>>?
                    if let Some((matched_name_with_prefix, func)) =
                        self.manager.get_with_match(&lookup_key)
                    {
                        // matched_name_with_prefix includes '$' e.g. "$ping"
                        // we need the name without '$'
                        let matched_name = matched_name_with_prefix
                            .strip_prefix('$')
                            .unwrap_or(&matched_name_with_prefix);

                        // Check if it's a valid prefix of our full_name
                        if full_name.to_lowercase().starts_with(matched_name) {
                            let correct_name = func.name.strip_prefix('$').unwrap_or(&func.name).to_string();
                            matched_function = Some((correct_name, func));
                            // Calculate where the matched name ends
                            let matched_len_bytes = matched_name.len();
                            used_name_end = name_start_idx + matched_len_bytes;
                        }
                    }
                }

                if let Some((name, meta)) = matched_function {
                    // A valid function was identified.
                    // If a prefix match occurred (e.g., matching '$ping' within '$pingms'), 
                    // the remaining suffix must be handled as a separate text token.

                    // Note: The iterator has already advanced past the full identifier.
                    // The function token spans from 'start' (including any modifiers like '$') 
                    // to 'used_name_end' (the end of the matched function name).

                    // 'name_start_idx' accounts for characters after modifiers. 
                    // Since 'used_name_end' represents the absolute end position of the 
                    // matched name, it serves as the correct boundary for the function token.

                    let token_end = used_name_end;

                    // parse args if any
                    let mut args_text: Option<&str> = None;
                    let mut args_start_offset = 0;

                    // Only check for brackets/arguments if the function name was an exact match.
                    // If a suffix exists (e.g., '$ping' matched in '$pingms'), the 'ms' suffix 
                    // is treated as literal text, which precludes argument parsing for the prefix.

                    // Note: The iterator has already consumed the full string (up to full_name_end).
                    // While we logically "rewind" to used_name_end to emit the function token, 
                    // the physical iterator remains ahead. A non-empty suffix effectively 
                    // signals that this match should be treated as a no-argument call.

                    let has_suffix = used_name_end < full_name_end;

                    if !has_suffix {
                        // We are at the end of the name, check for args
                        if let Some(&(i, '[')) = iter.peek() {
                            if let Some(end_idx) = find_matching_bracket(self.code, i) {
                                args_text = Some(&self.code[i + 1..end_idx]);
                                args_start_offset = i + 1;
                                while let Some(&(j, _)) = iter.peek() {
                                    if j <= end_idx {
                                        iter.next();
                                    } else {
                                        break;
                                    }
                                }
                                last_idx = end_idx + 1;
                            } else {
                                if !ignore_next_line {
                                    diagnostics.push(Diagnostic {
                                        message: format!("Unclosed '[' for function `${}`", name),
                                        start,
                                        end: self.code.len(),
                                    });
                                }
                                last_idx = self.code.len();
                            }
                        } else {
                            last_idx = token_end;
                        }
                    } else {
                        // Handle the suffix logic: The function call concludes at 'used_name_end'.
                        // Any characters between 'used_name_end' and 'full_name_end' (the suffix) 
                        // must be explicitly captured as text.

                        // Because the main loop's iterator 'iter' is already positioned at 
                        // 'full_name_end', simply updating 'last_idx' to 'used_name_end' would 
                        // cause the suffix to be skipped in the next iteration.

                        // To prevent data loss, we must emit the suffix as a text token immediately 
                        // and synchronize 'last_idx' with 'full_name_end'.
                        last_idx = full_name_end; // We have consumed up to here
                    }

                    let (min_args, max_args) = compute_arg_counts(&meta);
                    let mut parsed_args: Option<Vec<SmallVec<[ParsedArg; 8]>>> = None;

                    if let Some(inner) = args_text {
                        // ... args parsing logic ...
                        if meta.brackets.is_some() {
                            match parse_nested_args(
                                inner,
                                &self.manager,
                                &mut diagnostics,
                                args_start_offset,
                            ) {
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
                                        ignore_next_line,
                                    );

                                    if !ignore_next_line && let Some(meta_args) = &meta.args {
                                        validate_arg_enums(
                                            &name,
                                            &args_vec,
                                            meta_args,
                                            &self.manager,
                                            &mut diagnostics,
                                            start, // Use function start for now as we don't have better arg offsets
                                            self.code,
                                        );
                                    }
                                }
                                Err(_) => {
                                    if !ignore_next_line {
                                        diagnostics.push(Diagnostic {
                                            message: format!("Failed to parse args for `${name}`",),
                                            start,
                                            end: last_idx,
                                        });
                                    }
                                }
                            }
                        } else if !ignore_next_line {
                            diagnostics.push(Diagnostic {
                                message: format!("${} does not accept brackets", name),
                                start,
                                end: last_idx,
                            });
                        }
                    } else if meta.brackets == Some(true) && !ignore_next_line {
                        diagnostics.push(Diagnostic {
                            message: format!("${} expects brackets `[...]`", name),
                            start,
                            end: token_end,
                        });
                    }

                    tokens.push(Token {
                        kind: TokenKind::FunctionName,
                        text: self.code[start..token_end].to_string(),
                        start,
                        end: token_end,
                    });

                    if has_suffix {
                        tokens.push(Token {
                            kind: TokenKind::Text,
                            text: self.code[token_end..full_name_end].to_string(),
                            start: token_end,
                            end: full_name_end,
                        });
                    }

                    if !ignore_next_line {
                        functions.push(ParsedFunction {
                            name: meta.name.trim_start_matches('$').to_string(),
                            matched: self.code[start..token_end].to_string(),
                            args: parsed_args,
                            span: (start, if has_suffix { token_end } else { last_idx }),
                            silent,
                            negated,
                            count: None,
                            meta,
                        });
                    }
                } else {
                    // No match found (either exact match failed, or no prefix match)
                    if !ignore_next_line {
                        diagnostics.push(Diagnostic {
                            message: format!("Unknown function `${}`", full_name),
                            start,
                            end: full_name_end,
                        });
                    }
                    tokens.push(Token {
                        kind: TokenKind::Unknown,
                        text: self.code[start..full_name_end].to_string(),
                        start,
                        end: full_name_end,
                    });
                    last_idx = full_name_end;

                    // Check if the unknown function has brackets and parse them recursively
                    // This ensures that valid functions inside are found and brackets are balanced
                    if let Some(&(i, '[')) = iter.peek() {
                        if let Some(end_idx) = find_matching_bracket(self.code, i) {
                            // Found matching bracket, parse content
                            let content_start = i + 1;
                            let content_end = end_idx;
                            let content = &self.code[content_start..content_end];

                            // Emit '[' token
                            tokens.push(Token {
                                kind: TokenKind::Text,
                                text: self.code[i..i + 1].to_string(),
                                start: i,
                                end: i + 1,
                            });

                            // Parse content recursively
                            let parser =
                                ForgeScriptParser::new_internal(self.manager.clone(), content);
                            let res = parser.parse_internal();

                            // Adjust offsets and append tokens/diagnostics
                            for token in res.tokens {
                                tokens.push(Token {
                                    kind: token.kind,
                                    text: token.text,
                                    start: token.start + content_start,
                                    end: token.end + content_start,
                                });
                            }
                            for mut diag in res.diagnostics {
                                if !ignore_next_line {
                                    diag.start += content_start;
                                    diag.end += content_start;
                                    diagnostics.push(diag);
                                }
                            }
                            // Append functions found inside
                            for mut func in res.functions {
                                if !ignore_next_line {
                                    func.span.0 += content_start;
                                    func.span.1 += content_start;
                                    functions.push(func);
                                }
                            }

                            // Emit ']' token
                            tokens.push(Token {
                                kind: TokenKind::Text,
                                text: self.code[end_idx..end_idx + 1].to_string(),
                                start: end_idx,
                                end: end_idx + 1,
                            });

                            // Advance iterator past the closing bracket
                            while let Some(&(j, _)) = iter.peek() {
                                if j <= end_idx {
                                    iter.next();
                                } else {
                                    break;
                                }
                            }
                            last_idx = end_idx + 1;
                        } else {
                            // Unclosed bracket
                            if !ignore_next_line {
                                diagnostics.push(Diagnostic {
                                    message: format!(
                                        "Unclosed '[' for unknown function `${}`",
                                        full_name
                                    ),
                                    start: i,
                                    end: self.code.len(),
                                });
                            }
                            // Consume the '[' as text
                            tokens.push(Token {
                                kind: TokenKind::Text,
                                text: self.code[i..i + 1].to_string(),
                                start: i,
                                end: i + 1,
                            });
                            iter.next(); // consume '['
                            last_idx = i + 1;
                        }
                    }
                }
            }
        }

        // remaining text
        if last_idx < self.code.len() {
            tokens.push(Token {
                kind: TokenKind::Text,
                text: self.code[last_idx..].to_string(),
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

    // Use char_indices to ensure we're always on UTF-8 character boundaries
    for (i, c) in code.char_indices().skip_while(|&(idx, _)| idx < open_idx) {
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
        if !is_esc && c == '$' && find_escape_function_end(code, i).is_some() {
            // Skip the entire escape function - the for loop will continue past it
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
    }

    None
}

fn parse_nested_args(
    input: &str,
    manager: &Arc<MetadataManager>,
    diagnostics: &mut Vec<Diagnostic>,
    base_offset: usize,
) -> Result<Vec<SmallVec<[ParsedArg; 8]>>, nom::Err<()>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut seen_separator = false;
    let mut first_char_escaped = false;
    let mut is_start_of_arg = true;

    // Collect char positions for UTF-8 safe iteration
    let char_positions: Vec<(usize, char)> = input.char_indices().collect();
    let mut char_idx = 0;

    // Track where the current argument started in the input string (byte offset)
    let mut arg_start_offset = 0;

    while char_idx < char_positions.len() {
        let (byte_idx, c) = char_positions[char_idx];

        // Check if we're at an escape function
        if c == '$' && depth == 0 {
            // Check if this $ starts an escape function
            let remaining = &input[byte_idx..];
            if let Some(escape_end_relative) = find_escape_function_end(remaining, 0) {
                // Copy the entire escape function to current including the $esc[...] structure
                let escape_function = &remaining[..=escape_end_relative];
                current.push_str(escape_function);
                // Skip ahead to after the escape function
                let target_byte = byte_idx + escape_end_relative + 1;
                while char_idx < char_positions.len() && char_positions[char_idx].0 < target_byte {
                    char_idx += 1;
                }
                is_start_of_arg = false;
                continue;
            }
        }

        match c {
            '\\' => {
                // Check for backtick escape: \`
                if char_idx + 1 < char_positions.len() && char_positions[char_idx + 1].1 == '`' {
                    current.push('`');
                    char_idx += 2;
                    is_start_of_arg = false;
                    continue;
                }
                // Check for double backslash escapes: \\$, \\;, \\[, \\], \\\\
                if char_idx + 2 < char_positions.len() && char_positions[char_idx + 1].1 == '\\' {
                    let third = char_positions[char_idx + 2].1;
                    if matches!(third, '$' | '[' | ']' | ';' | '\\') {
                        if is_start_of_arg && third == '$' {
                            first_char_escaped = true;
                        }
                        current.push(third);
                        char_idx += 3; // Skip both backslashes and escaped char
                        is_start_of_arg = false;
                        continue;
                    }
                }
                // Not a recognized escape, keep the backslash
                current.push('\\');
                char_idx += 1;
            }
            '[' => {
                depth += 1;
                current.push(c);
                char_idx += 1;
            }
            ']' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(c);
                char_idx += 1;
            }
            ';' if depth == 0 => {
                seen_separator = true;
                let trimmed = current.trim();
                let leading_whitespace_len = current.len() - current.trim_start().len();
                if !trimmed.is_empty() {
                    // Calculate offset for the argument.
                    // We use arg_start_offset + base_offset + leading_whitespace_len.

                    args.push(parse_single_arg(
                        trimmed,
                        manager,
                        first_char_escaped,
                        diagnostics,
                        base_offset + arg_start_offset + leading_whitespace_len,
                    ));
                } else {
                    args.push(smallvec![ParsedArg::Literal {
                        text: String::new()
                    }]);
                }
                current.clear();
                first_char_escaped = false;
                is_start_of_arg = true;
                char_idx += 1;

                // Update start offset for next arg
                if char_idx < char_positions.len() {
                    arg_start_offset = char_positions[char_idx].0;
                } else {
                    arg_start_offset = input.len();
                }
            }
            _ => {
                current.push(c);
                char_idx += 1;
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
        let leading_whitespace_len = current.len() - current.trim_start().len();
        if !trimmed.is_empty() {
            args.push(parse_single_arg(
                trimmed,
                manager,
                first_char_escaped,
                diagnostics,
                base_offset + arg_start_offset + leading_whitespace_len,
            ));
        } else {
            args.push(smallvec![ParsedArg::Literal {
                text: String::new()
            }]);
        }
    }

    Ok(args)
}

fn parse_single_arg(
    input: &str,
    manager: &Arc<MetadataManager>,
    force_literal: bool,
    diagnostics: &mut Vec<Diagnostic>,
    base_offset: usize,
) -> SmallVec<[ParsedArg; 8]> {
    if !force_literal && input.starts_with('$') {
        // Use new_internal to parse directly without code block extraction
        let parser = ForgeScriptParser::new_internal((*manager).clone(), input);
        let res = parser.parse_internal();

        // Propagate diagnostics from the inner parser
        for mut diag in res.diagnostics {
            diag.start += base_offset;
            diag.end += base_offset;
            diagnostics.push(diag);
        }

        if let Some(f) = res.functions.first() {
            smallvec![ParsedArg::Function {
                func: Box::new(f.clone())
            }]
        } else {
            smallvec![ParsedArg::Literal {
                text: input.to_string()
            }]
        }
    } else {
        smallvec![ParsedArg::Literal {
            text: input.to_string()
        }]
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
    ignore_error: bool,
) {
    if ignore_error {
        return;
    }
    if total < min {
        diagnostics.push(Diagnostic {
            message: format!("${name} expects at least {min} args, got {total}"),
            start: span.0,
            end: span.1,
        });
    } else if !has_rest && total > max {
        diagnostics.push(Diagnostic {
            message: format!("${name} expects at most {max} args, got {total}"),
            start: span.0,
            end: span.1,
        });
    }
}

fn validate_arg_enums(
    name: &str,
    parsed_args: &[SmallVec<[ParsedArg; 8]>],
    meta_args: &[crate::metadata::Arg],
    manager: &Arc<MetadataManager>,
    diagnostics: &mut Vec<Diagnostic>,
    base_offset: usize,
    _source: &str,
) {
    for (i, arg_parts) in parsed_args.iter().enumerate() {
        // Skip validation if this specific argument is in the exception list
        if ENUM_VALIDATION_EXCEPTIONS.contains(&(name, i)) {
            continue;
        }

        // Find corresponding metadata arg
        // If function has rest args, the last metadata arg applies to all remaining parsed args
        let meta_arg = if i < meta_args.len() {
            &meta_args[i]
        } else if let Some(last) = meta_args.last() {
            if last.rest {
                last
            } else {
                continue; // Should have been caught by arg count validation
            }
        } else {
            continue;
        };

        // Check if this arg expects an enum
        let allowed_values = if let Some(enum_name) = &meta_arg.enum_name {
            if let Ok(enums) = manager.enums.read() {
                enums.get(enum_name).cloned()
            } else {
                None
            }
        } else {
            meta_arg.arg_enum.clone()
        };

        if let Some(values) = allowed_values {
            // Only validate if the argument is purely static (no functions inside)
            let is_static = arg_parts
                .iter()
                .all(|p| matches!(p, ParsedArg::Literal { .. }));

            if is_static {
                let mut full_text = String::new();
                for p in arg_parts {
                    if let ParsedArg::Literal { text } = p {
                        full_text.push_str(text);
                    }
                }

                // Case-sensitive check
                if !values.contains(&full_text) {
                    // Since we don't have exact offsets for individual args, we will mark the whole function call
                    // but add a specific message.
                    // TODO: Improve `parse_nested_args` to return spans.
                    diagnostics.push(Diagnostic {
                        message: format!(
                            "Invalid value `{}` for argument `{}` of `${}`. Expected one of: {:?}",
                            full_text, meta_arg.name, name, values
                        ),
                        start: base_offset, // This is not ideal, it points to start of args.
                        end: base_offset,   // We should probably pass the function span.
                    });
                }
            }
        }
    }
}
