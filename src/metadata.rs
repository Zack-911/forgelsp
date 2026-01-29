//! # Metadata Management Module
//!
//! Manages ForgeScript function metadata with three key components:
//! - **Fetcher**: HTTP client with file-based caching for metadata sources
//! - **FunctionTrie**: Prefix tree for O(k) function name lookup
//! - **MetadataManager**: Orchestrates fetching, caching, and indexing of function metadata
//!
//! Supports loading from multiple URLs, GitHub shorthand syntax, and custom user-defined functions.

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

// ==============================
// 📦 Data Model
// ==============================

/// Represents a ForgeScript function call with its metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Function {
    /// The name of the function, including the `$` prefix.
    pub name: String,
    /// Version information for the function.
    pub version: JsonValue,
    /// Detailed description of the function's behavior.
    pub description: String,
    /// Whether the function requires brackets `[...]`.
    pub brackets: Option<bool>,
    /// Whether the function unwraps its arguments.
    pub unwrap: bool,
    /// List of arguments the function accepts.
    pub args: Option<Vec<Arg>>,
    /// Expected output types or values.
    pub output: Option<Vec<String>>,
    /// Category the function belongs to.
    #[serde(default)]
    pub category: Option<String>,
    /// List of aliases for this function.
    pub aliases: Option<Vec<String>>,
    /// Whether the function is experimental.
    pub experimental: Option<bool>,
    /// Example usage of the function.
    pub examples: Option<Vec<String>>,
    /// Whether the function is deprecated.
    pub deprecated: Option<bool>,
    /// The extension this function belongs to.
    #[serde(skip)]
    pub extension: Option<String>,
    /// The source URL where this function was loaded from.
    #[serde(skip)]
    pub source_url: Option<String>,
    /// The local file path for custom functions.
    #[serde(skip)]
    pub local_path: Option<PathBuf>,
    /// The line number where this function is defined.
    #[serde(skip)]
    pub line: Option<u32>,
}

impl Function {
    /// Generates a signature label for the function, used in signature help and documentation.
    ///
    /// # Returns
    /// A string representation of the function signature, e.g., `$ping[message;?ephemeral]`.
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

                if a.required != Some(true) || a.rest {
                    name.push('?');
                }

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
                name
            })
            .collect::<Vec<_>>()
            .join("; ");

        format!("{name}[{params}]", name = self.name, params = params)
    }
}

/// Represents an argument for a ForgeScript function.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Arg {
    /// The name of the argument.
    pub name: String,
    /// Description of the argument's purpose.
    #[serde(default)]
    pub description: String,
    /// Whether this is a rest argument (variadic).
    #[serde(default)]
    pub rest: bool,
    /// Whether the argument is required.
    pub required: Option<bool>,
    /// The type of the argument.
    #[serde(rename = "type")]
    pub arg_type: JsonValue,
    /// Condition for the argument (if any).
    pub condition: Option<bool>,
    /// List of allowed enum values for this argument.
    #[serde(rename = "enum")]
    pub arg_enum: Option<Vec<String>>,
    /// The name of the enum type.
    pub enum_name: Option<String>,
    /// Pointer for complex argument types.
    pub pointer: Option<i64>,
    /// Property name for pointer types.
    pub pointer_property: Option<String>,
}

// ==============================
// 🌐 Fetcher + File Cache
// ==============================

/// HTTP client with file-based caching for metadata sources.
#[derive(Clone, Debug)]
pub struct Fetcher {
    http: Client,
    cache_dir: PathBuf,
    client: Option<LspClient>,
}

impl Fetcher {
    /// Creates a new Fetcher with the specified cache directory.
    ///
    /// # Arguments
    /// * `cache_dir` - Path to the directory where cached responses will be stored.
    /// * `client` - Optional LSP client for error reporting.
    pub fn new(cache_dir: impl Into<PathBuf>, client: Option<LspClient>) -> Self {
        let dir = cache_dir.into();
        if !dir.exists() {
            fs::create_dir_all(&dir).expect("Failed to create cache directory");
        }
        Self {
            http: Client::builder()
                .build()
                .expect("Failed to build HTTP client"),
            cache_dir: dir,
            client,
        }
    }

