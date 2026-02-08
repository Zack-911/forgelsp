//! Entry point for the ForgeLSP server.
//!
//! This module initializes the MetadataManager, loads configuration from forgeconfig.json,
//! and starts the Tower LSP server on stdin/stdout.

mod commands;
mod completion;
mod definition;
mod depth;
mod diagnostics;
mod folding_range;
mod hover;
mod metadata;
mod parser;
mod semantic;
mod server;
mod signature_help;
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
async fn main() -> anyhow::Result<()> {
    // Determine the root workspace folders for configuration lookup.
    let workspace_folders = vec![std::env::current_dir()?];

    // Initialize the specialized logger for ForgeLSP.
    let full_config = load_forge_config_full(&workspace_folders).map(|(c, _)| c);
    let log_level = full_config
        .as_ref()
        .and_then(|c| c.log_level)
        .unwrap_or(crate::utils::LogLevel::Info);

    crate::utils::init_logger(workspace_folders[0].clone(), log_level)?;
    crate::utils::forge_log(
        crate::utils::LogLevel::Info,
        &format!("ForgeLSP starting up (Level: {:?})", log_level),
    );

    // Resolve metadata URLs from forgeconfig.json or use the default production URL.
    let fetch_urls = load_forge_config(&workspace_folders).unwrap_or_else(|| {
        vec!["https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"]
            .into_iter()
            .map(String::from)
            .collect()
    });

    // Initialize the MetadataManager with a local cache directory and remote URLs.
    crate::utils::forge_log(
        crate::utils::LogLevel::Debug,
        "Initializing MetadataManager...",
    );
    let manager = Arc::new(MetadataManager::new("./.cache", fetch_urls, None)?);

    // Perform an initial fetch of all metadata sources.
    crate::utils::forge_log(crate::utils::LogLevel::Info, "Fetching metadata sources...");
    manager.load_all().await?;
    crate::utils::forge_log(
        crate::utils::LogLevel::Info,
        &format!(
            "Successfully indexed {} functions",
            manager.function_count()
        ),
    );

    // Load any project-specific custom function definitions from the configuration.
    if let Some((config, _)) = load_forge_config_full(&workspace_folders)
        && let Some(custom_funcs) = config.custom_functions
        && !custom_funcs.is_empty()
    {
        manager
            .add_custom_functions(custom_funcs)
            .expect("Failed to add custom functions");
        crate::utils::forge_log(
            crate::utils::LogLevel::Info,
            "Loaded project-specific custom functions",
        );
    }

    // Wrap state in RwLocks for shared mutable access during LSP requests.
    let manager_wrapped = Arc::new(RwLock::new(manager));
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
    crate::utils::forge_log(
        crate::utils::LogLevel::Info,
        "ForgeLSP ready to receive requests",
    );
    Server::new(stdin(), stdout(), socket).serve(service).await;
    crate::utils::forge_log(crate::utils::LogLevel::Info, "ForgeLSP shutting down");
    Ok(())
}
