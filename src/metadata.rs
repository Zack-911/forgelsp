//! Orchestrates the retrieval, caching, and indexing of ForgeScript function metadata.
//!
//! This module provides a prefix tree (Trie) for fast function lookup and a 
//! MetadataManager that handles remote fetches from GitHub or local configuration.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Result, anyhow};
use futures::future;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tower_lsp::Client as LspClient;
use tower_lsp::lsp_types::{MessageActionItem, MessageType};

use crate::utils::Event;

/// Comprehensive metadata for a ForgeScript command/function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Function {
    pub name: String,
    pub version: JsonValue,
    pub description: String,
    pub brackets: Option<bool>,
    pub unwrap: bool,
    pub args: Option<Vec<Arg>>,
    pub output: Option<Vec<String>>,
    #[serde(default)]
    pub category: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub experimental: Option<bool>,
    pub examples: Option<Vec<String>>,
    pub deprecated: Option<bool>,
    #[serde(skip)]
    pub extension: Option<String>,
    #[serde(skip)]
    pub source_url: Option<String>,
    #[serde(skip)]
    pub local_path: Option<PathBuf>,
    #[serde(skip)]
    pub line: Option<u32>,
}

impl Function {
    /// Constructs a human-readable signature for documentation purposes.
    pub fn signature_label(&self) -> String {
        let args = self.args.as_deref().unwrap_or(&[]);
        let params = args
            .iter()
            .map(|a| {
                let mut name = String::new();
                if a.rest { name.push_str("..."); }
                name.push_str(&a.name);

                if a.required != Some(true) || a.rest { name.push('?'); }

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
                name
            })
            .collect::<Vec<_>>()
            .join("; ");

        format!("{name}[{params}]", name = self.name, params = params)
    }
}

/// Description of an individual argument within a ForgeScript function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Arg {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
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

/// Handles network requests and local disk caching of JSON metadata.
#[derive(Clone, Debug)]
pub struct Fetcher {
    http: Client,
    cache_dir: PathBuf,
    client: Option<LspClient>,
}

impl Fetcher {
    /// Initializes a Fetcher with a dedicated cache directory and optional LSP client for error reporting.
    pub fn new(cache_dir: impl Into<PathBuf>, client: Option<LspClient>) -> Self {
        let dir = cache_dir.into();
        if !dir.exists() {
            fs::create_dir_all(&dir).expect("Failed to create cache directory");
        }
        Self {
            http: Client::builder().build().expect("Failed to build HTTP client"),
            cache_dir: dir,
            client,
        }
    }

    /// Maps a URL to a stable cache filename based on the repository and type.
    fn cache_path(&self, url: &str) -> PathBuf {
        let parts: Vec<&str> = url.split('/').collect();

        if url.contains("raw.githubusercontent.com") {
            let repo = parts.get(4).unwrap_or(&"Unknown");
            let branch = parts.get(5).unwrap_or(&"main");
            let file_name = parts.last().unwrap_or(&"unknown.json");
            let type_name = if file_name.contains("functions") {
                "Functions"
            } else if file_name.contains("enums") {
                "Enums"
            } else if file_name.contains("events") {
                "Events"
            } else {
                "Other"
            };

            let repo_cap = repo.chars().next().unwrap_or_default().to_uppercase().to_string() + &repo.chars().skip(1).collect::<String>();
            let branch_cap = branch.chars().next().unwrap_or_default().to_uppercase().to_string() + &branch.chars().skip(1).collect::<String>();

            self.cache_dir.join(format!("{repo_cap}{type_name}{branch_cap}.json"))
        } else {
            use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
            let safe = URL_SAFE_NO_PAD.encode(url);
            self.cache_dir.join(format!("{safe}.json"))
        }
    }

    /// Reads deserialized data from the local cache if it exists.
    fn get_from_cache<T: DeserializeOwned>(&self, path: &Path) -> Option<T> {
        fs::read_to_string(path).ok().and_then(|data| serde_json::from_str(&data).ok())
    }

