//! Implements a robust recursive descent parser for ForgeScript.
//!
//! This module handles the tokenization and structural analysis of ForgeScript code,
//! including nested function calls, complex argument partitioning, and diagnostic generation.
//! It validates function calls against metadata while respecting various modifiers like silenced calls.

use crate::metadata::{Function, MetadataManager};
use crate::utils::{find_escape_function_end, find_matching_bracket, find_matching_bracket_raw, is_escaped, is_escape_function};
use smallvec::{SmallVec, smallvec};
use std::sync::Arc;

/// Functions that bypass enum validation for specific arguments.
const ENUM_VALIDATION_EXCEPTIONS: &[(&str, usize); 1] = &[("color", 0)];

/// Captures syntax errors or warnings during the parsing phase.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub start: usize,
    pub end: usize,
}

/// Token types recognized by the ForgeScript scanner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Text,
    FunctionName,
    Escaped,
    JavaScript,
    Unknown,
}

/// A scanned fragment of source code with corresponding metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// A successfully identified ForgeScript function call.
#[derive(Debug, Clone)]
pub struct ParsedFunction {
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
    /// Adjusts the byte offsets for this function and its arguments.
    pub fn offset_spans(&mut self, offset: usize) {
        self.span.0 += offset;
        self.span.1 += offset;
        if let Some(args) = &mut self.args {
            for (arg_parts, span) in args {
                span.0 += offset;
                span.1 += offset;
                for part in arg_parts { part.offset_spans(offset); }
            }
        }
    }
}

/// Result of a complete parsing pass.
#[derive(Debug, Clone)]
pub struct ParseResult {
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
    pub functions: Vec<ParsedFunction>,
}

/// Represent an individual argument, which can be a literal or a nested function.
#[derive(Debug, Clone)]
pub enum ParsedArg {
    Literal { text: String },
    Function { func: Box<ParsedFunction> },
}

impl ParsedArg {
    pub fn offset_spans(&mut self, offset: usize) {
        if let ParsedArg::Function { func } = self { func.offset_spans(offset); }
    }
}

/// Maps a global character position back to a specific code block and local offset.
fn map_to_block(position: usize, block_lengths: &[usize]) -> (usize, usize) {
    let mut current_pos = 0;
    for (idx, &length) in block_lengths.iter().enumerate() {
        let block_end = current_pos + length;
        if position < block_end { return (idx, position - current_pos); }
        current_pos = block_end + 1; // Includes newline separator
    }
    let last_idx = block_lengths.len().saturating_sub(1);
    (last_idx, if last_idx < block_lengths.len() { block_lengths[last_idx] } else { 0 })
}

/// Checks for the presence of a diagnostic suppression directive.
fn is_ignore_error_directive(code: &str, dollar_idx: usize) -> Option<usize> {
    let directive = "$c[fs@ignore-error]";
    if code[dollar_idx..].starts_with(directive) { Some(dollar_idx + directive.len()) }
    else { None }
}

/// Main parser for ForgeScript files, capable of extracting code blocks and parsing them.
pub struct ForgeScriptParser<'a> {
    manager: Arc<MetadataManager>,
    code: &'a str,
    skip_extraction: bool,
}

impl<'a> ForgeScriptParser<'a> {
    /// Constructs a parser for a raw source file.
    pub fn new(manager: Arc<MetadataManager>, code: &'a str) -> Self {
        Self { manager, code, skip_extraction: false }
    }

    /// Constructs an internal parser for extracted code fragments.
    fn new_internal(manager: Arc<MetadataManager>, code: &'a str) -> Self {
        Self { manager, code, skip_extraction: true }
    }

