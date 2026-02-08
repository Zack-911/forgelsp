//! Entry point for the ForgeLSP server.
//!
//! This module initializes the MetadataManager, loads configuration from forgeconfig.json,
//! and starts the Tower LSP server on stdin/stdout.

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

/// Configures and starts the ForgeScript Language Server.
#[tokio::main]
async fn main() {
    // Determine the root workspace folders for configuration lookup.
    let workspace_folders = vec![std::env::current_dir().unwrap()];

    // Resolve metadata URLs from forgeconfig.json or use the default production URL.
    let fetch_urls = load_forge_config(&workspace_folders).unwrap_or_else(|| {
        vec!["https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"]
            .into_iter()
            .map(String::from)
            .collect()
    });

    // Initialize the MetadataManager with a local cache directory and remote URLs.
    // Arc is used for thread-safe shared ownership across async tasks.
    let manager = Arc::new(
        MetadataManager::new("./.cache", fetch_urls, None)
            .expect("Failed to initialize metadata manager: check cache directory permissions"),
    );

    // Perform an initial fetch of all metadata sources.
    manager
        .load_all()
        .await
        .expect("Failed to load metadata sources: check internet connection or URL validity");

    // Load any project-specific custom function definitions from the configuration.
    if let Some((config, _)) = load_forge_config_full(&workspace_folders)
        && let Some(custom_funcs) = config.custom_functions
        && !custom_funcs.is_empty()
    {
        manager
            .add_custom_functions(custom_funcs)
            .expect("Failed to add custom functions");
    }

    // Wrap state in RwLocks for shared mutable access during LSP requests.
    let manager_wrapped = Arc::new(RwLock::new(manager));
    let full_config = load_forge_config_full(&workspace_folders).map(|(c, _)| c);
    let config_wrapped = Arc::new(RwLock::new(full_config.clone()));

    // Instantiate the LSP service with the ForgeScriptServer state.
    let (service, socket) = LspService::new(|client| {
        let colors = full_config
            .as_ref()
            .and_then(|c| c.function_colors.clone())
            .unwrap_or_default();

        let consistent = full_config
            .as_ref()
            .and_then(|c| c.consistent_function_colors)
            .unwrap_or(false);

        ForgeScriptServer {
            client,
            manager: manager_wrapped.clone(),
            documents: Arc::new(RwLock::new(HashMap::new())),
            parsed_cache: Arc::new(RwLock::new(HashMap::new())),
            workspace_folders: Arc::new(RwLock::new(workspace_folders.clone())),
            multiple_function_colors: Arc::new(RwLock::new(true)),
            consistent_function_colors: Arc::new(RwLock::new(consistent)),
            function_colors: Arc::new(RwLock::new(colors)),
            config: config_wrapped,
            cursor_positions: Arc::new(RwLock::new(HashMap::new())),
        }
    });

    // Listen for incoming LSP requests over standard IO.
    Server::new(stdin(), stdout(), socket).serve(service).await;
}
