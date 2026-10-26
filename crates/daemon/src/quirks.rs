//! Reading kernel messages that bend the Jupyter protocol. R kernels write
//! JSON from R lists, so an empty map can arrive as `[]` (IRkernel's
//! metadata) or `null` (Ark's, for an htmlwidget), and IRkernel answers an
//! interrupted run with the status `abort`. The protocol crate can't read
//! those messages, so they are mended first; losing one would lose the
//! output, or the reply that ends the run.

use jupyter_protocol::{Header, JupyterMessage, JupyterMessageContent};
use jupyter_zmq_client::{Connection, RawMessage, RuntimeError};
use serde_json::Value;

/// Reads the next message from a kernel's socket, mended.
pub async fn read<S: zeromq::SocketRecv>(
    conn: &mut Connection<S>,
) -> Result<JupyterMessage, RuntimeError> {
    let raw = RawMessage::from_multipart(conn.socket.recv().await?, &conn.mac)?;
    parse(raw)
}

/// What `Connection::read` does with the parts of a message, with the
/// content mended before it is parsed.
fn parse(raw: RawMessage) -> Result<JupyterMessage, RuntimeError> {
    let RawMessage {
        zmq_identities,
        jparts,
    } = raw;
    if jparts.len() < 4 {
        return Err(RuntimeError::InsufficientMessageParts(jparts.len()));
    }
    let header: Header = serde_json::from_slice(&jparts[0])?;
    let mut content: Value = serde_json::from_slice(&jparts[3])?;
    mend(&mut content);
    let content = JupyterMessageContent::from_type_and_content(&header.msg_type, content).map_err(
        |source| RuntimeError::ParseError {
            msg_type: header.msg_type.clone(),
            source,
        },
    )?;
    Ok(JupyterMessage {
        zmq_identities,
        // An empty parent header is `{}`, which is no header.
        parent_header: serde_json::from_slice(&jparts[1]).ok(),
        metadata: serde_json::from_slice(&jparts[2])?,
        content,
        buffers: jparts[4..].to_vec(),
        header,
        channel: None,
    })
}

/// Mends a message's content in place.
fn mend(content: &mut Value) {
    let Some(content) = content.as_object_mut() else {
        return;
    };
    for key in ["data", "metadata", "transient", "user_expressions"] {
        let empty = match content.get(key) {
            Some(Value::Null) => true,
            Some(Value::Array(items)) => items.is_empty(),
            _ => false,
        };
        if empty {
            content.insert(key.into(), Value::Object(Default::default()));
        }
    }
    if content.get("status").and_then(Value::as_str) == Some("abort") {
        content.insert("status".into(), "aborted".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jupyter_protocol::ReplyStatus;
    use serde_json::json;

    fn raw(msg_type: &str, content: Value) -> RawMessage {
        let header = json!({
            "msg_id": "m1",
            "username": "r",
            "session": "s",
            "date": "2026-10-26T00:00:00Z",
            "msg_type": msg_type,
            "version": "5.3",
        });
        RawMessage {
            zmq_identities: vec![],
            jparts: [header, json!({}), json!({}), content]
                .iter()
                .map(|part| serde_json::to_vec(part).unwrap().into())
                .collect(),
        }
    }

    #[test]
    fn r_kernels_empty_maps_are_read_as_maps() {
        // IRkernel's metadata, from `list()`.
        let msg = parse(raw(
            "display_data",
            json!({"data": {"text/html": "<b>hi</b>"}, "metadata": [], "transient": []}),
        ))
        .unwrap();
        assert!(matches!(msg.content, JupyterMessageContent::DisplayData(_)));
        // Ark's, for an htmlwidget.
        let msg = parse(raw(
            "display_data",
            json!({"data": {"text/html": "<b>hi</b>"}, "metadata": null}),
        ))
        .unwrap();
        assert!(matches!(msg.content, JupyterMessageContent::DisplayData(_)));
    }

    #[test]
    fn an_interrupted_r_run_is_answered() {
        let msg = parse(raw(
            "execute_reply",
            json!({"status": "abort", "execution_count": 1}),
        ))
        .unwrap();
        let JupyterMessageContent::ExecuteReply(reply) = msg.content else {
            panic!("{:?}", msg.content);
        };
        assert_eq!(reply.status, ReplyStatus::Aborted);
    }

    #[test]
    fn what_cant_be_mended_is_an_error() {
        let err = parse(raw("display_data", json!({"data": "hi"}))).unwrap_err();
        assert!(matches!(err, RuntimeError::ParseError { .. }), "{err}");
    }
}