    /// Orchestrates the parsing process, including code block extraction if necessary.
    pub fn parse(&self) -> ParseResult {
        if self.skip_extraction { return self.parse_internal(); }

        let mut code_to_parse = String::new();
        let mut offsets = Vec::new();
        let mut lengths = Vec::new();
        let bytes = self.code.as_bytes();
        let mut i = 0;

        while i < self.code.len() {
            if i + 5 <= self.code.len() && &bytes[i..i + 5] == b"code:" {
                let mut j = i + 5;
                while j < bytes.len() && bytes[j].is_ascii_whitespace() { j += 1; }
                if j < bytes.len() && bytes[j] == b'`' {
                    j += 1;
                    let content_start = j;
                    let mut found_end = false;
                    while j < bytes.len() {
                        if bytes[j] == b'\\' && j + 1 < bytes.len() { j += 2; continue; }
                        if bytes[j] == b'`' { found_end = true; break; }
                        j += 1;
                    }
                    if found_end {
                        let content = &self.code[content_start..j];
                        offsets.push(content_start);
                        lengths.push(content.len());
                        code_to_parse.push_str(content);
                        code_to_parse.push('\n');
                        i = j + 1;
                        continue;
                    }
                }
            }
            i += 1;
        }

        if !offsets.is_empty() {
            let mut result = ForgeScriptParser::new_internal(self.manager.clone(), &code_to_parse).parse();
            for diag in &mut result.diagnostics {
                let (bi, bo) = map_to_block(diag.start, &lengths);
                diag.start = offsets[bi] + bo;
                let (ei, eo) = map_to_block(diag.end, &lengths);
                diag.end = offsets[ei] + eo;
            }
            for func in &mut result.functions {
                let (bi, bo) = map_to_block(func.span.0, &lengths);
                func.span.0 = offsets[bi] + bo;
                let (ei, eo) = map_to_block(func.span.1, &lengths);
                func.span.1 = offsets[ei] + eo;
            }
            return result;
        }
        ParseResult { tokens: Vec::new(), diagnostics: Vec::new(), functions: Vec::new() }
    }

    /// Primary parsing logic for a flattened stream of ForgeScript code.
    fn parse_internal(&self) -> ParseResult {
        let mut tokens = Vec::new();
        let mut diagnostics = Vec::new();
        let mut functions = Vec::new();
        let mut iter = self.code.char_indices().peekable();
        let mut last_idx = 0;
        let mut ignore_next_line = false;
        let mut pending_ignore_next_line = false;

        while let Some((idx, c)) = iter.next() {
            if self.handle_backslash_escape(&mut iter, idx, c, &mut last_idx, &mut tokens) { continue; }
            if c == '\n' { ignore_next_line = pending_ignore_next_line; pending_ignore_next_line = false; }
            if c == '$' && !is_escaped(self.code, idx) {
                if let Some(end_idx) = is_ignore_error_directive(self.code, idx) {
                    pending_ignore_next_line = true;
                    tokens.push(Token { kind: TokenKind::Text, text: self.code[idx..end_idx].to_string(), start: idx, end: end_idx });
                    while let Some(&(j, _)) = iter.peek() { if j < end_idx { iter.next(); } else { break; } }
                    last_idx = end_idx; continue;
                }
                if last_idx < idx { tokens.push(Token { kind: TokenKind::Text, text: self.code[last_idx..idx].to_string(), start: last_idx, end: idx }); }
                let start = idx;
                if let Some(&(brace_idx, '{')) = iter.peek() {
                    if self.handle_js_expression(&mut iter, start, brace_idx, &mut last_idx, &mut tokens, &mut diagnostics, ignore_next_line) { continue; }
                }

                let (silent, negated) = self.handle_modifiers(&mut iter);
                let (full_name, name_start, name_end) = self.extract_function_name(&mut iter);
                if full_name.is_empty() { last_idx = name_end; continue; }
                if self.handle_escape_function(&mut iter, start, &full_name, name_end, &mut last_idx, &mut tokens, &mut diagnostics, ignore_next_line) { continue; }

                let (matched, used_end) = self.find_matched_function(&mut iter, &full_name, name_start, name_end);
                if let Some((name, meta)) = matched {
                    self.handle_matched_function(&mut iter, name, meta, start, used_end, name_end, silent, negated, &mut last_idx, &mut tokens, &mut diagnostics, &mut functions, ignore_next_line);
                } else {
                    self.handle_unknown_function_call(&mut iter, &full_name, name_end, start, &mut last_idx, &mut tokens, &mut diagnostics, &mut functions, ignore_next_line);
                }
            }
        }

        if last_idx < self.code.len() { tokens.push(Token { kind: TokenKind::Text, text: self.code[last_idx..].to_string(), start: last_idx, end: self.code.len() }); }
        ParseResult { tokens, diagnostics, functions }
    }

