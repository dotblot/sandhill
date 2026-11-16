//! Read-only command metadata for the message box, without starting a turn.

mod folders;
mod probes;

use std::path::Path;

use serde_json::{Value, json};

/// Discovery failures leave Sandhill's guides available for every agent.
pub async fn list(kind: &str, cwd: &str) -> Vec<Value> {
    let mut commands = match kind {
        "claude" | "antigravity" => {
            let kind = kind.to_string();
            let cwd = cwd.to_string();
            let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
            tokio::task::spawn_blocking(move || {
                folders::list(&kind, Path::new(&cwd), home.as_deref())
            })
            .await
            .unwrap_or_default()
        }
        "codex" | "pi" => probes::list(kind, Path::new(cwd)).await.unwrap_or_default(),
        _ => Vec::new(),
    };
    commands.extend(crate::bio::guides::titles().map(|(id, title)| {
        json!({"kind": "guide", "name": id, "description": title,
            "prompt": format!("For this message, follow Sandhill's bioinformatics guide “{title}” (`{id}`): read it first with the get_bio_guide tool.")})
    }));
    commands
}

pub(crate) fn item(kind: &str, name: &str, description: &str) -> Value {
    json!({"kind": kind, "name": name.trim().trim_start_matches('/'),
        "description": description.split_whitespace().collect::<Vec<_>>().join(" ")})
}
