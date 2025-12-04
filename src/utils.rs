use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use tower_lsp::Client;
use tower_lsp::lsp_types::MessageType;

pub fn spawn_log(client: Client, ty: MessageType, msg: String) {
    tokio::spawn(async move {
        let _ = client.log_message(ty, msg).await;
    });
}

#[derive(Debug, Deserialize)]
struct ForgeConfig {
    urls: Vec<String>,
}

/// Loads `forgeconfig.json` from any workspace folder.
/// Supports GitHub shorthand entries like:
///   github:owner/repo
///   github:owner/repo#branch
///   github:owner/repo/path#branch
///
/// These are expanded to:
///   https://raw.githubusercontent.com/owner/repo/<branch>/forge.json
pub fn load_forge_config(workspace_folders: &[PathBuf]) -> Option<Vec<String>> {
    for folder in workspace_folders {
        let path = folder.join("forgeconfig.json");

        if !path.exists() {
            continue;
        }

        let data = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(_) => {
                continue;
            }
        };

        let raw = match serde_json::from_str::<ForgeConfig>(&data) {
            Ok(cfg) => cfg,
            Err(_) => {
                continue;
            }
        };

        // Transform shorthand into fully-qualified URLs
        let urls: Vec<String> = raw.urls.into_iter().map(resolve_github_shorthand).collect();

        return Some(urls);
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
///   https://raw.githubusercontent.com/owner/repo/<branch>/path/to/file.json
fn resolve_github_shorthand(input: String) -> String {
    // Only rewrite GitHub-style keys. Leave normal URLs untouched.
    if !input.starts_with("github:") {
        return input;
    }

    let trimmed = &input["github:".len()..];

    // Split branch if provided
    let (path, branch) = match trimmed.split_once('#') {
        Some((p, b)) => (p, b),
        None => (trimmed, "main"), // default branch
    };

    // Parse owner/repo/path structure
    let parts: Vec<&str> = path.split('/').collect();

    if parts.len() < 2 {
        // Invalid format, return as-is
        return input;
    }

    let owner = parts[0];
    let repo = parts[1];

    // If there's a file path specified, use it; otherwise default to forge.json
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