    fn handle_backslash_escape(&self, iter: &mut std::iter::Peekable<std::str::CharIndices>, idx: usize, c: char, last_idx: &mut usize, tokens: &mut Vec<Token>) -> bool {
        if c != '\\' { return false; }
        let Some(&(_, next_c)) = iter.peek() else { return false; };

        if next_c == '`' {
            if *last_idx < idx { tokens.push(Token { kind: TokenKind::Text, text: self.code[*last_idx..idx].to_string(), start: *last_idx, end: idx }); }
            iter.next(); *last_idx = idx; return true;
        }

        if next_c == '\\' {
            let mut look = iter.clone(); look.next();
            if let Some(&(_, third)) = look.peek() && matches!(third, '$' | '[' | ']' | ';' | '\\') {
                if *last_idx < idx { tokens.push(Token { kind: TokenKind::Text, text: self.code[*last_idx..idx].to_string(), start: *last_idx, end: idx }); }
                iter.next(); iter.next(); *last_idx = idx; return true;
            }
        }
        false
    }

    fn handle_js_expression(&self, iter: &mut std::iter::Peekable<std::str::CharIndices>, start: usize, brace_idx: usize, last_idx: &mut usize, tokens: &mut Vec<Token>, diagnostics: &mut Vec<Diagnostic>, ignore: bool) -> bool {
        if let Some(end) = find_matching_brace(self.code, brace_idx) {
            tokens.push(Token { kind: TokenKind::JavaScript, text: self.code[brace_idx + 1..end].to_string(), start, end: end + 1 });
            while let Some(&(j, _)) = iter.peek() { if j <= end { iter.next(); } else { break; } }
            *last_idx = end + 1; return true;
        }
        if !ignore { diagnostics.push(Diagnostic { message: "Unclosed JS expression".into(), start, end: self.code.len() }); }
        *last_idx = self.code.len(); true
    }

    fn handle_escape_function(&self, iter: &mut std::iter::Peekable<std::str::CharIndices>, start: usize, name: &str, end: usize, last: &mut usize, tokens: &mut Vec<Token>, diags: &mut Vec<Diagnostic>, ignore: bool) -> bool {
        if !is_escape_function(name) { return false; }
        if let Some(&(idx, '[')) = iter.peek() {
            if let Some(close) = find_matching_bracket_raw(self.code.as_bytes(), idx) {
                tokens.push(Token { kind: TokenKind::Escaped, text: self.code[idx + 1..close].to_string(), start, end: close + 1 });
                while let Some(&(j, _)) = iter.peek() { if j <= close { iter.next(); } else { break; } }
                *last = close + 1; return true;
            }
            if !ignore { diags.push(Diagnostic { message: format!("Unclosed '[' for ${name}"), start, end: self.code.len() }); }
            *last = self.code.len(); return true;
        }
        if !ignore { diags.push(Diagnostic { message: format!("${name} requires brackets"), start, end }); }
        tokens.push(Token { kind: TokenKind::Unknown, text: self.code[start..end].to_string(), start, end });
        *last = end; true
    }

    fn handle_modifiers(&self, iter: &mut std::iter::Peekable<std::str::CharIndices>) -> (bool, bool) {
        let mut silent = false;
        let mut negated = false;
        while let Some(&(_, c)) = iter.peek() {
            match c {
                '!' => { silent = true; iter.next(); }
                '#' => { negated = true; iter.next(); }
                '@' => {
                    let mut look = iter.clone(); look.next();
                    if let Some(&(idx, '[')) = look.peek() {
                        iter.next(); iter.next();
                        if let Some(e) = find_matching_bracket_raw(self.code.as_bytes(), idx) {
                            while let Some(&(j, _)) = iter.peek() { if j <= e { iter.next(); } else { break; } }
                        }
                    } else { break; }
                }
                _ => break,
            }
        }
        (silent, negated)
    }

    fn extract_function_name(&self, iter: &mut std::iter::Peekable<std::str::CharIndices>) -> (String, usize, usize) {
        let start = iter.peek().map(|&(i, _)| i).unwrap_or(self.code.len());
        let mut name = String::new();
        let mut end = start;
        while let Some(&(i, c)) = iter.peek() {
            if c.is_alphanumeric() || c == '_' { name.push(c); end = i + c.len_utf8(); iter.next(); }
            else { break; }
        }
        (name, start, end)
    }

    fn find_matched_function(&self, iter: &mut std::iter::Peekable<std::str::CharIndices>, full: &str, start: usize, end: usize) -> (Option<(String, Arc<Function>)>, usize) {
        let key = format!("${full}");
        if matches!(iter.peek(), Some(&(_, '['))) {
            if let Some(f) = self.manager.get_exact(&key) { return (Some((f.name.trim_start_matches('$').into(), f)), end); }
        } else if let Some((matched, f)) = self.manager.get_with_match(&key) {
            let m = matched.trim_start_matches('$');
            if full.to_lowercase().starts_with(m) { return (Some((f.name.trim_start_matches('$').into(), f)), start + m.len()); }
        }
        (None, end)
    }

