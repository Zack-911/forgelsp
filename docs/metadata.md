# metadata.rs

## Overview

The `metadata.rs` module manages ForgeScript function metadata, including fetching from remote URLs, caching to disk, and providing fast lookup via a Trie data structure.

## Architecture

The module consists of three main components:

```
┌─────────────────┐
│    Fetcher      │ ← HTTP client + file cache
└────────┬────────┘
         │
         ↓
┌─────────────────┐
│ MetadataManager │ ← Orchestrates fetching & indexing
└────────┬────────┘
         │
         ↓
┌─────────────────┐
│  FunctionTrie   │ ← O(k) function name lookup
└─────────────────┘
```

## Data Model

### `Function` Struct

Complete function metadata definition:

```rust
pub struct Function {
    pub name: String,              // e.g., "$ping"
    pub version: JsonValue,        // Version info
    pub description: String,       // Human-readable description
    pub brackets: Option<bool>,    // true=required, false=optional, None=not allowed
    pub unwrap: bool,              // Whether to unwrap the result
    pub args: Option<Vec<Arg>>,    // Function parameters
    pub output: Option<Vec<String>>, // Return types
    pub category: String,          // e.g., "network", "math"
    pub aliases: Option<Vec<String>>, // Alternative names
    pub experimental: Option<bool>, // Experimental flag
    pub examples: Option<Vec<String>>, // Usage examples
    pub deprecated: Option<bool>,  // Deprecation flag
}
```

### `Arg` Struct

Parameter definition:

```rust
pub struct Arg {
    pub name: String,              // Parameter name
    pub description: String,       // Parameter description
    pub rest: bool,                // Variadic parameter
    pub required: Option<bool>,    // Is required?
    pub arg_type: JsonValue,       // Type definition
    pub condition: Option<bool>,   // Conditional requirement
    pub arg_enum: Option<Vec<String>>, // Valid enum values
    pub enum_name: Option<String>, // Enum type name
    pub pointer: Option<i64>,      // Pointer to another arg
    pub pointer_property: Option<String>, // Property of pointed arg
}
```

## Fetcher Component

### HTTP Client with Caching

```rust
pub struct Fetcher {
    http: Client,         // Reqwest HTTP client
    cache_dir: PathBuf,   // Local cache directory
}
```

### `fetch_or_cache` Method

Smart fetching strategy:

1. **Try Network First:**
   ```rust
   match self.http.get(url).send().await {
       Ok(resp) => {
           let body = resp.text().await?;
           fs::write(&path, &body)?;  // Update cache
           serde_json::from_str(&body)?
       }
   }
   ```

2. **Fallback to Cache:**
   ```rust
   Err(_) => {
       if path.exists() {
           let data = fs::read_to_string(&path)?;
           serde_json::from_str(&data)?
       } else {
           Err(anyhow!("No cache found"))
       }
   }
   ```

**Benefits:**
- Works offline if cache exists
- Network failures don't break the LSP
- Reduced server load from caching

### Cache Key Generation

```rust
fn cache_path(&self, url: &str) -> PathBuf {
    let safe = URL_SAFE_NO_PAD.encode(url);
    self.cache_dir.join(format!("{safe}.json"))
}
```

Uses base64 encoding to create filesystem-safe cache filenames.

### `fetch_all` Method

Fetches from multiple URLs concurrently:

```rust
let tasks = urls.iter().map(|u| async move {
    this.fetch_or_cache(&u).await
});
let results = future::join_all(tasks).await;
```

**Why Concurrent:**
- Drastically reduces initialization time
- Multiple metadata sources can be fetched in parallel
- Failures are isolated per-source

## FunctionTrie Component

### Trie Data Structure

Prefix tree for efficient function name lookup:

```rust
struct TrieNode {
    children: HashMap<char, TrieNode>,  // Child nodes
    value: Option<Arc<Function>>,        // Function at this node
}

pub struct FunctionTrie {
    root: TrieNode,
    size: usize,  // Total functions in trie
}
```

### Complexity Analysis

