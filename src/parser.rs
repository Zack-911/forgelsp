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
use crate::utils::{find_escape_function_end, find_matching_bracket, find_matching_bracket_raw, is_escaped, is_escape_function};
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
    pub args: Option<Vec<(SmallVec<[ParsedArg; 8]>, (usize, usize))>>,
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

impl ParsedFunction {
    pub fn offset_spans(&mut self, offset: usize) {
        self.span.0 += offset;
        self.span.1 += offset;
        if let Some(args) = &mut self.args {
            for (arg_parts, span) in args {
                span.0 += offset;
                span.1 += offset;
                for part in arg_parts {
                    part.offset_spans(offset);
                }
            }
        }
    }
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

impl ParsedArg {
    pub fn offset_spans(&mut self, offset: usize) {
        if let ParsedArg::Function { func } = self {
            func.offset_spans(offset);
        }
    }
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
            if self.handle_backslash_escape(&mut iter, idx, c, &mut last_idx, &mut tokens) {
                continue;
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
                    if self.handle_js_expression(
                        &mut iter,
                        start,
                        brace_idx,
                        &mut last_idx,
                        &mut tokens,
                        &mut diagnostics,
                        ignore_next_line,
                    ) {
                        continue;
                    }
                }

                let (silent, negated) = self.handle_modifiers(&mut iter);

                // read function name
                let (full_name, name_start_idx, full_name_end) = self.extract_function_name(&mut iter);

                // If name is empty (e.g. just `$!`), handle gracefully
                if full_name.is_empty() {
                    // Treat as text
                    last_idx = full_name_end;
                    continue;
                }

                // Check if this is an escape function ($esc or $escape)
                if self.handle_escape_function(
                    &mut iter,
                    start,
                    &full_name,
                    full_name_end,
                    &mut last_idx,
                    &mut tokens,
                    &mut diagnostics,
                    ignore_next_line,
                ) {
                    continue;
                }

                // Determine the actual function name to use
                let (matched_function, used_name_end) = self.find_matched_function(
                    &mut iter,
                    &full_name,
                    name_start_idx,
                    full_name_end,
                );

