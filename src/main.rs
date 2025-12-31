//! # ForgeLSP Main Entry Point
//!
//! Initializes the Language Server Protocol server for `ForgeScript`.
//!
//! ## Initialization Flow:
//! 1. Detect workspace folders (defaults to current directory)
//! 2. Load configuration from `forgeconfig.json` (optional)
//! 3. Initialize metadata manager with function definitions
//! 4. Load custom functions if specified in configuration
//! 5. Create LSP server with all required state
//! 6. Bind to stdio and start serving LSP requests

mod diagnostics;
mod hover;
mod metadata;
mod parser;
mod semantic;
mod server;
mod utils;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tokio::io::{stdin, stdout};
use tower_lsp::{LspService, Server};

use crate::metadata::MetadataManager;
use crate::server::ForgeScriptServer;
use crate::utils::{load_forge_config, load_forge_config_full};

/// Main entry point for the `ForgeLSP` server.
#[tokio::main]
async fn main() {
    // Initialize workspace folders (will be updated during LSP initialize if client provides them)
    let workspace_folders = vec![std::env::current_dir().unwrap()];

    // Try to load URLs from forgeconfig.json, or use default ForgeScript metadata URL
    let fetch_urls = load_forge_config(&workspace_folders).unwrap_or_else(|| {
        vec!["https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"]
            .into_iter()
            .map(String::from)
            .collect()
    });

    // Initialize metadata manager with cache directory and fetch URLs
    // Wrapped in Arc for shared ownership across async tasks
    let manager = Arc::new(
        MetadataManager::new("./.cache", fetch_urls, None)
            .expect("Failed to initialize metadata manager: check cache directory permissions"),
    );

    // Load all function metadata from configured sources
    manager
        .load_all()
        .await
        .expect("Failed to load metadata sources: check internet connection or URL validity");

    // Load custom functions from forgeconfig.json if available
    if let Some(config) = load_forge_config_full(&workspace_folders)
        && let Some(custom_funcs) = config.custom_functions
        && !custom_funcs.is_empty()
    {
        manager
            .add_custom_functions(custom_funcs)
            .expect("Failed to add custom functions");
    }

    // Wrap manager in RwLock to allow dynamic updates during LSP operation
    // (e.g., when workspace configuration changes)
    let manager_wrapped = Arc::new(RwLock::new(manager));
    let full_config = load_forge_config_full(&workspace_folders);
    let config_wrapped = Arc::new(RwLock::new(full_config.clone()));

    // Initialize LSP service with all required state
    let (service, socket) = LspService::new(|client| {
        let colors = full_config
            .as_ref()
            .and_then(|c| c.function_colors.clone())
            .unwrap_or_default();

        ForgeScriptServer {
            client,                                                              // LSP client connection
            manager: manager_wrapped.clone(), // Function metadata (reloadable)
            documents: Arc::new(RwLock::new(HashMap::new())), // Document content cache
            parsed_cache: Arc::new(RwLock::new(HashMap::new())), // Parse result cache
            workspace_folders: Arc::new(RwLock::new(workspace_folders.clone())), // Active workspaces
            multiple_function_colors: Arc::new(RwLock::new(true)), // Semantic highlighting config
            function_colors: Arc::new(RwLock::new(colors)),
            config: config_wrapped,
        }
    });

    // Start the LSP server on stdin/stdout
    Server::new(stdin(), stdout(), socket).serve(service).await;
}
