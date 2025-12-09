mod diagnostics;
mod hover;
mod metadata;
mod parser;
mod semantic;
mod server;
mod utils;

use metadata::MetadataManager;
use server::ForgeScriptServer;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tokio::io::{stdin, stdout};
use tower_lsp::{LspService, Server};
use utils::load_forge_config;

#[tokio::main]
async fn main() {
    // Try to get workspace folders from environment or fallback
    let workspace_folders = vec![std::env::current_dir().unwrap()];

    // Try to load URLs from forgeconfig.json
    let fetch_urls = load_forge_config(&workspace_folders).unwrap_or_else(|| {
        vec!["https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"]
            .into_iter()
            .map(String::from)
            .collect()
    });

    // Initialize metadata manager
    let manager = Arc::new(
        MetadataManager::new("./.cache", fetch_urls)
            .await
            .expect("Failed to initialize metadata manager"),
    );

    manager
        .load_all()
        .await
        .expect("Failed to load metadata sources");

    // Wrap manager in RwLock so LSP server can update it dynamically
    let manager_wrapped = Arc::new(RwLock::new(manager));

    // Initialize LSP service
    let (service, socket) = LspService::new(|client| ForgeScriptServer {
        client,
        manager: manager_wrapped.clone(),
        documents: Arc::new(RwLock::new(HashMap::new())),
        parsed_cache: Arc::new(RwLock::new(HashMap::new())),
        workspace_folders: Arc::new(RwLock::new(workspace_folders.clone())),
        multiple_function_colors: Arc::new(RwLock::new(true)),
    });

    Server::new(stdin(), stdout(), socket).serve(service).await;
}