                if let Some((name, meta)) = matched_function {
                    self.handle_matched_function(
                        &mut iter,
                        name,
                        meta,
                        start,
                        used_name_end,
                        full_name_end,
                        silent,
                        negated,
                        &mut last_idx,
                        &mut tokens,
                        &mut diagnostics,
                        &mut functions,
                        ignore_next_line,
                    );
                } else {
                    self.handle_unknown_function_call(
                        &mut iter,
                        &full_name,
                        full_name_end,
                        start,
                        &mut last_idx,
                        &mut tokens,
                        &mut diagnostics,
                        &mut functions,
                        ignore_next_line,
                    );
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

    fn handle_backslash_escape(
        &self,
        iter: &mut std::iter::Peekable<std::str::CharIndices>,
        idx: usize,
        c: char,
        last_idx: &mut usize,
        tokens: &mut Vec<Token>,
    ) -> bool {
        if c != '\\' { return false; }
        
        let Some(&(_next_idx, next_c)) = iter.peek() else { return false; };

        // Check for backtick escape: \`
        if next_c == '`' {
            if *last_idx < idx {
                tokens.push(Token {
                    kind: TokenKind::Text,
                    text: self.code[*last_idx..idx].to_string(),
                    start: *last_idx,
                    end: idx,
                });
            }
            iter.next(); // consume the backtick
            *last_idx = idx; // Start from backslash
            return true;
        }

        // Check for double backslash escapes: \\$, \\;, \\[, \\]
        if next_c == '\\' {
            let mut lookahead = iter.clone();
            lookahead.next(); // skip second backslash
            if let Some(&(_third_idx, third_c)) = lookahead.peek() 
                && matches!(third_c, '$' | '[' | ']' | ';' | '\\')
            {
                if *last_idx < idx {
                    tokens.push(Token {
                        kind: TokenKind::Text,
                        text: self.code[*last_idx..idx].to_string(),
                        start: *last_idx,
                        end: idx,
                    });
                }
                iter.next(); // consume second backslash
                iter.next(); // consume escaped character
                *last_idx = idx; // Start from first backslash
                return true;
            }
        }
        
        false
    }

    fn handle_js_expression(
        &self,
        iter: &mut std::iter::Peekable<std::str::CharIndices>,
        start: usize,
        brace_idx: usize,
        last_idx: &mut usize,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
        ignore_next_line: bool,
    ) -> bool {
        let Some(end_idx) = find_matching_brace(self.code, brace_idx) else {
            if !ignore_next_line {
                diagnostics.push(Diagnostic {
                    message: "Unclosed '{' for JavaScript expression `${...}`".to_string(),
                    start,
                    end: self.code.len(),
                });
            }
            *last_idx = self.code.len();
            return true;
        };

        let js_content = &self.code[brace_idx + 1..end_idx];
        while let Some(&(j, _)) = iter.peek() {
            if j <= end_idx { iter.next(); } else { break; }
        }
        *last_idx = end_idx + 1;

        tokens.push(Token {
            kind: TokenKind::JavaScript,
            text: js_content.to_string(),
            start,
            end: *last_idx,
        });
        true
    }

    fn handle_escape_function(
        &self,
        iter: &mut std::iter::Peekable<std::str::CharIndices>,
        start: usize,
        full_name: &str,
        full_name_end: usize,
        last_idx: &mut usize,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
        ignore_next_line: bool,
    ) -> bool {
        if !is_escape_function(full_name) { return false; }

        if let Some(&(bracket_idx, '[')) = iter.peek() {
            if let Some(end_idx) = find_matching_bracket_raw(self.code.as_bytes(), bracket_idx) {
                let escaped_content = &self.code[bracket_idx + 1..end_idx];
                while let Some(&(j, _)) = iter.peek() {
                    if j <= end_idx { iter.next(); } else { break; }
                }
                *last_idx = end_idx + 1;
                tokens.push(Token {
                    kind: TokenKind::Escaped,
                    text: escaped_content.to_string(),
                    start,
                    end: *last_idx,
                });
                return true;
            }
            if !ignore_next_line {
                diagnostics.push(Diagnostic {
                    message: format!("Unclosed '[' for escape function `${full_name}`"),
                    start,
                    end: self.code.len(),
                });
            }
            *last_idx = self.code.len();
            return true;
        }

        if !ignore_next_line {
            diagnostics.push(Diagnostic {
                message: format!("${full_name} expects brackets `[...]` containing content to escape"),
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
        *last_idx = full_name_end;
        true
    }

    fn handle_modifiers(
        &self,
        iter: &mut std::iter::Peekable<std::str::CharIndices>,
    ) -> (bool, bool) {
        let mut silent = false;
        let mut negated = false;

        while let Some(&(_, next_c)) = iter.peek() {
            match next_c {
                '!' => {
                    silent = true;
                    iter.next();
                }
                '#' => {
                    negated = true;
                    iter.next();
                }
                '@' => {
                    let mut lookahead = iter.clone();
                    lookahead.next(); // consume '@'
                    if let Some(&(bracket_idx, '[')) = lookahead.peek() {
                        iter.next(); // consume '@'
                        iter.next(); // consume '['
                        if let Some(end_idx) = find_matching_bracket_raw(self.code.as_bytes(), bracket_idx) {
                            while let Some(&(j, _)) = iter.peek() {
                                if j <= end_idx { iter.next(); } else { break; }
                            }
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        (silent, negated)
    }

    fn extract_function_name(
        &self,
        iter: &mut std::iter::Peekable<std::str::CharIndices>,
    ) -> (String, usize, usize) {
        let name_start_idx = iter.peek().map(|&(i, _)| i).unwrap_or(self.code.len());
        let mut name_chars = vec![];
        let mut full_name_end = name_start_idx;

        while let Some(&(i, ch)) = iter.peek() {
            if ch.is_alphanumeric() || ch == '_' {
                name_chars.push(ch);
                full_name_end = i + ch.len_utf8();
                iter.next();
            } else {
                break;
            }
        }
        (name_chars.iter().collect(), name_start_idx, full_name_end)
    }

    fn find_matched_function(
        &self,
        iter: &mut std::iter::Peekable<std::str::CharIndices>,
        full_name: &str,
        name_start_idx: usize,
        full_name_end: usize,
    ) -> (Option<(String, Arc<Function>)>, usize) {
        let has_bracket = matches!(iter.peek(), Some(&(_, '[')));
        let lookup_key = format!("${}", full_name);

        if has_bracket {
            if let Some(func) = self.manager.get_exact(&lookup_key) {
                let correct_name = func.name.strip_prefix('$').unwrap_or(&func.name).to_string();
                return (Some((correct_name, func)), full_name_end);
            }
        } else if let Some((matched_name_with_prefix, func)) = self.manager.get_with_match(&lookup_key) {
            let matched_name = matched_name_with_prefix.strip_prefix('$').unwrap_or(&matched_name_with_prefix);
            if full_name.to_lowercase().starts_with(matched_name) {
                let correct_name = func.name.strip_prefix('$').unwrap_or(&func.name).to_string();
                let used_name_end = name_start_idx + matched_name.len();
                return (Some((correct_name, func)), used_name_end);
            }
        }

        (None, full_name_end)
    }

    fn handle_unknown_function_call(
        &self,
        iter: &mut std::iter::Peekable<std::str::CharIndices>,
        full_name: &str,
        full_name_end: usize,
        start: usize,
        last_idx: &mut usize,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
        functions: &mut Vec<ParsedFunction>,
        ignore_next_line: bool,
    ) {
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
        *last_idx = full_name_end;

        if let Some(&(i, '[')) = iter.peek() {
            if let Some(end_idx) = find_matching_bracket(self.code, i) {
                let content_start = i + 1;
                let content = &self.code[content_start..end_idx];

                if !ignore_next_line {
                    // Create a dummy meta for unknown functions to allow folding/hover
                    let dummy_meta = Arc::new(crate::metadata::Function {
                        name: format!("${full_name}"),
                        description: format!("Unknown function `${full_name}`"),
                        brackets: Some(true),
                        ..Default::default()
                    });

                    functions.push(ParsedFunction {
                        name: full_name.to_string(),
                        matched: full_name.to_string(),
                        args: None, // We'll parse nested functions but not structure the unknown's args
                        span: (start, end_idx + 1),
                        silent: false,
                        negated: false,
                        count: None,
                        meta: dummy_meta,
                    });
                }

                tokens.push(Token {
                    kind: TokenKind::Text,
                    text: "[".to_string(),
                    start: i,
                    end: content_start,
                });

                let parser = ForgeScriptParser::new_internal(self.manager.clone(), content);
                let res = parser.parse_internal();

                for token in res.tokens {
                    tokens.push(Token {
                        kind: token.kind,
                        text: token.text,
                        start: token.start + content_start,
                        end: token.end + content_start,
                    });
                }
                if !ignore_next_line {
                    for mut diag in res.diagnostics {
                        diag.start += content_start;
                        diag.end += content_start;
                        diagnostics.push(diag);
                    }
                    for mut func in res.functions {
                        func.offset_spans(content_start);
                        functions.push(func);
                    }
                }

                tokens.push(Token {
                    kind: TokenKind::Text,
                    text: "]".to_string(),
                    start: end_idx,
                    end: end_idx + 1,
                });

                while let Some(&(j, _)) = iter.peek() {
                    if j <= end_idx { iter.next(); } else { break; }
                }
                *last_idx = end_idx + 1;
            } else if !ignore_next_line {
                diagnostics.push(Diagnostic {
                    message: format!("Unclosed '[' for unknown function `${}`", full_name),
                    start: i,
                    end: self.code.len(),
                });
                tokens.push(Token {
                    kind: TokenKind::Text,
                    text: "[".to_string(),
                    start: i,
                    end: i + 1,
                });
                iter.next();
                *last_idx = i + 1;
            }
        }
    }

    fn handle_matched_function(
        &self,
        iter: &mut std::iter::Peekable<std::str::CharIndices>,
        name: String,
        meta: Arc<Function>,
        start: usize,
        used_name_end: usize,
        full_name_end: usize,
        silent: bool,
        negated: bool,
        last_idx: &mut usize,
        tokens: &mut Vec<Token>,
        diagnostics: &mut Vec<Diagnostic>,
        functions: &mut Vec<ParsedFunction>,
        ignore_next_line: bool,
    ) {
        let token_end = used_name_end;
        let mut args_text: Option<&str> = None;
        let mut args_start_offset = 0;
        let has_suffix = used_name_end < full_name_end;

        if !has_suffix {
            if let Some(&(i, '[')) = iter.peek() {
                if let Some(end_idx) = find_matching_bracket(self.code, i) {
                    args_text = Some(&self.code[i + 1..end_idx]);
                    args_start_offset = i + 1;
                    while let Some(&(j, _)) = iter.peek() {
                        if j <= end_idx { iter.next(); } else { break; }
                    }
                    *last_idx = end_idx + 1;
                } else {
                    if !ignore_next_line {
                        diagnostics.push(Diagnostic {
                            message: format!("Unclosed '[' for function `${}`", name),
                            start,
                            end: self.code.len(),
                        });
                    }
                    *last_idx = self.code.len();
                }
            } else {
                *last_idx = token_end;
            }
        } else {
            *last_idx = full_name_end;
        }

        let (min_args, max_args) = compute_arg_counts(&meta);
        let mut parsed_args: Option<Vec<(SmallVec<[ParsedArg; 8]>, (usize, usize))>> = None;

        if let Some(inner) = args_text {
            if meta.brackets.is_some() {
                match parse_nested_args(inner, &self.manager, diagnostics, functions, args_start_offset) {
                    Ok(args_vec) => {
                        parsed_args = Some(args_vec.clone());
                        validate_arg_count(
                            &name,
                            args_vec.len(),
                            min_args,
                            max_args,
                            meta.args.as_ref().map(|v| v.iter().any(|a| a.rest)).unwrap_or(false),
                            diagnostics,
                            (start, *last_idx),
                            self.code,
                            ignore_next_line,
                        );

                        if !ignore_next_line && let Some(meta_args) = &meta.args {
                            validate_arg_enums(&name, &args_vec, meta_args, &self.manager, diagnostics, self.code);
                        }
                    }
                    Err(_) => {
                        if !ignore_next_line {
                            diagnostics.push(Diagnostic {
                                message: format!("Failed to parse args for `${name}`"),
                                start,
                                end: *last_idx,
                            });
                        }
                    }
                }
            } else if !ignore_next_line {
                diagnostics.push(Diagnostic {
                    message: format!("${} does not accept brackets", name),
                    start,
                    end: *last_idx,
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
                span: (start, if has_suffix { token_end } else { *last_idx }),
                silent,
                negated,
                count: None,
                meta,
            });
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


fn parse_nested_args(
    input: &str,
    manager: &Arc<MetadataManager>,
    diagnostics: &mut Vec<Diagnostic>,
    functions: &mut Vec<ParsedFunction>,
    base_offset: usize,
) -> Result<Vec<(SmallVec<[ParsedArg; 8]>, (usize, usize))>, nom::Err<()>> {
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

        // Handle escape functions ($esc[...], etc.)
        if let Some(jump) = try_handle_escape_function_in_args(input, byte_idx, depth, &mut current) {
            char_idx += jump;
            is_start_of_arg = false;
            continue;
        }

        // Handle backslash escapes
        if c == '\\' {
            if let Some(jump) = try_handle_backslash_escape_in_args(
                &char_positions,
                char_idx,
                is_start_of_arg,
                &mut current,
                &mut first_char_escaped,
            ) {
                char_idx += jump;
                is_start_of_arg = false;
                continue;
            }
        }

        match c {
            '[' => {
                // Only increment depth if this bracket is part of a function call
                // Note: is_function_call_bracket expects full code, but here we have just the args text.
                // We can't easily use is_function_call_bracket here without passing the full text and adjusted offset.
                // Alternatively, we can check if it's preceded by $name in the 'input' string.
                if !is_escaped_bracket_in_args(&char_positions, char_idx) && 
                   (is_function_call_bracket_sub(input, byte_idx))
                {
                    depth += 1;
                }
                current.push(c);
                char_idx += 1;
            }
            ']' => {
                // Only decrement depth if this bracket is not escaped
                if depth > 0 && !is_escaped_bracket_in_args(&char_positions, char_idx) {
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
                    let arg_offset = base_offset + arg_start_offset + leading_whitespace_len;
                    let arg_len = trimmed.len();

                    args.push((
                        parse_single_arg(
                            trimmed,
                            manager,
                            first_char_escaped,
                            diagnostics,
                            functions,
                            arg_offset,
                        ),
                        (arg_offset, arg_offset + arg_len),
                    ));
                } else {
                    args.push((
                        smallvec![ParsedArg::Literal {
                            text: String::new()
                        }],
                        (base_offset + arg_start_offset, base_offset + arg_start_offset),
                    ));
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
            let arg_offset = base_offset + arg_start_offset + leading_whitespace_len;
            let arg_len = trimmed.len();
            args.push((
                parse_single_arg(
                    trimmed,
                    manager,
                    first_char_escaped,
                    diagnostics,
                    functions,
                    arg_offset,
                ),
                (arg_offset, arg_offset + arg_len),
            ));
        } else {
            args.push((
                smallvec![ParsedArg::Literal {
                    text: String::new()
                }],
                (base_offset + arg_start_offset, base_offset + arg_start_offset),
            ));
        }
    }

    Ok(args)
}

/// Checks if a bracket at the given position is escaped (preceded by exactly two backslashes).
/// This is used during structural bracket matching to ensure escaped brackets don't affect depth.
/// Helper for parse_nested_args to check if a '[' in the args string is a function call.
fn is_function_call_bracket_sub(input: &str, bracket_idx: usize) -> bool {
    if bracket_idx == 0 || input.as_bytes().get(bracket_idx) != Some(&b'[') {
        return false;
    }

    let mut i = bracket_idx;
    let bytes = input.as_bytes();
    
    // Skip modifiers
    while i > 0 && matches!(bytes[i - 1], b'!' | b'#' | b'@' | b']') {
        if bytes[i - 1] == b']' {
            let mut depth = 0;
            let mut found = false;
            while i > 0 {
                i -= 1;
                if bytes[i] == b']' {
                    depth += 1;
                } else if bytes[i] == b'[' {
                    depth -= 1;
                    if depth == 0 {
                        found = true;
                        break;
                    }
                }
            }
            if !found || i == 0 || bytes[i - 1] != b'@' {
                return false;
            }
            i -= 1;
        } else {
            i -= 1;
        }
    }

    let _name_end = i;
    while i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_') {
        i -= 1;
    }
    
    // In args, we don't necessarily have $ at the start if it was escaped? 
    // Actually, $function[...] ALWAYS starts with $.
    
    i > 0 && bytes[i - 1] == b'$' && (i == 1 || bytes[i - 2] != b'\\')
}


pub(crate) fn is_escaped_bracket_in_args(char_positions: &[(usize, char)], char_idx: usize) -> bool {
    // Check if current char is [ or ]
    if char_idx >= char_positions.len() {
        return false;
    }
    let (_byte_idx, c) = char_positions[char_idx];
    if c != '[' && c != ']' {
        return false;
    }
    
    // Check if preceded by exactly two backslashes (ForgeScript escape: \\[ or \\])
    if char_idx < 2 {
        return false;
    }
    
    if let (Some(&(_, c1)), Some(&(_, c2))) = 
        (char_positions.get(char_idx - 2), char_positions.get(char_idx - 1)) 
    {
        c1 == '\\' && c2 == '\\'
    } else {
        false
    }
}


fn try_handle_escape_function_in_args(
    input: &str,
    byte_idx: usize,
    depth: i32,
    current: &mut String,
) -> Option<usize> {
    if input.as_bytes().get(byte_idx) == Some(&b'$') && depth == 0 {
        if let Some(escape_end_relative) = find_escape_function_end(&input[byte_idx..], 0) {
            current.push_str(&input[byte_idx..=byte_idx + escape_end_relative]);
            return Some(escape_end_relative + 1);
        }
    }
    None
}

fn try_handle_backslash_escape_in_args(
    char_positions: &[(usize, char)],
    char_idx: usize,
    _is_start_of_arg: bool,
    current: &mut String,
    _first_char_escaped: &mut bool,
) -> Option<usize> {
    // Check for backtick escape: \`
    if let Some(&(_, next_c)) = char_positions.get(char_idx + 1) {
        if next_c == '`' {
            current.push('\\');
            current.push('`');
            return Some(2);
        }
    }
    
    // Check for double backslash escapes: \\$, \\;, \\[, \\], \\\\
    if let Some(&(_, '\\')) = char_positions.get(char_idx + 1) {
        if let Some(&(_, third)) = char_positions.get(char_idx + 2) {
            if matches!(third, '$' | '[' | ']' | ';' | '\\') {
                current.push('\\');
                current.push('\\');
                current.push(third);
                return Some(3);
            }
        }
    }
    
    current.push('\\');
    Some(1)
}

fn parse_single_arg(
    input: &str,
    manager: &Arc<MetadataManager>,
    force_literal: bool,
    diagnostics: &mut Vec<Diagnostic>,
    functions: &mut Vec<ParsedFunction>,
    base_offset: usize,
) -> SmallVec<[ParsedArg; 8]> {
    if !force_literal && input.starts_with('$') {
        // Use new_internal to parse directly without code block extraction
        let parser = ForgeScriptParser::new_internal((*manager).clone(), input);
        let mut res = parser.parse_internal();

        // Propagate diagnostics from the inner parser
        for mut diag in res.diagnostics {
            diag.start += base_offset;
            diag.end += base_offset;
            diagnostics.push(diag);
        }

        for func in &mut res.functions {
            func.offset_spans(base_offset);
            functions.push(func.clone());
        }

        if let Some(f) = res.functions.first() {
            let mut func = f.clone();
            func.offset_spans(base_offset);
            smallvec![ParsedArg::Function {
                func: Box::new(func)
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
    parsed_args: &[(SmallVec<[ParsedArg; 8]>, (usize, usize))],
    meta_args: &[crate::metadata::Arg],
    manager: &Arc<MetadataManager>,
    diagnostics: &mut Vec<Diagnostic>,
    _source: &str,
) {
    for (i, (arg_parts, span)) in parsed_args.iter().enumerate() {
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
                    diagnostics.push(Diagnostic {
                        message: format!(
                            "Invalid value `{}` for argument `{}` of `${}`. Expected one of: {:?}",
                            full_text, meta_arg.name, name, values
                        ),
                        start: span.0,
                        end: span.1,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::MetadataManager;
    use std::sync::Arc;

    #[test]
    fn test_parse_nested_args_spans() {
        let manager = Arc::new(MetadataManager::new_test());
        let mut diagnostics = Vec::new();
        let mut functions = Vec::new();
        let input = "abc; $func[def]; ghi";
        let base_offset = 10;
        
        let result = parse_nested_args(input, &manager, &mut diagnostics, &mut functions, base_offset).unwrap();
        
        assert_eq!(result.len(), 3);
        
        // "abc"
        assert_eq!(result[0].1, (10, 13));
        
        // "$func[def]"
        assert_eq!(result[1].1, (15, 25));
        
        // "ghi"
        assert_eq!(result[2].1, (27, 30));
    }

    #[test]
    fn test_offset_spans() {
        let func_meta = Arc::new(crate::metadata::Function {
            name: "$test".to_string(),
            ..Default::default()
        });
        
        let mut func = ParsedFunction {
            name: "test".to_string(),
            matched: "$test".to_string(),
            args: Some(vec![(
                smallvec![ParsedArg::Literal { text: "arg".to_string() }],
                (5, 8)
            )]),
            span: (0, 10),
            silent: false,
            negated: false,
            count: None,
            meta: func_meta,
        };
        
        func.offset_spans(100);
        
        assert_eq!(func.span, (100, 110));
        assert_eq!(func.args.unwrap()[0].1, (105, 108));
    }
}
