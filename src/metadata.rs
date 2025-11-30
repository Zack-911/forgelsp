use anyhow::{Result, anyhow};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::future;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, RwLock},
};

// ==============================
// 📦 Data Model
// ==============================

pub type Functions = Vec<Function>;

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
        tracing::debug!("📁 Initializing Fetcher with cache directory: {:?}", dir);
        if !dir.exists() {
            tracing::info!("📁 Creating cache directory: {:?}", dir);
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

    #[tracing::instrument(skip(self), fields(url = %url))]
    pub async fn fetch_or_cache(&self, url: &str) -> Result<Functions> {
        let start = std::time::Instant::now();
        let path = self.cache_path(url);
        tracing::debug!("🔍 Attempting to fetch or load from cache: {}", url);
        tracing::debug!("📂 Cache path: {:?}", path);
        
        match self.http.get(url).send().await {
            Ok(resp) => {
                let fetch_time = start.elapsed();
                tracing::info!("✅ Successfully fetched metadata from {} in {:?}", url, fetch_time);
                
                let body_start = std::time::Instant::now();
                let body = resp.text().await?;
                tracing::debug!("⏱️  Response body read in {:?}, size: {} bytes", body_start.elapsed(), body.len());
                
                let write_start = std::time::Instant::now();
                fs::write(&path, &body)?;
                tracing::debug!("⏱️  Cache write took {:?}", write_start.elapsed());
                
                let parse_start = std::time::Instant::now();
                let parsed: Functions = serde_json::from_str(&body)?;
                tracing::info!("⏱️  Parsed {} functions in {:?}", parsed.len(), parse_start.elapsed());
                tracing::info!("⏱️  Total fetch_or_cache took {:?}", start.elapsed());
                Ok(parsed)
            }
            Err(err) => {
                tracing::warn!("⚠️  Fetch failed for {url}: {err}. Attempting to use cached file...");
                if path.exists() {
                    tracing::info!("💾 Cache hit! Loading cached metadata from {:?}", path);
                    let read_start = std::time::Instant::now();
                    let data = fs::read_to_string(&path)?;
                    tracing::debug!("⏱️  Cache read took {:?}, size: {} bytes", read_start.elapsed(), data.len());
                    
                    let parse_start = std::time::Instant::now();
                    let parsed: Functions = serde_json::from_str(&data)?;
                    tracing::info!("⏱️  Parsed {} cached functions in {:?}", parsed.len(), parse_start.elapsed());
                    tracing::info!("⏱️  Total cache load took {:?}", start.elapsed());
                    Ok(parsed)
                } else {
                    tracing::error!("❌ No cache found for {url}");
                    Err(anyhow!("No cache found for {url}"))
                }
            }
        }
    }

    pub async fn fetch_all(&self, urls: &[String]) -> Result<Vec<Function>> {
        let start = std::time::Instant::now();
        tracing::info!("🌐 Starting to fetch all metadata from {} URLs", urls.len());
        
        let tasks = urls.iter().map(|u| {
            let u = u.clone();
            let this = self.clone();
            async move { this.fetch_or_cache(&u).await }
        });
        let results = future::join_all(tasks).await;
        
        let mut out = Vec::new();
        let mut success_count = 0;
        let mut fail_count = 0;
        
        for r in results {
            if let Ok(funcs) = r {
                out.extend(funcs);
                success_count += 1;
            } else {
                fail_count += 1;
            }
        }
        
        tracing::info!("✅ Fetched {} total functions from {}/{} URLs in {:?}", 
            out.len(), success_count, urls.len(), start.elapsed());
        if fail_count > 0 {
            tracing::warn!("⚠️  {} URLs failed to fetch", fail_count);
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
}

impl FunctionTrie {
    pub fn insert(&mut self, key: &str, func: Arc<Function>) {
        tracing::trace!("🔤 Inserting function '{}' into trie", key);
        let mut node = &mut self.root;
        for c in key.to_lowercase().chars() {
            node = node.children.entry(c).or_default();
        }
        if node.value.is_none() {
            self.size += 1;
            tracing::trace!("✅ New function added, trie size now: {}", self.size);
        } else {
            tracing::trace!("🔄 Updating existing function in trie");
        }
        node.value = Some(func);
    }
    pub fn collect_all(&self) -> Vec<Arc<Function>> {
        let mut out = Vec::new();
        self.root.collect_all(&mut out);
        out
    }
    pub fn get(&self, text: &str) -> Option<(String, Arc<Function>)> {
        let start = std::time::Instant::now();
        tracing::trace!("🔍 Searching trie for: '{}'", text);
        
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
                    None => break,
                }
            }
        }

        if let Some((ref _matched, ref func)) = best_match {
            tracing::trace!("✅ Found match: '{}' -> '{}' in {:?}", text, func.name, start.elapsed());
        } else {
            tracing::trace!("❌ No match found for '{}' in {:?}", text, start.elapsed());
        }
        
        best_match
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
}

impl MetadataManager {
    pub async fn new(cache_dir: impl Into<PathBuf>, fetch_urls: Vec<String>) -> Result<Self> {
        let start = std::time::Instant::now();
        tracing::info!("🧠 Creating MetadataManager with {} URLs", fetch_urls.len());
        
        let fetcher = Fetcher::new(cache_dir);
        let trie = Arc::new(RwLock::new(FunctionTrie::default()));

        tracing::debug!("⏱️  MetadataManager creation took {:?}", start.elapsed());
        
        Ok(Self {
            fetcher,
            fetch_urls,
            trie,
        })
    }

    #[tracing::instrument(skip(self))]
    pub async fn load_all(&self) -> Result<()> {
        let start = std::time::Instant::now();
        tracing::info!("📥 Starting to load all metadata sources...");
        
        let fetch_start = std::time::Instant::now();
        let all_funcs = self.fetcher.fetch_all(&self.fetch_urls).await?;
        tracing::info!("⏱️  Fetching all metadata took {:?}", fetch_start.elapsed());
        
        let trie_start = std::time::Instant::now();
        let mut trie = self.trie.write().unwrap();
        tracing::debug!("🔒 Acquired write lock on trie in {:?}", trie_start.elapsed());

        let count = all_funcs.len();
        tracing::info!("📝 Inserting {} functions into trie...", count);
        
        let insert_start = std::time::Instant::now();
        for func in all_funcs {
            let arc_func = Arc::new(func);
            trie.insert(&arc_func.name, arc_func.clone());
        }
        tracing::info!("⏱️  Trie insertion took {:?}", insert_start.elapsed());

        tracing::info!("✅ Loaded {} functions in {:?} total", count, start.elapsed());
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<Function>> {
        let start = std::time::Instant::now();
        tracing::trace!("🔍 MetadataManager::get called for: '{}'", name);
        
        let trie = self.trie.read().unwrap();
        let result = trie.get(name).map(|(_, func)| func);
        
        tracing::trace!("⏱️  MetadataManager::get took {:?}", start.elapsed());
        result
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
