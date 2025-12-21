//! # Metadata Management Module
//!
//! Manages ForgeScript function metadata with three key components:
//! - **Fetcher**: HTTP client with file-based caching for metadata sources
//! - **FunctionTrie**: Prefix tree for O(k) function name lookup
//! - **MetadataManager**: Orchestrates fetching, caching, and indexing of function metadata
//!
//! Supports loading from multiple URLs, GitHub shorthand syntax, and custom user-defined functions.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::future;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::utils::Event;

// ==============================
// 📦 Data Model
// ==============================



#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub version: JsonValue,
    pub description: String,
    pub brackets: Option<bool>,
    pub unwrap: bool,
    pub args: Option<Vec<Arg>>,
    pub output: Option<Vec<String>>,
    pub category: String,
    pub aliases: Option<Vec<String>>,
    pub experimental: Option<bool>,
    pub examples: Option<Vec<String>>,
    pub deprecated: Option<bool>,
}

impl Function {
    pub fn signature_label(&self) -> String {
        let args = self.args.as_deref().unwrap_or(&[]);
        let params = args
            .iter()
            .map(|a| {
                let mut name = String::new();
                if a.rest {
                    name.push_str("...");
                }
                name.push_str(&a.name);

                // Add type info
                let type_str = match &a.arg_type {
                    JsonValue::String(s) => s.clone(),
                    JsonValue::Array(arr) => arr
                        .iter()
                        .map(|v| v.as_str().unwrap_or("?").to_string())
                        .collect::<Vec<_>>()
                        .join("|"),
                    _ => "Any".to_string(),
                };

                if !type_str.is_empty() {
                    name.push_str(": ");
                    name.push_str(&type_str);
                }

                if a.required == Some(false) {
                    name.push('?');
                }
                name
            })
            .collect::<Vec<_>>()
            .join("; ");

        format!("{}[{}]", self.name, params)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Arg {
    pub name: String,
    pub description: String,
    pub rest: bool,
    pub required: Option<bool>,
    #[serde(rename = "type")]
    pub arg_type: JsonValue,
    pub condition: Option<bool>,
    #[serde(rename = "enum")]
    pub arg_enum: Option<Vec<String>>,
    pub enum_name: Option<String>,
    pub pointer: Option<i64>,
    pub pointer_property: Option<String>,
}

// ==============================
// 🌐 Fetcher + File Cache
// ==============================

#[derive(Clone, Debug)]
pub struct Fetcher {
    http: Client,
    cache_dir: PathBuf,
}

impl Fetcher {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        let dir = cache_dir.into();
        if !dir.exists() {
            fs::create_dir_all(&dir).unwrap();
        }
        Self {
            http: Client::builder().build().unwrap(),
            cache_dir: dir,
        }
    }

    fn cache_path(&self, url: &str) -> PathBuf {
        let safe = URL_SAFE_NO_PAD.encode(url);
        self.cache_dir.join(format!("{safe}.json"))
    }

    pub async fn fetch_or_cache<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let path = self.cache_path(url);

        match self.http.get(url).send().await {
            Ok(resp) => {
                let body = resp.text().await?;
                fs::write(&path, &body)?;
                let parsed: T = serde_json::from_str(&body)?;
                Ok(parsed)
            }
            Err(_err) => {
                if path.exists() {
                    let data = fs::read_to_string(&path)?;
                    let parsed: T = serde_json::from_str(&data)?;
                    Ok(parsed)
                } else {
                    Err(anyhow!("No cache found for {url}"))
                }
            }
        }
    }