    fn handle_unknown_function_call(&self, iter: &mut std::iter::Peekable<std::str::CharIndices>, name: &str, name_end: usize, start: usize, last: &mut usize, tokens: &mut Vec<Token>, diags: &mut Vec<Diagnostic>, funcs: &mut Vec<ParsedFunction>, ignore: bool) {
        if !ignore { diags.push(Diagnostic { message: format!("Unknown function `${name}`"), start, end: name_end }); }
        tokens.push(Token { kind: TokenKind::Unknown, text: self.code[start..name_end].to_string(), start, end: name_end });
        *last = name_end;

        if let Some(&(idx, '[')) = iter.peek() {
            if let Some(close) = find_matching_bracket(self.code, idx) {
                let inner = &self.code[idx + 1..close];
                if !ignore {
                    funcs.push(ParsedFunction {
                        name: name.to_string(), matched: name.to_string(), args: None, span: (start, close + 1),
                        silent: false, negated: false, count: None,
                        meta: Arc::new(Function { name: format!("${name}"), description: format!("Unknown function ${name}"), brackets: Some(true), ..Default::default() }),
                    });
                }
                tokens.push(Token { kind: TokenKind::Text, text: "[".into(), start: idx, end: idx + 1 });
                let res = ForgeScriptParser::new_internal(self.manager.clone(), inner).parse_internal();
                for t in res.tokens { tokens.push(Token { kind: t.kind, text: t.text, start: t.start + idx + 1, end: t.end + idx + 1 }); }
                if !ignore {
                    for mut d in res.diagnostics { d.start += idx + 1; d.end += idx + 1; diags.push(d); }
                    for mut f in res.functions { f.offset_spans(idx + 1); funcs.push(f); }
                }
                tokens.push(Token { kind: TokenKind::Text, text: "]".into(), start: close, end: close + 1 });
                while let Some(&(j, _)) = iter.peek() { if j <= close { iter.next(); } else { break; } }
                *last = close + 1;
            } else if !ignore {
                diags.push(Diagnostic { message: format!("Unclosed '[' for ${name}"), start: idx, end: self.code.len() });
                tokens.push(Token { kind: TokenKind::Text, text: "[".into(), start: idx, end: idx + 1 });
                iter.next(); *last = idx + 1;
            }
        }
    }

    fn handle_matched_function(&self, iter: &mut std::iter::Peekable<std::str::CharIndices>, name: String, meta: Arc<Function>, start: usize, used_end: usize, full_end: usize, silent: bool, negated: bool, last: &mut usize, tokens: &mut Vec<Token>, diags: &mut Vec<Diagnostic>, funcs: &mut Vec<ParsedFunction>, ignore: bool) {
        let mut args_text = None;
        let mut args_start = 0;
        let has_suffix = used_end < full_end;

        if !has_suffix {
            if let Some(&(idx, '[')) = iter.peek() {
                if let Some(close) = find_matching_bracket(self.code, idx) {
                    args_text = Some(&self.code[idx + 1..close]); args_start = idx + 1;
                    while let Some(&(j, _)) = iter.peek() { if j <= close { iter.next(); } else { break; } }
                    *last = close + 1;
                } else {
                    if !ignore { diags.push(Diagnostic { message: format!("Unclosed '[' for ${name}"), start, end: self.code.len() }); }
                    *last = self.code.len();
                }
            } else { *last = used_end; }
        } else { *last = full_end; }

        let (min, max) = compute_arg_counts(&meta);
        let mut parsed_args = None;

        if let Some(inner) = args_text {
            if meta.brackets.is_some() {
                if let Ok(vec) = parse_nested_args(inner, &self.manager, diags, funcs, args_start) {
                    parsed_args = Some(vec.clone());
                    validate_arg_count(&name, vec.len(), min, max, meta.args.as_ref().map(|v| v.iter().any(|a| a.rest)).unwrap_or(false), diags, (start, *last), self.code, ignore);
                    if !ignore && let Some(m_args) = &meta.args { validate_arg_enums(&name, &vec, m_args, &self.manager, diags, self.code); }
                } else if !ignore {
                    diags.push(Diagnostic { message: format!("Failed to parse args for ${name}"), start, end: *last });
                }
            } else if !ignore { diags.push(Diagnostic { message: format!("${name} does not accept brackets"), start, end: *last }); }
        } else if meta.brackets == Some(true) && !ignore {
            diags.push(Diagnostic { message: format!("${name} expects brackets"), start, end: used_end });
        }

        tokens.push(Token { kind: TokenKind::FunctionName, text: self.code[start..used_end].to_string(), start, end: used_end });
        if has_suffix { tokens.push(Token { kind: TokenKind::Text, text: self.code[used_end..full_end].to_string(), start: used_end, end: full_end }); }
        if !ignore {
            funcs.push(ParsedFunction {
                name: meta.name.trim_start_matches('$').into(), matched: self.code[start..used_end].into(),
                args: parsed_args, span: (start, if has_suffix { used_end } else { *last }),
                silent, negated, count: None, meta,
            });
        }
    }
}