    /// Fetches JSON from a URL and updates the local cache, falling back to cache on failure.
    pub async fn fetch_or_cache<T: DeserializeOwned + Serialize>(&self, url: &str, mandatory: bool) -> Result<T> {
        let path = self.cache_path(url);

        loop {
            let res = self.http.get(url).send().await;
            
            match res {
                Ok(resp) if resp.status().is_success() || mandatory => {
                    let body = resp.text().await?;
                    if let Ok(json) = serde_json::from_str::<JsonValue>(&body) {
                        if json.is_array() || json.is_object() {
                            if let Ok(parsed) = serde_json::from_value::<T>(json) {
                                fs::write(&path, &body)?;
                                return Ok(parsed);
                            }
                        }
                    }
                    if let Some(cached) = self.get_from_cache::<T>(&path) { return Ok(cached); }
                    if mandatory && self.client.is_some() && self.show_error_report(url, "Data mismatch").await {
                        continue;
                    }
                    return Err(anyhow!("Failed to parse {url}"));
                }
                Ok(resp) => return Err(anyhow!("Status {}: {}", resp.status(), url)),
                Err(err) => {
                    if let Some(cached) = self.get_from_cache::<T>(&path) { return Ok(cached); }
                    if mandatory && self.client.is_some() && self.show_error_report(url, &format!("{err}")).await {
                        continue;
                    }
                    return Err(anyhow!("Network failure for {url}: {err}"));
                }
            }
        }
    }

    /// Displays an error message to the user via the LSP client with a retry option.
    async fn show_error_report(&self, url: &str, reason: &str) -> bool {
        if let Some(client) = &self.client {
            let message = format!("Metadata fetch failed for {url}: {reason}");
            let actions = vec![MessageActionItem { title: "Retry".to_string(), properties: HashMap::new() }];
            if let Ok(Some(item)) = client.show_message_request(MessageType::ERROR, message, Some(actions)).await {
                return item.title == "Retry";
            }
        }
        false
    }

    /// Concurrent fetch of multiple function metadata sources.
    pub async fn fetch_all(&self, urls: &[String]) -> Result<std::collections::HashMap<String, Vec<Function>>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move { (u.clone(), this.fetch_or_cache::<Vec<Function>>(&u, true).await) }
        });
        let results = future::join_all(tasks).await;

        let mut out = std::collections::HashMap::new();
        for (url, r) in results {
            if let Ok(funcs) = r { out.insert(url, funcs); }
        }

        self.cleanup_unused_cache(urls).ok();
        Ok(out)
    }

    /// Deletes cache files that are no longer referenced in the current configuration.
    fn cleanup_unused_cache(&self, active_urls: &[String]) -> Result<()> {
        if !self.cache_dir.exists() { return Ok(()); }

        let mut all_possible_active: std::collections::HashSet<PathBuf> = active_urls.iter().map(|u| self.cache_path(u)).collect();
        for url in active_urls {
            if url.ends_with("functions.json") {
                all_possible_active.insert(self.cache_path(&url.replace("functions.json", "enums.json")));
                all_possible_active.insert(self.cache_path(&url.replace("functions.json", "events.json")));
            }
        }

        for entry in fs::read_dir(&self.cache_dir)? {
            let path = entry?.path();
            if path.is_file() && !all_possible_active.contains(&path) { fs::remove_file(path)?; }
        }
        Ok(())
    }

    /// Fetches enum definitions from remote sources.
    pub async fn fetch_all_enums(&self, urls: &[String]) -> Result<HashMap<String, Vec<String>>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move { this.fetch_or_cache::<HashMap<String, Vec<String>>>(&u, false).await }
        });
        let results = future::join_all(tasks).await;
        let mut out = HashMap::new();
        for enums in results.into_iter().flatten() { out.extend(enums); }
        Ok(out)
    }

    /// Fetches event definitions from remote sources.
    pub async fn fetch_all_events(&self, urls: &[String]) -> Result<Vec<Event>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move { this.fetch_or_cache::<Vec<Event>>(&u, false).await }
        });
        let results = future::join_all(tasks).await;
        let mut out = Vec::new();
        for events in results.into_iter().flatten() { out.extend(events); }
        Ok(out)
    }
}

