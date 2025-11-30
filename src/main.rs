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
    // Initialize logging
    let log_dir = std::env::current_dir().unwrap();
    let log_path = log_dir.join("forgelsp.log");
    
    let file_appender = tracing_appender::rolling::never(".", "forgelsp.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
        .with_ansi(false) // Disable ANSI colors for file logging
        .with_target(true) // Show the module path
        .with_thread_ids(true) // Show thread IDs
        .with_line_number(true) // Show line numbers
        .init();

    tracing::info!("{}", "=".repeat(80));
    tracing::info!("🚀 Starting ForgeLSP server...");
    tracing::info!("📝 Log file location: {}", log_path.display());
    tracing::info!("{}", "=".repeat(80));
    
    let init_start = std::time::Instant::now();
    
    // Try to get workspace folders from environment or fallback
    let workspace_folders = vec![std::env::current_dir().unwrap()];
    tracing::info!("📂 Workspace folders: {:?}", workspace_folders);

    // Try to load URLs from forgeconfig.json
    let config_load_start = std::time::Instant::now();
    let fetch_urls = load_forge_config(&workspace_folders).unwrap_or_else(|| {
        tracing::info!("⚠️  No forgeconfig.json found, using default metadata URL");
        vec!["https://raw.githubusercontent.com/tryforge/forgescript/dev/metadata/functions.json"]
            .into_iter()
            .map(String::from)
            .collect()
    });
    tracing::info!("⏱️  Config loading took: {:?}", config_load_start.elapsed());
    tracing::info!("🔗 Metadata URLs to fetch: {:?}", fetch_urls);

    // Initialize metadata manager
    let manager_init_start = std::time::Instant::now();
    let manager = Arc::new(
        MetadataManager::new("./.cache", fetch_urls)
            .await
            .expect("Failed to initialize metadata manager"),
    );
    tracing::info!("⏱️  Metadata manager initialization took: {:?}", manager_init_start.elapsed());

    let metadata_load_start = std::time::Instant::now();
    manager
        .load_all()
        .await
        .expect("Failed to load metadata sources");
    tracing::info!("⏱️  Metadata loading took: {:?}", metadata_load_start.elapsed());
    tracing::info!("✅ Loaded {} functions", manager.function_count());

    // Wrap manager in RwLock so LSP server can update it dynamically
    let manager_wrapped = Arc::new(RwLock::new(manager));

    // Initialize LSP service
    tracing::info!("🔧 Initializing LSP service...");
    let (service, socket) = LspService::new(|client| ForgeScriptServer {
        client,
        manager: manager_wrapped.clone(),
        documents: Arc::new(RwLock::new(HashMap::new())),
        parsed_cache: Arc::new(RwLock::new(HashMap::new())),
        workspace_folders: Arc::new(RwLock::new(workspace_folders.clone())),
    });

    tracing::info!("⏱️  Total initialization took: {:?}", init_start.elapsed());
    tracing::info!("✅ ForgeLSP server ready, starting to serve requests...");
    tracing::info!("{}", "=".repeat(80));
    
    Server::new(stdin(), stdout(), socket).serve(service).await;
    
    tracing::info!("🛑 ForgeLSP server shutting down...");
}
