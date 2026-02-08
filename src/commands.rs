use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

use crate::server::{CursorMovedParams, ForgeScriptServer, TriggerCompletionNotification};

pub async fn handle_execute_command(
    server: &ForgeScriptServer,
    params: ExecuteCommandParams,
) -> Result<Option<serde_json::Value>> {
    if params.command == "forge/cursorMoved"
        && let Some(args) = params.arguments.get(0)
    {
        if let Ok(moved) = serde_json::from_value::<CursorMovedParams>(args.clone()) {
            server
                .cursor_positions
                .write()
                .expect("Server: lock poisoned")
                .insert(moved.uri.clone(), moved.position);
            crate::depth::handle_update_depth(server, moved.uri.clone()).await;

            let should_trigger = (|| {
                let docs = server.documents.read().expect("Server: lock poisoned");
                let text = docs.get(&moved.uri)?;
                let up_to_cursor = server.get_text_up_to_cursor(text, moved.position);
                let (name, open) = server.find_active_function_call(&up_to_cursor)?;
                let idx = server.compute_active_param_index(&up_to_cursor[open + 1..]) as usize;
                let mgr = server
                    .manager
                    .read()
                    .expect("Server: lock poisoned")
                    .clone();
                let func = mgr.get(&format!("${name}"))?;
                let args = func.args.as_ref()?;
                let arg_idx = if idx >= args.len() && args.last()?.rest {
                    args.len() - 1
                } else {
                    idx
                };
                let arg = args.get(arg_idx)?;
                Some(arg.enum_name.is_some() || arg.arg_enum.is_some())
            })()
            .unwrap_or(false);

            if should_trigger {
                server
                    .client
                    .send_notification::<TriggerCompletionNotification>(moved.uri.clone())
                    .await;
            }
        }
    }
    Ok(None)
}