    /// Returns the cache path for a given URL using the scheme: <RepoName><Type><Branch>.json
    fn cache_path(&self, url: &str) -> PathBuf {
        let parts: Vec<&str> = url.split('/').collect();

        // New scheme: <RepoName><Type><Branch>.json
        // URL format: https://raw.githubusercontent.com/<owner>/<repo>/<branch>/<path/to/file.json>
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

            // Capitalize repo and branch for better file names
            let repo_cap = repo
                .chars()
                .next()
                .unwrap_or_default()
                .to_uppercase()
                .to_string()
                + &repo.chars().skip(1).collect::<String>();
            let branch_cap = branch
                .chars()
                .next()
                .unwrap_or_default()
                .to_uppercase()
                .to_string()
                + &branch.chars().skip(1).collect::<String>();

            self.cache_dir
                .join(format!("{repo_cap}{type_name}{branch_cap}.json"))
        } else {
            // Fallback for non-GitHub URLs
            use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
            let safe = URL_SAFE_NO_PAD.encode(url);
            self.cache_dir.join(format!("{safe}.json"))
        }
    }

    fn get_from_cache<T: DeserializeOwned>(&self, path: &Path) -> Option<T> {
        fs::read_to_string(path).ok()
            .and_then(|data| serde_json::from_str(&data).ok())
    }

    /// Fetches data from a URL or returns cached data if available.
    ///
    /// # Arguments
    /// * `url` - The URL to fetch data from.
    ///
    /// # Returns
    /// The deserialized data of type `T`.
    pub async fn fetch_or_cache<T: DeserializeOwned + Serialize>(&self, url: &str, mandatory: bool) -> Result<T> {
        let path = self.cache_path(url);

        loop {
            let res = self.http.get(url).send().await;
            
            match res {
                Ok(resp) if resp.status().is_success() || mandatory => {
                    let body = resp.text().await?;
                    
                    // Validate JSON and try to parse into T
                    if let Ok(json) = serde_json::from_str::<JsonValue>(&body) {
                        if json.is_array() || json.is_object() {
                            if let Ok(parsed) = serde_json::from_value::<T>(json) {
                                fs::write(&path, &body)?;
                                return Ok(parsed);
                            }
                        }
                    }

                    // If we reach here, network fetch or JSON validation/parsing failed.
                    // Try fallback to cache.
                    if let Some(cached) = self.get_from_cache::<T>(&path) {
                        return Ok(cached);
                    }

                    // If no cache and mandatory, show error report and possibly retry
                    if mandatory && self.client.is_some() {
                        if self.show_error_report(url, "Fetch failed or data model mismatch").await {
                            continue;
                        }
                    }
                    
                    return Err(anyhow!("Failed to fetch or parse {url}"));
                }
                Ok(resp) => {
                    return Err(anyhow!("Optional file returned status {}: {}", resp.status(), url));
                }
                Err(err) => {
                    if let Some(cached) = self.get_from_cache::<T>(&path) {
                        return Ok(cached);
                    }

                    if mandatory && self.client.is_some() {
                        if self.show_error_report(url, &format!("Network error: {err}")).await {
                            continue;
                        }
                    }

                    return Err(anyhow!("Failed to fetch {url}: {err}"));
                }
            }
        }
    }

    async fn show_error_report(&self, url: &str, reason: &str) -> bool {
        if let Some(client) = &self.client {
            let message = format!("Failed to fetch ForgeScript metadata from {url}: {reason}");
            let actions = vec![MessageActionItem {
                title: "Retry".to_string(),
                properties: HashMap::new(),
            }];

            if let Ok(Some(item)) = client.show_message_request(MessageType::ERROR, message, Some(actions)).await {
                return item.title == "Retry";
            }
        }
        false
    }

    /// Fetches function metadata from multiple URLs concurrently.
    ///
    /// # Arguments
    /// * `urls` - List of URLs to fetch functions from.
    ///
    /// # Returns
    /// A map of URL to successfully fetched functions.
    pub async fn fetch_all(&self, urls: &[String]) -> Result<std::collections::HashMap<String, Vec<Function>>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move { (u.clone(), this.fetch_or_cache::<Vec<Function>>(&u, true).await) }
        });
        let results = future::join_all(tasks).await;

        let mut out = std::collections::HashMap::new();
        let mut fail_count = 0;

        for (url, r) in results {
            match r {
                Ok(funcs) => {
                    out.insert(url, funcs);
                }
                Err(_) => {
                    fail_count += 1;
                }
            }
        }

        // Cleanup unused cache files
        self.cleanup_unused_cache(urls).ok();

        // Silently ignore failures - we have cached data as fallback
        let _ = fail_count;

        Ok(out)
    }

    fn cleanup_unused_cache(&self, active_urls: &[String]) -> Result<()> {
        if !self.cache_dir.exists() {
            return Ok(());
        }

        let active_paths: std::collections::HashSet<PathBuf> = active_urls
            .iter()
            .map(|u| self.cache_path(u))
            .collect();

        // Also include enum and event URLs because they are fetched separately
        let mut all_possible_active = active_paths.clone();
        for url in active_urls {
            if url.ends_with("functions.json") {
                all_possible_active.insert(self.cache_path(&url.replace("functions.json", "enums.json")));
                all_possible_active.insert(self.cache_path(&url.replace("functions.json", "events.json")));
            }
        }

        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && !all_possible_active.contains(&path) {
                fs::remove_file(path)?;
            }
        }

        Ok(())
    }

    /// Fetches enum definitions from multiple URLs concurrently.
    ///
    /// # Arguments
    /// * `urls` - List of URLs to fetch enums from.
    ///
    /// # Returns
    /// A map of enum names to their allowed values.
    pub async fn fetch_all_enums(&self, urls: &[String]) -> Result<HashMap<String, Vec<String>>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move {
                this.fetch_or_cache::<HashMap<String, Vec<String>>>(&u, false)
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

    /// Fetches event definitions from multiple URLs concurrently.
    ///
    /// # Arguments
    /// * `urls` - List of URLs to fetch events from.
    ///
    /// # Returns
    /// A vector of all successfully fetched events.
    pub async fn fetch_all_events(&self, urls: &[String]) -> Result<Vec<Event>> {
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move { this.fetch_or_cache::<Vec<Event>>(&u, false).await }
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

/// A node in the [FunctionTrie].
#[derive(Default, Debug)]
struct TrieNode {
    /// Child nodes indexed by character.
    children: HashMap<char, TrieNode>,
    /// The function metadata stored at this node, if any.
    value: Option<Arc<Function>>,
}

/// A prefix tree (Trie) for efficient function name lookup.
///
/// Supports O(k) lookup time where k is the length of the function name.
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
        if let Some(child) = self.children.get_mut(&c)
            && child.remove_recursive(chars, index + 1, size)
        {
            self.children.remove(&c);
            return self.value.is_none() && self.children.is_empty();
        }
        false
    }
}

