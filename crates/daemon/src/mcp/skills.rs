use anyhow::{Result, bail};
use sandhill_proto::Call;
use serde_json::{Value, json};

use super::{Tools, text_item};

pub(crate) const GUIDANCE: &str = "Skills the user installed in Sandhill are listed by get_skill: before work that matches one's description, read it with get_skill and follow it; its reference files and scripts are in the folder it names.";

impl Tools {
    pub(super) async fn get_skill(&self, args: &Value) -> Result<Vec<Value>> {
        let optional = |key: &str| -> Result<Option<String>> {
            match args.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.clone())),
                _ => bail!("`{key}` must be a nonempty string"),
            }
        };
        let result = self.client().await?.call::<Value>(Call::GetSkill {
            name: optional("name")?, file: optional("file")?,
        }).await.map_err(|error| {
            let error = anyhow::Error::from(error);
            if error.to_string().contains("unknown variant") || error.to_string().contains("method not found") {
                error.context("this daemon cannot list skills; update Sandhill on this device and restart this conversation")
            } else { error }
        })?;
        Ok(vec![text_item(&serde_json::to_string(&result)?)])
    }
}

pub(super) fn tool() -> Value {
    json!({
        "name": "get_skill", "title": "Read installed skills",
        "description": "List enabled skills installed by the user, or read a named skill's SKILL.md with its folder path and file list. Give file to read a relative UTF-8 reference file (up to 256 KiB). Read a matching skill before work and follow its instructions. Does not run scripts or install anything.",
        "inputSchema": {"type": "object", "additionalProperties": false, "properties": {
            "name": {"type": "string", "description": "An installed skill name; omit to list skills."},
            "file": {"type": "string", "description": "Relative text file inside the named skill; omit for SKILL.md."}
        }},
        "annotations": {"readOnlyHint": true}
    })
}