fn compute_arg_counts(meta: &Function) -> (usize, usize) {
    let min = meta.args.as_ref().map(|v| v.iter().filter(|a| a.required.unwrap_or(false)).count()).unwrap_or(0);
    let max = if meta.args.as_ref().map(|v| v.iter().any(|a| a.rest)).unwrap_or(false) { usize::MAX }
        else { meta.args.as_ref().map(|v| v.len()).unwrap_or(0) };
    (min, max)
}

fn find_matching_brace(code: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (i, c) in code.char_indices().skip_while(|&(i, _)| i < open) {
        if c == '{' { depth += 1; }
        else if c == '}' { depth -= 1; if depth == 0 { return Some(i); } }
    }
    None
}

fn parse_nested_args(input: &str, manager: &Arc<MetadataManager>, diags: &mut Vec<Diagnostic>, funcs: &mut Vec<ParsedFunction>, base: usize) -> Result<Vec<(SmallVec<[ParsedArg; 8]>, (usize, usize))>, nom::Err<()>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut seen_sep = false;
    let mut first_escaped = false;
    let mut is_start = true;
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let mut c_idx = 0;
    let mut arg_start = 0;

    while c_idx < chars.len() {
        let (byte_idx, c) = chars[c_idx];
        if let Some(jump) = try_handle_escape_function_in_args(input, byte_idx, depth, &mut current) { c_idx += jump; is_start = false; continue; }
        if c == '\\' && let Some(jump) = try_handle_backslash_escape_in_args(&chars, c_idx, is_start, &mut current, &mut first_escaped) { c_idx += jump; is_start = false; continue; }

        match c {
            '[' => { if !is_escaped_bracket_in_args(&chars, c_idx) && is_function_call_bracket_sub(input, byte_idx) { depth += 1; } current.push(c); c_idx += 1; }
            ']' => { if depth > 0 && !is_escaped_bracket_in_args(&chars, c_idx) { depth -= 1; } current.push(c); c_idx += 1; }
            ';' if depth == 0 => {
                seen_sep = true;
                let trimmed = current.trim();
                let leading = current.len() - current.trim_start().len();
                let off = base + arg_start + leading;
                if !trimmed.is_empty() { args.push((parse_single_arg(trimmed, manager, first_escaped, diags, funcs, off), (off, off + trimmed.len()))); }
                else { args.push((smallvec![ParsedArg::Literal { text: String::new() }], (base + arg_start, base + arg_start))); }
                current.clear(); first_escaped = false; is_start = true; c_idx += 1;
                arg_start = chars.get(c_idx).map(|&(b, _)| b).unwrap_or(input.len());
            }
            _ => { current.push(c); c_idx += 1; }
        }
        if c != ';' || depth != 0 { is_start = false; }
    }

    if seen_sep || !current.is_empty() {
        let trimmed = current.trim();
        let leading = current.len() - current.trim_start().len();
        let off = base + arg_start + leading;
        if !trimmed.is_empty() { args.push((parse_single_arg(trimmed, manager, first_escaped, diags, funcs, off), (off, off + trimmed.len()))); }
        else { args.push((smallvec![ParsedArg::Literal { text: String::new() }], (base + arg_start, base + arg_start))); }
    }
    Ok(args)
}