    pub async fn fetch_all(&self, urls: &[String]) -> Result<Vec<Function>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move { this.fetch_or_cache::<Vec<Function>>(&u).await }
        });
        let results = future::join_all(tasks).await;

        let mut out = Vec::new();
        let mut fail_count = 0;

        for r in results {
            if let Ok(funcs) = r {
                out.extend(funcs);
            } else {
                fail_count += 1;
            }
        }

        // Silently ignore failures - we have cached data as fallback
        let _ = fail_count;

        Ok(out)
    }

    pub async fn fetch_all_enums(&self, urls: &[String]) -> Result<HashMap<String, Vec<String>>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move {
                this.fetch_or_cache::<HashMap<String, Vec<String>>>(&u)
                    .await
            }
        });
        let results = future::join_all(tasks).await;

        let mut out = HashMap::new();
        for enums in results.into_iter().flatten() {
            out.extend(enums);
        }
        Ok(out)
    }

    pub async fn fetch_all_events(&self, urls: &[String]) -> Result<Vec<Event>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move { this.fetch_or_cache::<Vec<Event>>(&u).await }
        });
        let results = future::join_all(tasks).await;

        let mut out = Vec::new();
        for events in results.into_iter().flatten() {
            out.extend(events);
        }
        Ok(out)
    }
}

// ==============================
// ⚡ Trie Implementation
// ==============================

#[derive(Default, Debug)]
struct TrieNode {
    children: HashMap<char, TrieNode>,
    value: Option<Arc<Function>>,
}

#[derive(Default, Debug)]
pub struct FunctionTrie {
    root: TrieNode,
    size: usize,
}

impl TrieNode {
    // recursively collect functions from this node
    fn collect_all(&self, out: &mut Vec<Arc<Function>>) {
        if let Some(v) = &self.value {
            out.push(v.clone());
        }
        for child in self.children.values() {
            child.collect_all(out);
        }
    }

    fn remove_recursive(&mut self, chars: &[char], index: usize, size: &mut usize) -> bool {
        if index == chars.len() {
            if self.value.is_some() {
                self.value = None;
                *size -= 1;
                return self.children.is_empty();
            }
            return false;
        }

        let c = chars[index];
        if let Some(child) = self.children.get_mut(&c) && child.remove_recursive(chars, index + 1, size) {
            self.children.remove(&c);
            return self.value.is_none() && self.children.is_empty();
        }
        false
    }
}

impl FunctionTrie {
    pub fn insert(&mut self, key: &str, func: Arc<Function>) {
        let mut node = &mut self.root;
        for c in key.to_lowercase().chars() {
            node = node.children.entry(c).or_default();
        }
        if node.value.is_none() {
            self.size += 1;
        }
        node.value = Some(func);
    }

    pub fn remove(&mut self, key: &str) {
        let chars: Vec<char> = key.to_lowercase().chars().collect();
        self.root.remove_recursive(&chars, 0, &mut self.size);
    }
    pub fn collect_all(&self) -> Vec<Arc<Function>> {
        let mut out = Vec::new();
        self.root.collect_all(&mut out);
        out
    }
    pub fn get(&self, text: &str) -> Option<(String, Arc<Function>)> {
        let chars: Vec<char> = text.to_lowercase().chars().collect();
        let mut best_match: Option<(String, Arc<Function>)> = None;

        for start_pos in 0..chars.len() {
            let mut node = &self.root;
            let mut current_match = String::new();

            for &c in &chars[start_pos..] {
                match node.children.get(&c) {
                    Some(next) => {
                        current_match.push(c);
                        node = next;
                        if let Some(val) = &node.value {
                            best_match = Some((current_match.clone(), val.clone()));
                        }
                    }
                    _ => break,
                }
            }
        }

        best_match
    }

    pub fn get_exact(&self, text: &str) -> Option<Arc<Function>> {
        let mut node = &self.root;
        for c in text.to_lowercase().chars() {
            if let Some(next) = node.children.get(&c) {
                node = next;
            } else {
                return None;
            }
        }
        node.value.clone()
    }

    pub fn len(&self) -> usize {
        self.size
    }
}

// ==============================
// 🧠 MetadataManager
// ==============================

#[derive(Debug)]
pub struct MetadataManager {
    fetcher: Fetcher,
    fetch_urls: Vec<String>,
    trie: Arc<RwLock<FunctionTrie>>,
    pub enums: Arc<RwLock<HashMap<String, Vec<String>>>>,
    pub events: Arc<RwLock<Vec<Event>>>,
    pub file_map: Arc<RwLock<HashMap<PathBuf, Vec<String>>>>,
}