| Operation | Time Complexity | Space Complexity |
|-----------|----------------|------------------|
| Insert | O(k) | O(k) |
| Search | O(k) | O(1) |
| Prefix Match | O(k + m) | O(m) |

Where:
- k = length of function name
- m = number of matches

### `insert` Method

```rust
pub fn insert(&mut self, key: &str, func: Arc<Function>) {
    let mut node = &mut self.root;
    for c in key.to_lowercase().chars() {  // Case-insensitive
        node = node.children.entry(c).or_default();
    }
    if node.value.is_none() {
        self.size += 1;
    }
    node.value = Some(func);
}
```

**Key Features:**
- Case-insensitive matching (all keys lowercased)
- Reference counting with `Arc` for shared ownership
- Size tracking for metadata count reporting

### `get` Method

Fuzzy matching algorithm:

```rust
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
```

**Algorithm Explanation:**
1. Try matching from every position in the input text
2. Follow Trie paths as long as characters match
3. Update `best_match` whenever a complete function is found
4. Return the **longest** matching function

**Example:**
- Input: `$pingserver`
- Matches: `$ping` (4 chars), `$pingserver` (11 chars if exists)
- Returns: `$pingserver` (longest match)

## MetadataManager Component

### Orchestration Layer

```rust
pub struct MetadataManager {
    fetcher: Fetcher,
    fetch_urls: Vec<String>,
    trie: Arc<RwLock<FunctionTrie>>,
}
```

### `load_all` Method

Main metadata loading process:

```rust
pub async fn load_all(&self) -> Result<()> {
    let all_funcs = self.fetcher.fetch_all(&self.fetch_urls).await?;

    let mut trie = self.trie.write().unwrap();
    for func in all_funcs {
        // Handle aliases first
        if let Some(aliases) = &func.aliases {
            for alias in aliases {
                let mut alias_func = func.clone();
                alias_func.name = alias.clone();
                trie.insert(alias, Arc::new(alias_func));
            }
        }

        // Insert main function
        let arc_func = Arc::new(func);
        trie.insert(&arc_func.name, arc_func.clone());
    }
    Ok(())
}
```

**Alias Handling:**
- Creates separate `Function` entries for each alias
- Alias functions have the alias as their `name` field
- Allows lookup by either primary name or alias

### `add_custom_functions` Method

Integrates user-defined functions:

```rust
pub fn add_custom_functions(&self, custom_funcs: Vec<CustomFunction>) -> Result<()> {
    let mut trie = self.trie.write().unwrap();
    
    for custom in custom_funcs {
        let args = /* parse params to Arg format */;
        
        let func = Function {
            name: custom.name.clone(),
            description: custom.description.unwrap_or("Custom function".to_string()),
            brackets: Some(true),
            category: "custom".to_string(),
            args,
            // ... default values for other fields
        };

        trie.insert(&custom.name, Arc::new(func));
    }
    
    Ok(())
}
```

**Parameter Parsing:**
- Handles both object-style and string-style parameter definitions
- Converts to standard `Arg` format
- Provides sensible defaults for missing fields

## Public API

### Query Functions

```rust
pub fn get(&self, name: &str) -> Option<Arc<Function>>
pub fn function_count(&self) -> usize
pub fn all_functions(&self) -> Vec<Arc<Function>>
```

### Thread Safety

All public methods use `RwLock` for concurrent access:
- Multiple readers can query simultaneously
- Writers block all readers and other writers
- Critical for LSP's concurrent request handling

## Performance Characteristics

| Scenario | Performance |
|----------|-------------|
| Cold start (no cache) | Network-bound (~1-2s) |
| Warm start (cached) | Disk I/O (<100ms) |
| Function lookup | O(k) where k = name length |
| Autocomplete | O(n) where n = total functions |

## Future Optimizations

1. **Incremental Updates:** Watch for metadata changes without full reload
2. **Compressed Cache:** Use gzip for cache files
3. **Memory Pooling:** Reduce allocations for temporary strings
4. **Radix Tree:** More memory-efficient than HashMap-based Trie