fn is_function_call_bracket_sub(input: &str, idx: usize) -> bool {
    if idx == 0 || input.as_bytes().get(idx) != Some(&b'[') { return false; }
    let mut i = idx;
    let b = input.as_bytes();
    while i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') { i -= 1; }
    while i > 0 && matches!(b[i - 1], b'!' | b'#' | b'@' | b']') {
        match b[i - 1] {
            b'!' | b'#' => i -= 1,
            b']' => {
                let mut d = 0; let mut f = false;
                while i > 0 { i -= 1; if b[i] == b']' { d += 1; } else if b[i] == b'[' { d -= 1; if d == 0 { f = true; break; } } }
                if !f || i == 0 || b[i - 1] != b'@' { return false; }
                i -= 1;
            }
            _ => break,
        }
    }
    i > 0 && b[i - 1] == b'$' && (i == 1 || b[i - 2] != b'\\')
}

pub(crate) fn is_escaped_bracket_in_args(chars: &[(usize, char)], idx: usize) -> bool {
    if idx < 2 { return false; }
    if let (Some(&(_, c1)), Some(&(_, c2))) = (chars.get(idx - 2), chars.get(idx - 1)) { c1 == '\\' && c2 == '\\' } else { false }
}

fn try_handle_escape_function_in_args(input: &str, idx: usize, depth: i32, current: &mut String) -> Option<usize> {
    if input.as_bytes().get(idx) == Some(&b'$') && depth == 0 {
        if let Some(e) = find_escape_function_end(&input[idx..], 0) {
            current.push_str(&input[idx..=idx + e]); return Some(e + 1);
        }
    }
    None
}

fn try_handle_backslash_escape_in_args(chars: &[(usize, char)], col: usize, _start: bool, current: &mut String, _first: &mut bool) -> Option<usize> {
    if let Some(&(_, next)) = chars.get(col + 1) {
        if next == '`' { current.push_str("\\`" ); return Some(2); }
        if next == '\\' && let Some(&(_, third)) = chars.get(col + 2) && matches!(third, '$' | '[' | ']' | ';' | '\\') {
            current.push_str("\\\\"); current.push(third); return Some(3);
        }
    }
    current.push('\\'); Some(1)
}

fn parse_single_arg(input: &str, mgr: &Arc<MetadataManager>, force: bool, diags: &mut Vec<Diagnostic>, funcs: &mut Vec<ParsedFunction>, base: usize) -> SmallVec<[ParsedArg; 8]> {
    if !force && input.starts_with('$') {
        let mut res = ForgeScriptParser::new_internal((*mgr).clone(), input).parse_internal();
        for mut d in res.diagnostics { d.start += base; d.end += base; diags.push(d); }
        for f in &mut res.functions { f.offset_spans(base); funcs.push(f.clone()); }
        if let Some(f) = res.functions.first() {
            let mut func = f.clone(); func.offset_spans(base);
            return smallvec![ParsedArg::Function { func: Box::new(func) }];
        }
    }
    smallvec![ParsedArg::Literal { text: input.to_string() }]
}

fn validate_arg_count(name: &str, tot: usize, min: usize, max: usize, rest: bool, diags: &mut Vec<Diagnostic>, span: (usize, usize), _src: &str, ignore: bool) {
    if ignore { return; }
    if tot < min { diags.push(Diagnostic { message: format!("${name} expects >= {min} args, got {tot}"), start: span.0, end: span.1 }); }
    else if !rest && tot > max { diags.push(Diagnostic { message: format!("${name} expects <= {max} args, got {tot}"), start: span.0, end: span.1 }); }
}

fn validate_arg_enums(name: &str, parsed: &[(SmallVec<[ParsedArg; 8]>, (usize, usize))], meta: &[crate::metadata::Arg], mgr: &Arc<MetadataManager>, diags: &mut Vec<Diagnostic>, _src: &str) {
    for (i, (parts, span)) in parsed.iter().enumerate() {
        if ENUM_VALIDATION_EXCEPTIONS.contains(&(name, i)) { continue; }
        let arg = if i < meta.len() { &meta[i] } else if let Some(last) = meta.last() && last.rest { last } else { continue; };
        let vals = if let Some(en) = &arg.enum_name { mgr.enums.read().ok().and_then(|e| e.get(en).cloned()) } else { arg.arg_enum.clone() };

        if let Some(v) = vals && parts.iter().all(|p| matches!(p, ParsedArg::Literal { .. })) {
            let mut text = String::new();
            for p in parts { if let ParsedArg::Literal { text: t } = p { text.push_str(t); } }
            if !v.contains(&text) { diags.push(Diagnostic { message: format!("Invalid value `{text}` for `{}`. Expected: {:?}", arg.name, v), start: span.0, end: span.1 }); }
        }
    }
}