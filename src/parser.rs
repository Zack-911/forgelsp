use crate::metadata::{Function, MetadataManager};
use smallvec::{SmallVec, smallvec};
use std::borrow::Cow;
use std::sync::Arc;
use regex::Regex;

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

/// Check if a character at the given byte index is escaped by a backslash.
/// Counts consecutive backslashes before the position. If odd, the character is escaped.
fn is_escaped(code: &str, byte_idx: usize) -> bool {
    if byte_idx == 0 {
        return false;
    }

    let bytes = code.as_bytes();
    let mut backslash_count = 0;
    let mut pos = byte_idx;

    while pos > 0 {
        pos -= 1;
        if bytes[pos] == b'\\' {
            backslash_count += 1;
        } else {
            break;
        }
    }

    // Odd number of backslashes means the character is escaped
    backslash_count % 2 == 1
}

/// Process escape sequences in a string, returning the unescaped version.
/// Handles: \$ -> $, \[ -> [, \] -> ], \; -> ;, \\ -> \
#[allow(dead_code)]
pub fn unescape_string(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                match next {
                    '$' | '[' | ']' | ';' | '\\' => {
                        result.push(next);
                        chars.next();
                    }
                    _ => {
                        // Keep the backslash if it's not escaping a special char
                        result.push(c);
                    }
                }
            } else {
                // Trailing backslash
                result.push(c);
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Check if the function name is an escape function ($esc or $escape)
fn is_escape_function(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == "esc" || lower == "escape"
}

pub struct ForgeScriptParser<'a> {
    manager: Arc<MetadataManager>,
    code: &'a str,
    skip_extraction: bool,
}

impl<'a> ForgeScriptParser<'a> {
    pub fn new(manager: Arc<MetadataManager>, code: &'a str) -> Self {
        Self { manager, code, skip_extraction: false }
    }

    fn new_internal(manager: Arc<MetadataManager>, code: &'a str) -> Self {
        Self { manager, code, skip_extraction: true }
    }

    pub fn parse(&self) -> ParseResult {
        // If we're already inside extracted code, skip extraction and go straight to parsing
        if self.skip_extraction {
            return self.parse_internal();
        }

        // Extract code blocks using regex - match code: ` ... ` pattern (template literals)
        let code_block_regex = Regex::new(r"code:\s*`([\s\S]*?)`").unwrap();
        
        // Check if there are any code blocks
        if code_block_regex.is_match(self.code) {
            // Extract all code block contents with their original offsets
            let mut code_to_parse = String::new();
            let mut offsets: Vec<usize> = Vec::new();
            let mut block_count = 0;
            
            for cap in code_block_regex.captures_iter(self.code) {
                if let Some(content) = cap.get(1) {
                    block_count += 1;
                    let offset = content.start();
                    offsets.push(offset);
                    eprintln!("[Parser] Found code block #{}: {} chars at offset {}", block_count, content.as_str().len(), offset);
                    code_to_parse.push_str(content.as_str());
                    code_to_parse.push('\n');
                }
            }
            
            eprintln!("[Parser] Total code blocks found: {}", block_count);
            eprintln!("[Parser] Total content to parse: {} chars", code_to_parse.len());
            
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
            if c == '\\' {
                if let Some(&(_next_idx, next_c)) = iter.peek() {
                    match next_c {
                        '$' | '[' | ']' | ';' | '\\' => {
                            // Push text before the backslash
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
                            // Skip the backslash, the escaped char will be handled as regular text
                            iter.next(); // consume the escaped character
                            last_idx = idx; // Start from backslash so \$ becomes part of text
                            continue;
                        }
                        _ => {
                            // Not a special escape, treat backslash as regular text
                        }
                    }
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
fn find_matching_bracket(code: &str, open_idx: usize) -> Option<usize> {
    let mut depth = 0;
    let bytes = code.as_bytes();

    for (i, c) in code.char_indices().skip_while(|&(i, _)| i < open_idx) {
        // Check if this character is escaped
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
    manager: Arc<MetadataManager>,
) -> Result<Vec<SmallVec<[ParsedArg; 8]>>, nom::Err<()>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut seen_separator = false;
    let mut iter = input.char_indices().peekable();
    let mut first_char_escaped = false;
    let mut is_start_of_arg = true;

    while let Some((_, c)) = iter.next() {
        match c {
            '\\' => {
                if let Some(&(_, next_c)) = iter.peek() {
                    match next_c {
                        '$' | '[' | ']' | ';' | '\\' => {
                            if is_start_of_arg && next_c == '$' {
                                first_char_escaped = true;
                            }
                            current.push(next_c);
                            iter.next();
                        }
                        _ => {
                            current.push('\\');
                        }
                    }
                } else {
                    current.push('\\');
                }
            }
            '[' => {
                depth += 1;
                current.push(c);
            }
            ']' => {
                if depth > 0 {
                    depth -= 1;
                }
                current.push(c);
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
                continue; // Skip setting is_start_of_arg to false
            }
            _ => {
                current.push(c);
            }
        }
        is_start_of_arg = false;
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
        let parser = ForgeScriptParser::new(manager.clone(), input);
        let res = parser.parse();
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