#[derive(Default, Debug)]
struct TrieNode {
    children: HashMap<char, TrieNode>,
    value: Option<Arc<Function>>,
}

/// Prefix tree for efficient lookup of ForgeScript functions by their name.
#[derive(Default, Debug)]
pub struct FunctionTrie {
    root: TrieNode,
    size: usize,
}

impl TrieNode {
    fn collect_all(&self, out: &mut Vec<Arc<Function>>) {
        if let Some(v) = &self.value { out.push(v.clone()); }
        for child in self.children.values() { child.collect_all(out); }
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
    /// Inserts a function index by name (case-insensitive).
    pub fn insert(&mut self, key: &str, func: Arc<Function>) {
        let mut node = &mut self.root;
        for c in key.to_lowercase().chars() { node = node.children.entry(c).or_default(); }
        if node.value.is_none() { self.size += 1; }
        node.value = Some(func);
    }

    /// Removes a function index by name.
    pub fn remove(&mut self, key: &str) {
        let chars: Vec<char> = key.to_lowercase().chars().collect();
        self.root.remove_recursive(&chars, 0, &mut self.size);
    }

    /// Returns all functions stored in the trie.
    pub fn collect_all(&self) -> Vec<Arc<Function>> {
        let mut out = Vec::new();
        self.root.collect_all(&mut out);
        out
    }

    /// Finds the function that matches the longest prefix of the provided text.
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

    /// Performs an exact, case-insensitive match for a function name.
    pub fn get_exact(&self, text: &str) -> Option<Arc<Function>> {
        let mut node = &self.root;
        for c in text.to_lowercase().chars() {
            if let Some(next) = node.children.get(&c) { node = next; }
            else { return None; }
        }
        node.value.clone()
    }

    /// Returns the number of functions indexed.
    pub fn len(&self) -> usize { self.size }
}

/// Orchestrator for ForgeScript metadata, managing fetching, caching and indexing.
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
    /// Creates a MetadataManager with caching enabled.
    pub fn new(cache_dir: impl Into<PathBuf>, fetch_urls: Vec<String>, client: Option<LspClient>) -> Result<Self> {
        Ok(Self {
            fetcher: Fetcher::new(cache_dir, client),
            fetch_urls,
            trie: Arc::new(RwLock::new(FunctionTrie::default())),
            enums: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(Vec::new())),
            file_map: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    /// Triggers a refresh of all metadata from the configured source URLs.
    pub async fn load_all(&self) -> Result<()> {
        let all_funcs_map = self.fetcher.fetch_all(&self.fetch_urls).await?;

        {
            let mut trie = self.trie.write().expect("MetadataManager: trie lock poisoned");
            for (url, mut funcs) in all_funcs_map {
                let extension = if url.contains("githubusercontent.com") {
                    url.split('/').nth(4).map(|s| s.to_string())
                } else { None };

                for func in &mut funcs {
                    func.extension = extension.clone();
                    func.source_url = Some(url.clone());
                }

                for func in funcs {
                    if let Some(aliases) = &func.aliases {
                        for alias in aliases {
                            let mut alias_func = func.clone();
                            alias_func.name = alias.clone();
                            trie.insert(alias, Arc::new(alias_func));
                        }
                    }
                    let name = func.name.clone();
                    trie.insert(&name, Arc::new(func));
                }
            }
        }

        let mut enum_urls = Vec::new();
        let mut event_urls = Vec::new();
        for url in &self.fetch_urls {
            if url.ends_with("functions.json") {
                enum_urls.push(url.replace("functions.json", "enums.json"));
                event_urls.push(url.replace("functions.json", "events.json"));
            }
        }

        *self.enums.write().expect("MetadataManager: enums lock poisoned") = self.fetcher.fetch_all_enums(&enum_urls).await?;
        *self.events.write().expect("MetadataManager: events lock poisoned") = self.fetcher.fetch_all_events(&event_urls).await?;
        Ok(())
    }

    /// Ingests custom functions from inline configuration or file paths.
    pub fn load_custom_functions_from_config(&self, config: &crate::utils::ForgeConfig, config_dir: &Path) -> Result<()> {
        if let Some(funcs) = &config.custom_functions && !funcs.is_empty() {
            self.add_custom_functions(funcs.clone())?;
        }
        if let Some(custom_path) = &config.custom_functions_path {
             let full_path = config_dir.join(custom_path);
             if full_path.exists() { self.load_custom_functions_from_folder(full_path)?; }
        }
        Ok(())
    }

    /// Recursively scans a directory for custom function definitions in JS/TS files.
    pub fn load_custom_functions_from_folder(&self, path: PathBuf) -> Result<(Vec<PathBuf>, usize)> {
        if !path.exists() || !path.is_dir() { return Ok((Vec::new(), 0)); }
        let mut custom_funcs = Vec::new();
        let mut files_found = Vec::new();
        self.scan_recursive(&path, &mut custom_funcs, &mut files_found)?;
        Ok((files_found, custom_funcs.len()))
    }

    fn scan_recursive(&self, path: &Path, funcs: &mut Vec<crate::utils::CustomFunction>, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let path = entry?.path();
            if path.is_dir() { self.scan_recursive(&path, funcs, files)?; }
            else if path.is_file() && let Some(_ext) = path.extension().filter(|&e| e == "js" || e == "ts") {
                let content = fs::read_to_string(&path)?;
                let parsed = self.parse_custom_functions_from_js(&content, path.to_str().unwrap_or_default());
                let names = self.add_custom_functions(parsed.clone())?;
                self.file_map.write().expect("MetadataManager: file_map lock poisoned").insert(path.clone(), names);
                funcs.extend(parsed);
                files.push(path);
            }
        }
        Ok(())
    }

    /// Unregisters all functions associated with a specific file path.
    pub fn remove_functions_at_path(&self, path: &Path) {
        if let Some(names) = self.file_map.write().expect("MetadataManager: lock poisoned").remove(path) {
            let mut trie = self.trie.write().expect("MetadataManager: lock poisoned");
            for name in names { trie.remove(&name); }
        }
    }

    /// Forces a reload of custom functions from a modified file.
    pub fn reload_file(&self, path: PathBuf) -> Result<usize> {
        if !path.exists() || !path.is_file() {
            self.remove_functions_at_path(&path);
            return Ok(0);
        }
        let content = fs::read_to_string(&path)?;
        let parsed = self.parse_custom_functions_from_js(&content, path.to_str().unwrap_or_default());
        self.remove_functions_at_path(&path);
        let count = parsed.len();
        let names = self.add_custom_functions(parsed)?;
        self.file_map.write().expect("MetadataManager: lock poisoned").insert(path, names);
        Ok(count)
    }

    /// Parses function headers and metadata from JS/TS source code using regular expressions.
    fn parse_custom_functions_from_js(&self, content: &str, file_path: &str) -> Vec<crate::utils::CustomFunction> {
        let mut functions = Vec::new();

        let name_re = regex::Regex::new(r#"name:\s*['"]([^'"]+)['"]"#).expect("MetadataManager: regex failure");
        let name_matches: Vec<_> = name_re.captures_iter(content).map(|c| {
            let m = c.get(0).unwrap();
            let start = m.start();
            let line = content[..start].chars().filter(|&c| c == '\n').count() as u32;
            (start, m.end(), c[1].to_string(), line)
        }).collect();

        let params_start_re = regex::Regex::new(r#"(?:params|args):\s*\["#).expect("MetadataManager: regex failure");
        let mut params_ranges = Vec::new();
        for m in params_start_re.find_iter(content) {
            let start = m.start();
            let mut depth = 0;
            for (i, c) in content[start..].char_indices() {
                if c == '[' { depth += 1; }
                else if c == ']' {
                    depth -= 1;
                    if depth == 0 { params_ranges.push(start..start + i); break; }
                }
            }
        }

        let mut filtered_names = Vec::new();
        for m in &name_matches {
            if !params_ranges.iter().any(|r| r.contains(&m.0)) { filtered_names.push(m.clone()); }
        }

        let desc_re = regex::Regex::new(r#"(?s)description:\s*(?:'((?:[^'\\]|\\.)*?)'|"((?:[^"\\]|\\.)*?)"|`((?:[^`\\]|\\.)*?)`)"#).expect("MetadataManager: regex failure");
        let brackets_re = regex::Regex::new(r"brackets:\s*(true|false)").expect("MetadataManager: regex failure");
        let p_name_re = regex::Regex::new(r#"name:\s*['"]([^'"]+)['"]"#).expect("MetadataManager: regex failure");
        let required_re = regex::Regex::new(r"(?i)required:\s*(true|false)").expect("MetadataManager: regex failure");
        let rest_re = regex::Regex::new(r"(?i)rest:\s*(true|false)").expect("MetadataManager: regex failure");
        let type_re = regex::Regex::new(r"type:\s*([^,}\n\s]+)").expect("MetadataManager: regex failure");
        let output_re = regex::Regex::new(r"output:\s*([^,}\n\s]+)").expect("MetadataManager: regex failure");

        for i in 0..filtered_names.len() {
            let (_, end_pos, name, line) = &filtered_names[i];
            let chunk_end = if i + 1 < filtered_names.len() { filtered_names[i + 1].0 } else { content.len() };
            let chunk = &content[*end_pos..chunk_end];

            let description = desc_re.captures(chunk).map(|c| c.get(1).or(c.get(2)).or(c.get(3)).unwrap().as_str().to_string());
            let brackets = brackets_re.captures(chunk).map(|c| &c[1] == "true");
            let output = output_re.captures(chunk).map(|c| c[1].split(',').map(|s| s.trim().trim_matches(|c| c == '\'' || c == '\"').to_string()).filter(|s| !s.is_empty()).collect());

            let mut params = None;
            if let Some(p_range) = params_ranges.iter().find(|r| r.start >= *end_pos && r.start < chunk_end) {
                let p_content = &content[p_range.clone()];
                let mut param_objects = Vec::new();
                let mut search_idx = 0;
                while let Some(start_bracket) = p_content[search_idx..].find('{') {
                    let abs_start = search_idx + start_bracket;
                    let mut depth = 0;
                    for (i, c) in p_content[abs_start..].char_indices() {
                        if c == '{' { depth += 1; }
                        else if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                let obj_body = &p_content[abs_start + 1..abs_start + i];
                                let mut obj_map = serde_json::Map::new();
                                if let Some(n_cap) = p_name_re.captures(obj_body) {
                                    obj_map.insert("name".to_string(), JsonValue::String(n_cap[1].to_string()));
                                    if let Some(r_cap) = required_re.captures(obj_body) { obj_map.insert("required".to_string(), JsonValue::Bool(&r_cap[1] == "true")); }
                                    if let Some(rest_cap) = rest_re.captures(obj_body) { obj_map.insert("rest".to_string(), JsonValue::Bool(&rest_cap[1] == "true")); }
                                    let t_val = type_re.captures(obj_body).map(|c| c[1].trim().trim_matches(|c| c == '\'' || c == '\"').to_string()).unwrap_or_else(|| "String".into());
                                    obj_map.insert("type".to_string(), JsonValue::String(t_val.strip_prefix("ArgType.").unwrap_or(&t_val).to_string()));
                                    if let Some(d_cap) = desc_re.captures(obj_body) { obj_map.insert("description".to_string(), JsonValue::String(d_cap.get(1).or(d_cap.get(2)).or(d_cap.get(3)).unwrap().as_str().to_string())); }
                                    param_objects.push(JsonValue::Object(obj_map));
                                }
                                search_idx = abs_start + i + 1;
                                break;
                            }
                        }
                    }
                }
                if !param_objects.is_empty() { params = Some(JsonValue::Array(param_objects)); }
                else {
                    let names: Vec<JsonValue> = p_name_re.captures_iter(p_content).map(|c| JsonValue::String(c[1].to_string())).collect();
                    if !names.is_empty() { params = Some(JsonValue::Array(names)); }
                }
            }

            functions.push(crate::utils::CustomFunction { name: name.clone(), description, params, brackets, alias: None, path: Some(file_path.to_string()), line: Some(*line), output });
        }
        functions
    }

    /// Registers a list of custom function definitions.
    pub fn add_custom_functions(&self, custom_funcs: Vec<crate::utils::CustomFunction>) -> Result<Vec<String>> {
        let mut trie = self.trie.write().expect("MetadataManager: lock poisoned");
        let mut registered_names = Vec::new();

        for custom in custom_funcs {
            let name = if custom.name.starts_with('$') { custom.name.clone() } else { format!("${}", custom.name) };
            let args = if let Some(JsonValue::Array(arr)) = custom.params.clone() {
                let mut parsed_args = Vec::new();
                for item in arr {
                    if let Ok(param) = serde_json::from_value::<crate::utils::CustomFunctionParam>(item.clone()) {
                        parsed_args.push(Arg { name: param.name, description: param.description.unwrap_or_default(), rest: param.rest.unwrap_or(false), required: param.required, arg_type: JsonValue::String(param.param_type), condition: None, arg_enum: param.arg_enum, enum_name: param.enum_name, pointer: None, pointer_property: None });
                    } else if let JsonValue::String(name) = item {
                        parsed_args.push(Arg { name, description: String::new(), rest: false, required: Some(true), arg_type: JsonValue::String("String".to_string()), condition: None, arg_enum: None, enum_name: None, pointer: None, pointer_property: None });
                    }
                }
                if parsed_args.is_empty() { None } else { Some(parsed_args) }
            } else { None };

            let brackets = custom.brackets.or(if custom.params.is_some() { Some(true) } else { None });
            let aliases = custom.alias.as_ref().map(|v| v.iter().map(|a| if a.starts_with('$') { a.clone() } else { format!("${}", a) }).collect::<Vec<_>>());

            let func = Function { name: name.clone(), version: JsonValue::String("1.0.0".to_string()), description: custom.description.unwrap_or_else(|| "Custom function".to_string()), brackets, unwrap: false, args, output: custom.output, category: Some("custom".to_string()), aliases, experimental: None, examples: None, deprecated: None, extension: None, source_url: None, local_path: custom.path.as_ref().map(PathBuf::from), line: custom.line };

            let arc_func = Arc::new(func.clone());
            trie.insert(&name, arc_func);
            registered_names.push(name.clone());

            if let Some(aliases) = &func.aliases {
                for alias in aliases {
                    let mut alias_func = func.clone();
                    alias_func.name = alias.clone();
                    trie.insert(alias, Arc::new(alias_func));
                    registered_names.push(alias.clone());
                }
            }
        }
        Ok(registered_names)
    }

    /// Retrieves function metadata by its full name or prefix match.
    pub fn get_with_match(&self, name: &str) -> Option<(String, Arc<Function>)> { self.trie.read().expect("MetadataManager: lock poisoned").get(name) }

    /// Helper for retrieving function metadata by its full name.
    pub fn get(&self, name: &str) -> Option<Arc<Function>> { self.get_with_match(name).map(|(_, func)| func) }

    /// Retrieves function metadata by exact name match.
    pub fn get_exact(&self, name: &str) -> Option<Arc<Function>> { self.trie.read().expect("MetadataManager: lock poisoned").get_exact(name) }

    /// Returns the total number of functions managed.
    pub fn function_count(&self) -> usize { self.trie.read().expect("MetadataManager: lock poisoned").len() }

    /// Returns a list of all managed functions.
    pub fn all_functions(&self) -> Vec<Arc<Function>> { self.trie.read().expect("MetadataManager: lock poisoned").collect_all() }
}
