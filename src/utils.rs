//! # Utility Functions Module
//!
//! Provides helper functions for:
//! - Loading and parsing `forgeconfig.json` configuration files
//! - Transforming GitHub shorthand URLs to raw githubusercontent URLs
//! - Asynchronous LSP client logging

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tower_lsp::Client;
#[allow(clippy::wildcard_imports)]
use tower_lsp::lsp_types::*;

/// Spawns an asynchronous task to log a message to the LSP client.
///
/// This is non-blocking and errors are ignored (logging failures don't affect functionality).
///
/// # Arguments
/// * `client` - LSP client to send the log message to
/// * `ty` - Message type (INFO, WARNING, ERROR, LOG)
/// * `msg` - Message content
pub fn spawn_log(client: Client, ty: MessageType, msg: String) {
    tokio::spawn(async move {
        let () = client.log_message(ty, msg).await;
    });
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomFunctionParam {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub param_type: String,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub rest: Option<bool>,
    #[serde(default)]
    pub arg_enum: Option<Vec<String>>,
    #[serde(default)]
    pub enum_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomFunction {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub params: Option<JsonValue>, // Can be array of objects or array of strings
    #[serde(default)]
    pub brackets: Option<bool>,
    #[serde(default)]
    pub alias: Option<Vec<String>>,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    pub name: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub intents: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ForgeConfig {
    pub urls: Vec<String>,
    #[serde(default)]
    pub multiple_function_colors: Option<bool>,
    #[serde(default)]
    pub custom_functions: Option<Vec<CustomFunction>>,
    #[serde(default)]
    pub custom_functions_path: Option<String>,
}

/// Loads `forgeconfig.json` from any workspace folder.
/// Supports GitHub shorthand entries like:
///   github:owner/repo
///   github:owner/repo#branch
///   github:owner/repo/path#branch
///
/// These are expanded to:
///   `<https://raw.githubusercontent.com/owner/repo/<branch>/forge.json>`
pub fn load_forge_config(workspace_folders: &[PathBuf]) -> Option<Vec<String>> {
    load_forge_config_full(workspace_folders).map(|cfg| cfg.urls)
}

/// Loads the full `forgeconfig.json` configuration from any workspace folder.
pub fn load_forge_config_full(workspace_folders: &[PathBuf]) -> Option<ForgeConfig> {
    for folder in workspace_folders {
        let path = folder.join("forgeconfig.json");

        if !path.exists() {
            continue;
        }

        let Ok(data) = fs::read_to_string(&path) else {
            continue;
        };

        let Ok(mut raw) = serde_json::from_str::<ForgeConfig>(&data) else {
            continue;
        };

        // Transform shorthand URLs into fully-qualified URLs
        raw.urls = raw.urls.into_iter().map(resolve_github_shorthand).collect();

        return Some(raw);
    }

    None
}

/// Converts GitHub shorthands into full raw URLs.
/// Examples:
///   github:owner/repo
///   github:owner/repo#dev
///   github:owner/repo/path/to/file.json
///
/// Output:
///   `<https://raw.githubusercontent.com/owner/repo/<branch>/path/to/file.json>`
fn resolve_github_shorthand(input: String) -> String {
    // Only rewrite GitHub-style keys. Leave normal URLs untouched.
    if !input.starts_with("github:") {
        return input;
    }

    // Remove the "github:" prefix
    let Some(trimmed) = input.strip_prefix("github:") else {
        return input;
    };

    // Split branch if provided (default to "main" if not specified)
    let (path, branch) = match trimmed.split_once('#') {
        Some((p, b)) => (p, b),
        Option::None => (trimmed, "main"), // default branch
    };

    // Parse owner/repo/path structure
    // Expected format: owner/repo or owner/repo/custom/path
    let parts: Vec<&str> = path.split('/').collect();

    if parts.len() < 2 {
        // Invalid format, return as-is
        return input;
    }

    let owner = parts[0];
    let repo = parts[1];

    // If there's a file path specified, use it; otherwise default to metadata/functions.json
    let file_path = if parts.len() > 2 {
        parts[2..].join("/")
    } else {
        "metadata/functions.json".to_string()
    };

    // Construct the raw.githubusercontent.com URL
    format!("https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{file_path}")
}
