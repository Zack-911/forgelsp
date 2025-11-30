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

/// Looks in the workspace folders for forgeconfig.json
#[tracing::instrument(skip(workspace_folders), fields(folder_count = workspace_folders.len()))]
pub fn load_forge_config(workspace_folders: &[PathBuf]) -> Option<Vec<String>> {
    let start = std::time::Instant::now();
    tracing::debug!("🔍 Searching for forgeconfig.json in {} folders", workspace_folders.len());
    
    for folder in workspace_folders {
        let path = folder.join("forgeconfig.json");
        tracing::trace!("  Checking: {:?}", path);
        
        if path.exists() {
            tracing::debug!("✅ Found forgeconfig.json at {:?}", path);
            
            let read_start = std::time::Instant::now();
            if let Ok(data) = fs::read_to_string(&path) {
                tracing::trace!("⏱️  File read took {:?}, size: {} bytes", read_start.elapsed(), data.len());
                
                let parse_start = std::time::Instant::now();
                if let Ok(config) = serde_json::from_str::<ForgeConfig>(&data) {
                    tracing::info!("✅ Loaded forgeconfig.json with {} URLs in {:?}", 
                        config.urls.len(), start.elapsed());
                    return Some(config.urls);
                } else {
                    tracing::warn!("⚠️  Failed to parse forgeconfig.json (took {:?})", parse_start.elapsed());
                }
            } else {
                tracing::warn!("⚠️  Failed to read forgeconfig.json");
            }
        }
    }
    
    tracing::debug!("❌ No forgeconfig.json found in {:?}", start.elapsed());
    None
}
