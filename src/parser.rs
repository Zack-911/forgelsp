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

pub struct ForgeScriptParser<'a> {
    manager: Arc<MetadataManager>,
    code: &'a str,
}

impl<'a> ForgeScriptParser<'a> {
    pub fn new(manager: Arc<MetadataManager>, code: &'a str) -> Self {
        Self { manager, code }
    }

    pub fn parse(&self) -> ParseResult {
        let mut tokens: Vec<Token> = Vec::new();
        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        let mut functions: Vec<ParsedFunction> = Vec::new();

        let mut iter = self.code.char_indices().peekable();
        let mut last_idx = 0;

        while let Some((idx, c)) = iter.next() {
            if c == '$' {
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
                        if meta.brackets.unwrap_or(false) {
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
                            diagnostics.push(Diagnostic {
                                message: format!("${} does not accept brackets", name),
                                start,
                                end: last_idx,
                            });
                        }
                    } else if meta.brackets.unwrap_or(false) {
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

fn find_matching_bracket(code: &str, open_idx: usize) -> Option<usize> {
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

fn parse_nested_args(
    input: &str,
    manager: Arc<MetadataManager>,
) -> Result<Vec<SmallVec<[ParsedArg; 8]>>, nom::Err<()>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0;

    for c in input.chars() {
        match c {
            '[' => {
                depth += 1;
                current.push(c);
            }
            ']' => {
                depth -= 1;
                current.push(c);
            }
            ';' if depth == 0 => {
                let trimmed = current.trim();
                if !trimmed.is_empty() {
                    args.push(parse_single_arg(trimmed, manager.clone())?);
                } else {
                    args.push(smallvec![ParsedArg::Literal {
                        text: Cow::Owned(String::new())
                    }]);
                }
                current.clear();
            }
            _ => current.push(c),
        }
    }

    if !current.is_empty() {
        let trimmed = current.trim();
        if !trimmed.is_empty() {
            args.push(parse_single_arg(trimmed, manager.clone())?);
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
) -> Result<SmallVec<[ParsedArg; 8]>, nom::Err<()>> {
    if input.starts_with('$') {
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
