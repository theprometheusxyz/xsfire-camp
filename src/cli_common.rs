use agent_client_protocol::{
    Client, ConfigOptionUpdate, ContentBlock, ContentChunk, EmbeddedResource,
    EmbeddedResourceResource, ResourceLink, SessionConfigOption, SessionId, SessionNotification,
    SessionUpdate, TextContent, TextResourceContents,
};
use tracing::error;

use crate::{
    ACP_CLIENT, link_paths::normalize_outgoing_local_markdown_links, resolve_session_alias,
};

pub fn prompt_blocks_to_text(blocks: &[ContentBlock]) -> String {
    let mut parts: Vec<String> = Vec::new();

    for block in blocks {
        match block {
            ContentBlock::Text(t) => parts.push(t.text.clone()),
            ContentBlock::ResourceLink(ResourceLink { name, uri, .. }) => {
                if !name.is_empty() {
                    parts.push(format!("[@{name}]({uri})"));
                } else {
                    parts.push(uri.clone());
                }
            }
            ContentBlock::Resource(EmbeddedResource {
                resource:
                    EmbeddedResourceResource::TextResourceContents(TextResourceContents {
                        text,
                        uri,
                        ..
                    }),
                ..
            }) => {
                parts.push(format!("[@embedded]({uri})"));
                parts.push(text.clone());
            }
            // Keep placeholders to preserve user intent without crashing downstream CLIs.
            ContentBlock::Image(_) => parts.push("[image omitted]".to_string()),
            ContentBlock::Audio(_) => parts.push("[audio omitted]".to_string()),
            _ => parts.push("[unsupported content omitted]".to_string()),
        }
    }

    while parts.last().is_some_and(|p| p.trim().is_empty()) {
        parts.pop();
    }

    parts.join("\n")
}

async fn send_session_update_via(
    client: &(impl Client + ?Sized),
    session_id: &SessionId,
    update: SessionUpdate,
    error_context: &str,
) {
    let routed_session_id = resolve_session_alias(session_id);

    if let Err(err) = client
        .session_notification(SessionNotification::new(routed_session_id, update))
        .await
    {
        error!("Failed to send {error_context}: {err:?}");
    }
}

pub async fn send_agent_text_via(
    client: &(impl Client + ?Sized),
    session_id: &SessionId,
    text: impl Into<String>,
) {
    let text = normalize_outgoing_local_markdown_links(&text.into());
    let update = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
        TextContent::new(text),
    )));
    send_session_update_via(client, session_id, update, "agent text").await;
}

pub async fn send_agent_text(session_id: &SessionId, text: impl Into<String>) {
    let Some(client) = ACP_CLIENT.get() else {
        return;
    };
    send_agent_text_via(client.as_ref(), session_id, text).await;
}

pub async fn send_config_options_update_via(
    client: &(impl Client + ?Sized),
    session_id: &SessionId,
    config_options: Vec<SessionConfigOption>,
) {
    let update = SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(config_options));
    send_session_update_via(client, session_id, update, "config options update").await;
}

pub async fn send_config_options_update(
    session_id: &SessionId,
    config_options: Vec<SessionConfigOption>,
) {
    let Some(client) = ACP_CLIENT.get() else {
        return;
    };
    send_config_options_update_via(client.as_ref(), session_id, config_options).await;
}