impl MetadataManager {
    pub async fn new(cache_dir: impl Into<PathBuf>, fetch_urls: Vec<String>) -> Result<Self> {
        let fetcher = Fetcher::new(cache_dir);
        let trie = Arc::new(RwLock::new(FunctionTrie::default()));
        let enums = Arc::new(RwLock::new(HashMap::new()));
        let events = Arc::new(RwLock::new(Vec::new()));
        let file_map = Arc::new(RwLock::new(HashMap::new()));

        Ok(Self {
            fetcher,
            fetch_urls,
            trie,
            enums,
            events,
            file_map,
        })
    }

    pub async fn load_all(&self) -> Result<()> {
        let all_funcs = self.fetcher.fetch_all(&self.fetch_urls).await?;

        {
            let mut trie = self.trie.write().unwrap();
            for func in all_funcs {
                if let Some(aliases) = &func.aliases {
                    for alias in aliases {
                        let mut alias_func = func.clone();
                        alias_func.name = alias.clone();
                        let arc_alias_func = Arc::new(alias_func);
                        trie.insert(alias, arc_alias_func);
                    }
                }

                let arc_func = Arc::new(func);
                trie.insert(&arc_func.name, arc_func.clone());
            }
        }

        // Fetch Enums and Events
        let mut enum_urls = Vec::new();
        let mut event_urls = Vec::new();

        for url in &self.fetch_urls {
            if url.ends_with("functions.json") {
                enum_urls.push(url.replace("functions.json", "enums.json"));
                event_urls.push(url.replace("functions.json", "events.json"));
            }
        }

        let all_enums = self.fetcher.fetch_all_enums(&enum_urls).await?;
        *self.enums.write().unwrap() = all_enums;

        let all_events = self.fetcher.fetch_all_events(&event_urls).await?;
        *self.events.write().unwrap() = all_events;

        Ok(())
    }

    pub fn load_custom_functions_from_folder(&self, path: PathBuf) -> Result<(Vec<PathBuf>, usize)> {
        if !path.exists() || !path.is_dir() {
            return Ok((Vec::new(), 0));
        }

        let mut custom_funcs = Vec::new();
        let mut files_found = Vec::new();

        self.scan_recursive(&path, &mut custom_funcs, &mut files_found)?;

        Ok((files_found, custom_funcs.len()))
    }