impl FunctionTrie {
    /// Inserts a function into the trie.
    ///
    /// # Arguments
    /// * `key` - The function name (case-insensitive).
    /// * `func` - The function metadata.
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

    /// Removes a function from the trie.
    ///
    /// # Arguments
    /// * `key` - The function name to remove.
    pub fn remove(&mut self, key: &str) {
        let chars: Vec<char> = key.to_lowercase().chars().collect();
        self.root.remove_recursive(&chars, 0, &mut self.size);
    }
    /// Collects all functions stored in the trie.
    ///
    /// # Returns
    /// A vector of all function metadata.
    pub fn collect_all(&self) -> Vec<Arc<Function>> {
        let mut out = Vec::new();
        self.root.collect_all(&mut out);
        out
    }
    /// Finds the longest prefix match for a given string.
    ///
    /// # Arguments
    /// * `text` - The text to search in.
    ///
    /// # Returns
    /// A tuple containing the matched string and the function metadata, if found.
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

/// Manages ForgeScript function metadata, enums, and events.
///
/// Orchestrates loading from remote sources, local configuration, and custom JS/TS files.
#[derive(Debug)]
pub struct MetadataManager {
    /// Helper for fetching and caching remote data.
    fetcher: Fetcher,
    /// List of source URLs for function metadata.
    fetch_urls: Vec<String>,
    /// Trie for efficient function lookup.
    trie: Arc<RwLock<FunctionTrie>>,
    /// Map of enum names to allowed values.
    pub enums: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// List of events supported by ForgeScript.
    pub events: Arc<RwLock<Vec<Event>>>,
    /// Map of files to the function names they register.
    pub file_map: Arc<RwLock<HashMap<PathBuf, Vec<String>>>>,
}

impl MetadataManager {
    /// Creates a new MetadataManager.
    ///
    /// # Arguments
    /// * `cache_dir` - Path for caching fetched metadata.
    /// * `fetch_urls` - Initial list of metadata source URLs.
    /// * `client` - Optional LSP client for error reporting.
    pub fn new(cache_dir: impl Into<PathBuf>, fetch_urls: Vec<String>, client: Option<LspClient>) -> Result<Self> {
        let fetcher = Fetcher::new(cache_dir, client);
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

    #[cfg(test)]
    pub fn new_test() -> Self {
        Self::new("./.test_cache", vec![], None).unwrap()
    }

    /// Loads all metadata from the configured URLs.
    pub async fn load_all(&self) -> Result<()> {
        let all_funcs_map = self.fetcher.fetch_all(&self.fetch_urls).await?;

        {
            let mut trie = self
                .trie
                .write()
                .expect("MetadataManager: trie lock poisoned");

            for (url, mut funcs) in all_funcs_map {
                // Determine extension name from URL
                // github:tryforge/ForgeDB -> ForgeDB
                // https://raw.githubusercontent.com/zack-911/forgelsp/master/metadata/functions.json -> forgelsp
                let extension = if url.contains("githubusercontent.com") {
                    url.split('/').nth(4).map(|s| s.to_string())
                } else {
                    None
                };

                for func in &mut funcs {
                    func.extension = extension.clone();
                    func.source_url = Some(url.clone());
                }

                for func in funcs {
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
        *self
            .enums
            .write()
            .expect("MetadataManager: enums lock poisoned") = all_enums;

        let all_events = self.fetcher.fetch_all_events(&event_urls).await?;
        *self
            .events
            .write()
            .expect("MetadataManager: events lock poisoned") = all_events;

        Ok(())
    }

    /// Loads custom functions based on the provided configuration.
    /// Supports both inline custom functions and loading from a specified path.
    pub fn load_custom_functions_from_config(
        &self,
        config: &crate::utils::ForgeConfig,
        config_dir: &Path,
    ) -> Result<()> {
        // 1. Load inline custom functions
        if let Some(funcs) = &config.custom_functions {
             if !funcs.is_empty() {
                self.add_custom_functions(funcs.clone())?;
             }
        }

        // 2. Load from custom_functions_path (relative to config_dir)
        if let Some(custom_path) = &config.custom_functions_path {
             let full_path = config_dir.join(custom_path);
             if full_path.exists() {
                 let _ = self.load_custom_functions_from_folder(full_path)?;
             }
        }

        Ok(())
    }

    /// Loads custom functions from all `.js` and `.ts` files in a folder and its subfolders.
    ///
    /// # Arguments
    /// * `path` - The root directory to scan.
    ///
    /// # Returns
    /// A tuple of (list of scanned files, total count of registered functions).
    pub fn load_custom_functions_from_folder(
        &self,
        path: PathBuf,
    ) -> Result<(Vec<PathBuf>, usize)> {
        if !path.exists() || !path.is_dir() {
            return Ok((Vec::new(), 0));
        }

        let mut custom_funcs = Vec::new();
        let mut files_found = Vec::new();

        self.scan_recursive(&path, &mut custom_funcs, &mut files_found)?;

        Ok((files_found, custom_funcs.len()))
    }

    /// Recursively scans a directory for `.js` and `.ts` files.
    fn scan_recursive(
        &self,
        path: &Path,
        funcs: &mut Vec<crate::utils::CustomFunction>,
        files: &mut Vec<PathBuf>,
    ) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                self.scan_recursive(&path, funcs, files)?;
            } else if path.is_file()
                && let Some(_ext) = path.extension().filter(|&e| e == "js" || e == "ts")
            {
                let content = fs::read_to_string(&path)?;
                let parsed = self
                    .parse_custom_functions_from_js(&content, path.to_str().unwrap_or_default());

                let names = self.add_custom_functions(parsed.clone())?;
                self.file_map
                    .write()
                    .expect("MetadataManager: file_map lock poisoned")
                    .insert(path.clone(), names);

                funcs.extend(parsed);
                files.push(path);
            }
        }
        Ok(())
    }

    /// Removes all functions registered from a specific file path.
    pub fn remove_functions_at_path(&self, path: &Path) {
        let mut file_map = self
            .file_map
            .write()
            .expect("MetadataManager: file_map lock poisoned");
        if let Some(names) = file_map.remove(path) {
            let mut trie = self
                .trie
                .write()
                .expect("MetadataManager: trie lock poisoned");
            for name in names {
                trie.remove(&name);
            }
        }
    }

    /// Reloads custom functions from a single file.
    pub fn reload_file(&self, path: PathBuf) -> Result<usize> {
        if !path.exists() || !path.is_file() {
            self.remove_functions_at_path(&path);
            return Ok(0);
        }

        let content = fs::read_to_string(&path)?;
        let parsed =
            self.parse_custom_functions_from_js(&content, path.to_str().unwrap_or_default());

        // Remove old entries first
        self.remove_functions_at_path(&path);

        let count = parsed.len();
        let names = self.add_custom_functions(parsed)?;
        self.file_map
            .write()
            .expect("MetadataManager: file_map lock poisoned")
            .insert(path, names);

        Ok(count)
    }

    /// Internal regex-based parser for extracting custom functions from JS/TS code.
    fn parse_custom_functions_from_js(
        &self,
        content: &str,
        file_path: &str,
    ) -> Vec<crate::utils::CustomFunction> {
        let mut functions = Vec::new();

        // 1. Find all "name:" positions
        let name_re = regex::Regex::new(r#"name:\s*['"]([^'"]+)['"]"#)
            .expect("MetadataManager: regex compile failed");
        let name_matches: Vec<_> = name_re
            .captures_iter(content)
            .map(|c| {
                let m = c.get(0).unwrap();
                let name_start = m.start();
                // Calculate line number (0-indexed)
                let line = content[..name_start].chars().filter(|&c| c == '\n').count() as u32;
                (name_start, m.end(), c[1].to_string(), line)
            })
            .collect();

        // 2. Find all "params: [" positions and their matching "]"
        let params_start_re =
            regex::Regex::new(r#"params:\s*\["#).expect("MetadataManager: regex compile failed");
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
        for (start, end_pos, name, line) in &name_matches {
            let is_nested = params_ranges.iter().any(|r| r.contains(start));
            if !is_nested {
                filtered_names.push((*start, *end_pos, name.clone(), *line));
            }
        }

        let desc_double_re = regex::Regex::new(r#"(?s)description:\s*"((?:[^"\\]|\\.)*?)""#)
            .expect("MetadataManager: regex compile failed");
        let desc_single_re = regex::Regex::new(r#"(?s)description:\s*'((?:[^'\\]|\\.)*?)'"#)
            .expect("MetadataManager: regex compile failed");
        let desc_backtick_re = regex::Regex::new(r"(?s)description:\s*`((?:[^`\\]|\\.)*?)`")
            .expect("MetadataManager: regex compile failed");
        let brackets_re = regex::Regex::new(r"brackets:\s*(true|false)")
            .expect("MetadataManager: regex compile failed");
        let p_name_re = regex::Regex::new(r#"name:\s*['"]([^'"]+)['"]"#)
            .expect("MetadataManager: regex compile failed");

        let required_re = regex::Regex::new(r"(?i)required:\s*(true|false)")
            .expect("MetadataManager: regex compile failed");
        let rest_re = regex::Regex::new(r"(?i)rest:\s*(true|false)")
            .expect("MetadataManager: regex compile failed");
        let type_re = regex::Regex::new(r"type:\s*([^,}\n\s]+)")
            .expect("MetadataManager: regex compile failed");

        for i in 0..filtered_names.len() {
            let (_start, end_pos, name, line) = &filtered_names[i];
            let chunk_end = if i + 1 < filtered_names.len() {
                filtered_names[i + 1].0
            } else {
                content.len()
            };
            let chunk = &content[*end_pos..chunk_end];

            // Extract metadata from chunk
            let description = desc_double_re
                .captures(chunk)
                .or_else(|| desc_single_re.captures(chunk))
                .or_else(|| desc_backtick_re.captures(chunk))
                .map(|c| c[1].to_string());

            let brackets = brackets_re.captures(chunk).map(|c| &c[1] == "true");

            let mut params = None;
            // Find if there's a params range that starts within this function's chunk
            if let Some(p_range) = params_ranges.iter().find(|r| r.start >= *end_pos && r.start < chunk_end) {
                let p_content = &content[p_range.start + 1..p_range.end]; // Content inside [ ]

                // Try to parse individual objects in the array first
                let mut param_objects = Vec::new();

                // Manually find { ... } blocks with balanced braces
                let mut search_idx = 0;
                while let Some(start_bracket) = p_content[search_idx..].find('{') {
                    let absolute_start = search_idx + start_bracket;
                    let mut depth = 0;
                    let mut absolute_end = None;

                    for (i, c) in p_content[absolute_start..].char_indices() {
                        if c == '{' {
                            depth += 1;
                        } else if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                absolute_end = Some(absolute_start + i);
                                break;
                            }
                        }
                    }

                    if let Some(end_bracket) = absolute_end {
                        let obj_body = &p_content[absolute_start + 1..end_bracket];
                        let mut obj_map = serde_json::Map::new();

                        // Extract name (required for a valid param object)
                        if let Some(n_cap) = p_name_re.captures(obj_body) {
                            obj_map.insert("name".to_string(), JsonValue::String(n_cap[1].to_string()));

                            // Extract required
                            if let Some(r_cap) = required_re.captures(obj_body) {
                                obj_map.insert(
                                    "required".to_string(),
                                    JsonValue::Bool(r_cap[1].eq_ignore_ascii_case("true")),
                                );
                            }

                            // Extract rest
                            if let Some(rest_cap) = rest_re.captures(obj_body) {
                                obj_map.insert(
                                    "rest".to_string(),
                                    JsonValue::Bool(rest_cap[1].eq_ignore_ascii_case("true")),
                                );
                            }

                            // Extract type
                            if let Some(t_cap) = type_re.captures(obj_body) {
                                let t_val = t_cap[1].trim();
                                let t_val_clean =
                                    t_val.trim_matches(|c| c == '\'' || c == '\"').to_string();
                                let t_final =
                                    if let Some(stripped) = t_val_clean.strip_prefix("ArgType.") {
                                        stripped.to_string()
                                    } else {
                                        t_val_clean
                                    };
                                obj_map.insert("type".to_string(), JsonValue::String(t_final));
                            } else {
                                // Default type if missing
                                obj_map.insert(
                                    "type".to_string(),
                                    JsonValue::String("String".to_string()),
                                );
                            }

                            // Extract description
                            if let Some(d_cap) = desc_double_re
                                .captures(obj_body)
                                .or_else(|| desc_single_re.captures(obj_body))
                                .or_else(|| desc_backtick_re.captures(obj_body))
                            {
                                obj_map.insert(
                                    "description".to_string(),
                                    JsonValue::String(d_cap[1].to_string()),
                                );
                            }

                            param_objects.push(JsonValue::Object(obj_map));
                        }
                        search_idx = end_bracket + 1;
                    } else {
                        break; // Unbalanced or malformed
                    }
                }

                if !param_objects.is_empty() {
                    params = Some(JsonValue::Array(param_objects));
                } else {
                    // Fallback to simple name list (e.g. params: ["arg1", "arg2"])
                    let mut names = Vec::new();
                    for p_cap in p_name_re.captures_iter(p_content) {
                        names.push(p_cap[1].to_string());
                    }
                    if !names.is_empty() {
                        params = Some(JsonValue::Array(
                            names.into_iter().map(JsonValue::String).collect(),
                        ));
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
                line: Some(*line),
            });
        }

        functions
    }

    /// Adds custom user-defined functions to the manager.
    ///
    /// # Arguments
    /// * `custom_funcs` - List of custom functions to register.
    ///
    /// # Returns
    /// A list of registered function names (including `$`).
    pub fn add_custom_functions(
        &self,
        custom_funcs: Vec<crate::utils::CustomFunction>,
    ) -> Result<Vec<String>> {
        let mut trie = self
            .trie
            .write()
            .expect("MetadataManager: trie lock poisoned");
        let mut registered_names = Vec::new();

        for custom in custom_funcs {
            // Ensure name starts with $
            let name = if custom.name.starts_with('$') {
                custom.name.clone()
            } else {
                format!("${name}", name = custom.name)
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
                            format!("${a}", a = a)
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
                category: Some("custom".to_string()),
                aliases,
                experimental: None,
                examples: None,
                deprecated: None,
                extension: None,
                source_url: None,
                local_path: custom.path.as_ref().map(PathBuf::from),
                line: custom.line,
            };

            // Insert the main function
            let arc_func = Arc::new(func.clone());
            trie.insert(&name, arc_func);
            registered_names.push(name.clone());

            // Insert aliases
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

    /// Finds a function by its name using prefix matching.
    pub fn get_with_match(&self, name: &str) -> Option<(String, Arc<Function>)> {
        let trie = self
            .trie
            .read()
            .expect("MetadataManager: trie lock poisoned");
        trie.get(name)
    }

    /// Finds a function by its name (shorthand for get_with_match).
    pub fn get(&self, name: &str) -> Option<Arc<Function>> {
        self.get_with_match(name).map(|(_, func)| func)
    }

    /// Finds a function by its exact name (no prefix matching).
    pub fn get_exact(&self, name: &str) -> Option<Arc<Function>> {
        let trie = self
            .trie
            .read()
            .expect("MetadataManager: trie lock poisoned");
        trie.get_exact(name)
    }

    /// Returns the number of functions stored in the manager.
    pub fn function_count(&self) -> usize {
        let trie = self
            .trie
            .read()
            .expect("MetadataManager: trie lock poisoned");
        trie.len()
    }

    /// Returns a list of all functions stored in the manager.
    pub fn all_functions(&self) -> Vec<Arc<Function>> {
        let trie = self
            .trie
            .read()
            .expect("MetadataManager: trie lock poisoned");
        trie.collect_all()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_path() {
        let fetcher = Fetcher::new("./.test_cache", None);
        
        // GitHub URL
        let url1 = "https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json";
        let path1 = fetcher.cache_path(url1);
        assert_eq!(path1.file_name().unwrap().to_str().unwrap(), "ForgescriptFunctionsDev.json");

        let url2 = "https://raw.githubusercontent.com/owner/repo/master/enums.json";
        let path2 = fetcher.cache_path(url2);
        assert_eq!(path2.file_name().unwrap().to_str().unwrap(), "RepoEnumsMaster.json");

        // Fallback
        let url3 = "https://example.com/data.json";
        let path3 = fetcher.cache_path(url3);
        assert!(path3.file_name().unwrap().to_str().unwrap().ends_with(".json"));
        assert!(!path3.file_name().unwrap().to_str().unwrap().contains("Functions"));
    }

    #[test]
    fn test_cleanup() {
        let cache_dir = "./.test_cleanup_cache";
        let fetcher = Fetcher::new(cache_dir, None);
        
        let url1 = "https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json";
        let path1 = fetcher.cache_path(url1);
        fs::write(&path1, "{}").unwrap();

        let old_path = PathBuf::from(cache_dir).join("old_cache_file.json");
        fs::write(&old_path, "{}").unwrap();

        assert!(path1.exists());
        assert!(old_path.exists());

        fetcher.cleanup_unused_cache(&[url1.to_string()] as &[String]).unwrap();

        assert!(path1.exists());
        assert!(!old_path.exists());

        fs::remove_dir_all(cache_dir).unwrap();
    }
    #[test]
    fn test_signature_label() {
        let func = Function {
            name: "$testFunc".to_string(),
            version: JsonValue::String("1.0".to_string()),
            description: "A test function".to_string(),
            brackets: Some(true),
            unwrap: false,
            args: Some(vec![
                Arg {
                    name: "requiredArg".to_string(),
                    description: "desc".to_string(),
                    rest: false,
                    required: Some(true),
                    arg_type: JsonValue::String("Number".to_string()),
                    condition: None,
                    arg_enum: None,
                    enum_name: None,
                    pointer: None,
                    pointer_property: None,
                },
                Arg {
                    name: "optionalArg".to_string(),
                    description: "desc".to_string(),
                    rest: false,
                    required: Some(false),
                    arg_type: JsonValue::String("String".to_string()),
                    condition: None,
                    arg_enum: None,
                    enum_name: None,
                    pointer: None,
                    pointer_property: None,
                },
                Arg {
                    name: "implicitOptional".to_string(),
                    description: "desc".to_string(),
                    rest: false,
                    required: None, // Implicitly optional
                    arg_type: JsonValue::String("Boolean".to_string()),
                    condition: None,
                    arg_enum: None,
                    enum_name: None,
                    pointer: None,
                    pointer_property: None,
                },
            ]),
            output: Some(vec!["Void".to_string()]),
            category: None,
            aliases: None,
            experimental: None,
            examples: None,
            deprecated: None,
            extension: None,
            source_url: None,
        };

        let label = func.signature_label();
        // requiredArg: Number; optionalArg?: String; implicitOptional?: Boolean
        assert_eq!(label, "$testFunc[requiredArg: Number; optionalArg?: String; implicitOptional?: Boolean]");
    }
}
