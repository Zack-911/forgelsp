# utils.rs

## Overview

The `utils.rs` module provides utility functions for configuration loading, GitHub URL transformation, and asynchronous logging. It enables flexible project configuration through `forgeconfig.json`.

## Async Logging

### `spawn_log` Function

Spawns asynchronous log messages to the LSP client:

```rust
pub fn spawn_log(client: Client, ty: MessageType, msg: String) {
    tokio::spawn(async move {
        let _ = client.log_message(ty, msg).await;
    });
}
```

**Why Spawn:**
- Logging shouldn't block LSP operations
- Fire-and-forget pattern
- Errors ignored (logging failures don't affect functionality)

**Message Types:**
- `MessageType::INFO` - General information
- `MessageType::WARNING` - Warnings and issues
- `MessageType::ERROR` - Errors (not used currently)
- `MessageType::LOG` - Debug/performance logs

## Configuration Structures

### `CustomFunctionParam`

Parameter definition for custom functions:

```rust
pub struct CustomFunctionParam {
    pub name: String,
    pub description: Option<String>,
    pub param_type: String,          // Renamed from "type" for Rust
    pub required: Option<bool>,
}
```

**JSON Example:**
```json
{
  "name": "url",
  "description": "The URL to fetch",
  "type": "String",
  "required": true
}
```

### `CustomFunction`

User-defined function specification:

```rust
pub struct CustomFunction {
    pub name: String,
    pub description: Option<String>,
    pub params: Option<JsonValue>,   // Flexible: array of objects or strings
}
```

**JSON Examples:**

Object-style params:
```json
{
  "name": "$myFunc",
  "description": "My custom function",
  "params": [
    {
      "name": "arg1",
      "type": "String",
      "required": true
    }
  ]
}
```

String-style params:
```json
{
  "name": "$simpleFunc",
  "params": ["arg1", "arg2"]
}
```

### `ForgeConfig`

Complete configuration file structure:

```rust
pub struct ForgeConfig {
    pub urls: Vec<String>,                          // Metadata source URLs
    pub multiple_function_colors: Option<bool>,     // Enable multi-color highlighting
    pub custom_functions: Option<Vec<CustomFunction>>, // User-defined functions
    pub custom_functions_path: Option<String>,      // Path to folder with .js/.ts functions
}
```

**Complete Example:**
```json
{
  "urls": [
    "https://raw.githubusercontent.com/tryforge/forgescript/main/metadata/functions.json",
    "github:myorg/custom-functions#dev"
  ],
  "multiple_function_colors": true,
  "custom_functions_path": "./custom",
  "custom_functions": [
    {
      "name": "$customHttp",
      "description": "Custom HTTP request function",
      "params": [
        {
          "name": "method",
          "type": "String",
          "description": "HTTP method",
          "required": true
        },
        {
          "name": "url",
          "type": "String",
          "description": "Target URL",
          "required": true
        }
      ]
    }
  ]
}
```

## Configuration Loading

### `load_forge_config`

Simplified loader that returns only URLs:

```rust
pub fn load_forge_config(workspace_folders: &[PathBuf]) -> Option<Vec<String>> {
    load_forge_config_full(workspace_folders).map(|cfg| cfg.urls)
}
```

Used in `main.rs` for initial configuration.

### `load_forge_config_full`

Complete configuration loader:

```rust
pub fn load_forge_config_full(workspace_folders: &[PathBuf]) -> Option<ForgeConfig> {
    for folder in workspace_folders {
        let path = folder.join("forgeconfig.json");
        
        if !path.exists() { continue; }
        
        let data = fs::read_to_string(&path).ok()?;
        let mut raw = serde_json::from_str::<ForgeConfig>(&data).ok()?;
        
        // Transform GitHub shorthand URLs
        raw.urls = raw.urls.into_iter()
            .map(resolve_github_shorthand)
            .collect();
        
        return Some(raw);
    }
    
    None
}
```

**Search Strategy:**
- Iterates through all workspace folders
- Returns first valid `forgeconfig.json` found
- Errors are silently ignored (returns `None`)
- URLs transformed before returning

## GitHub Shorthand Resolution

### `resolve_github_shorthand`

Converts GitHub shorthand syntax to raw URLs:

```rust
fn resolve_github_shorthand(input: String) -> String {
    if !input.starts_with("github:") {
        return input;  // Pass through non-GitHub URLs
    }
    
    let trimmed = &input["github:".len()..];
    
    // Split branch if provided
    let (path, branch) = match trimmed.split_once('#') {
        Some((p, b)) => (p, b),
        None => (trimmed, "main"),  // Default branch
    };
    
    // Parse owner/repo/path structure
    let parts: Vec<&str> = path.split('/').collect();
    
    if parts.len() < 2 {
        return input;  // Invalid format
    }
    
    let owner = parts[0];
    let repo = parts[1];
    
    // File path or default
    let file_path = if parts.len() > 2 {
        parts[2..].join("/")
    } else {
        "metadata/functions.json".to_string()
    };
    
    format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        owner, repo, branch, file_path
    )
}
```

### Shorthand Syntax

**Basic Format:**
```
github:owner/repo
```
Expands to:
```
https://raw.githubusercontent.com/owner/repo/main/metadata/functions.json
```

**With Branch:**
```
github:owner/repo#dev
```
Expands to:
```
https://raw.githubusercontent.com/owner/repo/dev/metadata/functions.json
```

**With Custom Path:**
```
github:owner/repo/custom/path/file.json
```
Expands to:
```
https://raw.githubusercontent.com/owner/repo/main/custom/path/file.json
```

**With Branch and Path:**
```
github:owner/repo/api/functions.json#feature-branch
```
Expands to:
```
https://raw.githubusercontent.com/owner/repo/feature-branch/api/functions.json
```

### Why Shorthand?

**Benefits:**
- Shorter, more readable configuration
- Easy to switch branches (just change `#branch`)
- Consistent format across projects
- Reduces typos in long URLs

**Example Config:**
```json
{
  "urls": [
    "github:tryforge/forgescript#main",
    "github:myteam/custom-functions#production",
    "https://example.com/other-functions.json"
  ]
}
```

## Integration Points

### Used By `main.rs`

```rust
let fetch_urls = load_forge_config(&workspace_folders).unwrap_or_else(|| {
    vec!["https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"]
});
```

Provides fallback URL if no config found.

### Used By `server.rs`

```rust
if let Some(config) = load_forge_config_full(&paths) {
    let manager = MetadataManager::new("./.cache", config.urls).await?;
    
    if let Some(custom_funcs) = config.custom_functions {
        manager.add_custom_functions(custom_funcs)?;
    }
    
    if let Some(use_colors) = config.multiple_function_colors {
        *self.multiple_function_colors.write().unwrap() = use_colors;
    }
}
```

Dynamic reload during `initialize`.

### Used By `metadata.rs`

Custom functions converted to standard `Function` format:

```rust
pub fn add_custom_functions(&self, custom_funcs: Vec<CustomFunction>) -> Result<()> {
    for custom in custom_funcs {
        let args = /* parse params */;
        let func = Function {
            name: custom.name,
            description: custom.description.unwrap_or("Custom function".to_string()),
            category: "custom".to_string(),
            args,
            // ...
        };
        trie.insert(&custom.name, Arc::new(func));
    }
}
```

## Error Handling

Currently uses silent failures:
- Invalid JSON → Returns `None`
- Missing file → Returns `None`
- Parse errors → Returns `None`

**Future Enhancement:**
Could provide detailed error messages:
```rust
Err("forgeconfig.json: Invalid JSON at line 5")
```

## Configuration Best Practices

1. **Place in Workspace Root:**
   ```
   project/
   ├── forgeconfig.json
   ├── src/
   └── ...
   ```

2. **Version Control:**
   - Commit `forgeconfig.json` for shared team config
   - Use `.gitignore` for local overrides if needed

3. **Multiple Sources:**
   - Order matters (first config found is used)
   - Combine official + custom function sources

4. **Custom Functions:**
   - Prefix with `$` like built-in functions
   - Provide clear descriptions
   - Mark required parameters

## Example Complete Config

```json
{
  "urls": [
    "github:tryforge/forgescript#main",
    "github:mycompany/discord-functions#stable",
    "https://cdn.example.com/extra-functions.json"
  ],
  "multiple_function_colors": true,
  "custom_functions": [
    {
      "name": "$env",
      "description": "Get environment variable",
      "params": [
        {
          "name": "key",
          "type": "String",
          "description": "Environment variable name",
          "required": true
        },
        {
          "name": "default",
          "type": "String",
          "description": "Default value if not found",
          "required": false
        }
      ]
    }
  ]
}
```
