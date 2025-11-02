use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
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

/// Looks in the workspace folders for forgeconfig.json
pub fn load_forge_config(workspace_folders: &[PathBuf]) -> Option<Vec<String>> {
    for folder in workspace_folders {
        let path = folder.join("forgeconfig.json");
        eprintln!("Looking for forgeconfig.json in: {:?}", path);
        if path.exists() {
            if let Ok(data) = fs::read_to_string(&path) {
                if let Ok(config) = serde_json::from_str::<ForgeConfig>(&data) {
                    return Some(config.urls);
                }
            }
        }
    }
    None
}
