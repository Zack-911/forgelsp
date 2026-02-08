use crate::server::{DepthNotification, ForgeDepthParams, ForgeScriptServer};
use crate::utils::position_to_offset;
use tower_lsp::lsp_types::*;

pub async fn handle_update_depth(server: &ForgeScriptServer, uri: Url) {
    let depth = {
        let docs = server.documents.read().expect("Server: lock poisoned");
        let Some(text) = docs.get(&uri) else {
            return;
        };
        let cursors = server
            .cursor_positions
            .read()
            .expect("Server: lock poisoned");
        let Some(&position) = cursors.get(&uri) else {
            return;
        };
        let offset = position_to_offset(text, position).unwrap_or(0);
        crate::utils::calculate_depth(text, offset)
    };
    server
        .client
        .send_notification::<DepthNotification>(ForgeDepthParams { uri, depth })
        .await;
}