    fn scan_recursive(&self, path: &Path, funcs: &mut Vec<crate::utils::CustomFunction>, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.scan_recursive(&path, funcs, files)?;
            } else if path.is_file() && let Some(_ext) = path.extension().filter(|&e| e == "js" || e == "ts") {
                let content = fs::read_to_string(&path)?;
                let parsed = self.parse_custom_functions_from_js(&content, path.to_str().unwrap_or_default());
                
                let names = self.add_custom_functions(parsed.clone())?;
                self.file_map.write().unwrap().insert(path.clone(), names);

                funcs.extend(parsed);
                files.push(path);
            }
        }
        Ok(())
    }

    pub fn remove_functions_at_path(&self, path: &Path) {
        let mut file_map = self.file_map.write().unwrap();
        if let Some(names) = file_map.remove(path) {
            let mut trie = self.trie.write().unwrap();
            for name in names {
                trie.remove(&name);
            }
        }
    }

    pub fn reload_file(&self, path: PathBuf) -> Result<usize> {
        if !path.exists() || !path.is_file() {
            self.remove_functions_at_path(&path);
            return Ok(0);
        }

        let content = fs::read_to_string(&path)?;
        let parsed = self.parse_custom_functions_from_js(&content, path.to_str().unwrap_or_default());
        
        // Remove old entries first
        self.remove_functions_at_path(&path);
        
        let count = parsed.len();
        let names = self.add_custom_functions(parsed)?;
        self.file_map.write().unwrap().insert(path, names);
        
        Ok(count)
    }

    fn parse_custom_functions_from_js(&self, content: &str, file_path: &str) -> Vec<crate::utils::CustomFunction> {
        let mut functions = Vec::new();

        // 1. Find all "name:" positions
        let name_re = regex::Regex::new(r#"name:\s*['"]([^'"]+)['"]"#).unwrap();
        let name_matches: Vec<_> = name_re.captures_iter(content).map(|c| {
            let m = c.get(0).unwrap();
            (m.start(), m.end(), c[1].to_string())
        }).collect();

        // 2. Find all "params: [" positions and their matching "]"
        let params_start_re = regex::Regex::new(r#"params:\s*\["#).unwrap();
        let mut params_ranges = Vec::new();
        for m in params_start_re.find_iter(content) {
            let start = m.start();
            let mut depth = 0;
            let mut end = None;
            for (i, c) in content[start..].char_indices() {
                if c == '[' {
                    depth += 1;
                } else if c == ']' {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + i);
                        break;
                    }
                }
            }
            if let Some(e) = end {
                params_ranges.push(start..e);
            }
        }

        // 3. Filter names that are NOT inside any params range
        let mut filtered_names = Vec::new();
        for (start, end_pos, name) in &name_matches {
            let is_nested = params_ranges.iter().any(|r| r.contains(start));
            if !is_nested {
                filtered_names.push((*start, *end_pos, name.clone()));
            }
        }

        let desc_double_re = regex::Regex::new(r#"(?s)description:\s*"((?:[^"\\]|\\.)*?)""#).unwrap();
        let desc_single_re = regex::Regex::new(r#"(?s)description:\s*'((?:[^'\\]|\\.)*?)'"#).unwrap();
        let desc_backtick_re = regex::Regex::new(r#"(?s)description:\s*`((?:[^`\\]|\\.)*?)`"#).unwrap();
        let brackets_re = regex::Regex::new(r#"brackets:\s*(true|false)"#).unwrap();
        let params_re = regex::Regex::new(r#"(?s)params:\s*\[(.*?)\]"#).unwrap();
        let p_name_re = regex::Regex::new(r#"name:\s*['"]([^'"]+)['"]"#).unwrap();

        for i in 0..filtered_names.len() {
            let (_start, end_pos, name) = &filtered_names[i];
            let chunk_end = if i + 1 < filtered_names.len() {
                filtered_names[i+1].0
            } else {
                content.len()
            };
            let chunk = &content[*end_pos..chunk_end];

            // Extract metadata from chunk
            let description = desc_double_re.captures(chunk)
                .or_else(|| desc_single_re.captures(chunk))
                .or_else(|| desc_backtick_re.captures(chunk))
                .map(|c| c[1].to_string());

            let brackets = brackets_re.captures(chunk).map(|c| &c[1] == "true");

            let mut params = None;
            if let Some(p_cap) = params_re.captures(chunk) {
                let p_content = &p_cap[1];
                let p_json_str = format!("[{}]", p_content.replace('\'', "\""));
                if let Ok(p_json) = serde_json::from_str::<JsonValue>(&p_json_str) {
                    params = Some(p_json);
                } else {
                    let mut names = Vec::new();
                    for p_cap in p_name_re.captures_iter(p_content) {
                        names.push(p_cap[1].to_string());
                    }
                    if !names.is_empty() {
                        params = Some(JsonValue::Array(names.into_iter().map(JsonValue::String).collect()));
                    }
                }
            }

            functions.push(crate::utils::CustomFunction {
                name: name.clone(),
                description,
                params,
                brackets,
                alias: None,
                path: Some(file_path.to_string()),
            });
        }

        functions
    }

    pub fn add_custom_functions(
        &self,
        custom_funcs: Vec<crate::utils::CustomFunction>,
    ) -> Result<Vec<String>> {
        let mut trie = self.trie.write().unwrap();
        let mut registered_names = Vec::new();

        for custom in custom_funcs {
            // Ensure name starts with $
            let name = if custom.name.starts_with('$') {
                custom.name.clone()
            } else {
                format!("${}", custom.name)
            };

            // Convert custom function params to standard Arg format
            let args = if let Some(params) = custom.params.clone() {
                match params {
                    JsonValue::Array(arr) => {
                        let mut parsed_args = Vec::new();
                        for item in arr {
                            if let JsonValue::Object(obj) = item {
                                // Parse as CustomFunctionParam
                                if let Ok(param) =
                                    serde_json::from_value::<crate::utils::CustomFunctionParam>(
                                        JsonValue::Object(obj.clone()),
                                    )
                                {
                                    parsed_args.push(Arg {
                                        name: param.name,
                                        description: param.description.unwrap_or_default(),
                                        rest: param.rest.unwrap_or(false),
                                        required: param.required,
                                        arg_type: JsonValue::String(param.param_type),
                                        condition: None,
                                        arg_enum: param.arg_enum,
                                        enum_name: param.enum_name,
                                        pointer: None,
                                        pointer_property: None,
                                    });
                                }
                            } else if let JsonValue::String(_) = item {
                                // Simple string param
                                if let JsonValue::String(name) = item {
                                    parsed_args.push(Arg {
                                        name,
                                        description: String::new(),
                                        rest: false,
                                        required: Some(true),
                                        arg_type: JsonValue::String("String".to_string()),
                                        condition: None,
                                        arg_enum: None,
                                        enum_name: None,
                                        pointer: None,
                                        pointer_property: None,
                                    });
                                }
                            }
                        }
                        if parsed_args.is_empty() {
                            None
                        } else {
                            Some(parsed_args)
                        }
                    }
                    _ => None,
                }
            } else {
                None
            };

            // Determine brackets value
            // - If explicitly set, use that value
            // - If params given but brackets not set, default to true (required)
            // - If no params and brackets not set, leave as None (undefined)
            let brackets = if let Some(b) = custom.brackets {
                Some(b)
            } else if custom.params.is_some() {
                Some(true)
            } else {
                None
            };

            // Normalize aliases
            let aliases = custom.alias.as_ref().map(|v| {
                v.iter()
                    .map(|a| {
                        if a.starts_with('$') {
                            a.clone()
                        } else {
                            format!("${}", a)
                        }
                    })
                    .collect::<Vec<_>>()
            });

            let func = Function {
                name: name.clone(),
                version: JsonValue::String("1.0.0".to_string()),
                description: custom
                    .description
                    .unwrap_or_else(|| "Custom function".to_string()),
                brackets,
                unwrap: false,
                args,
                output: None,
                category: "custom".to_string(),
                aliases,
                experimental: None,
                examples: None,
                deprecated: None,
            };

            // Insert the main function
            let arc_func = Arc::new(func.clone());
            trie.insert(&name, arc_func);
            registered_names.push(name.clone());

            // Insert aliases (like load_all does)
            if let Some(aliases) = &func.aliases {
                for alias in aliases {
                    let mut alias_func = func.clone();
                    alias_func.name = alias.clone();
                    let arc_alias_func = Arc::new(alias_func);
                    trie.insert(alias, arc_alias_func);
                    registered_names.push(alias.clone());
                }
            }
        }

        Ok(registered_names)
    }

    pub fn get(&self, name: &str) -> Option<Arc<Function>> {
        let trie = self.trie.read().unwrap();
        trie.get(name).map(|(_, func)| func)
    }

    pub fn get_exact(&self, name: &str) -> Option<Arc<Function>> {
        let trie = self.trie.read().unwrap();
        trie.get_exact(name)
    }

    pub fn get_with_match(&self, name: &str) -> Option<(String, Arc<Function>)> {
        let trie = self.trie.read().unwrap();
        trie.get(name)
    }

    pub fn function_count(&self) -> usize {
        let trie = self.trie.read().unwrap();
        trie.len()
    }
    pub fn all_functions(&self) -> Vec<Arc<Function>> {
        let trie = self.trie.read().unwrap();
        trie.collect_all()
    }
}