//! Content-Length transport shared by stdio servers and Ark's loopback TCP.
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

const MAX_MESSAGE: usize = 16 * 1024 * 1024;
pub(super) type Writer = Box<dyn AsyncWrite + Unpin + Send>;
type Reader = Box<dyn AsyncBufRead + Unpin + Send>;

pub(super) struct Transport {
    pub writer: Writer,
    pub reader: Reader,
    pub child: Option<Child>,
}

impl Transport {
    pub async fn tcp(port: u16) -> Result<Self> {
        let socket = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
        let (reader, writer) = socket.into_split();
        Ok(Self {
            reader: Box::new(BufReader::new(reader)),
            writer: Box::new(writer),
            child: None,
        })
    }

    pub fn stdio(mut command: Command) -> Result<Self> {
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let writer = Box::new(child.stdin.take().context("language server has no stdin")?);
        let reader = Box::new(BufReader::new(
            child
                .stdout
                .take()
                .context("language server has no stdout")?,
        ));
        Ok(Self {
            writer,
            reader,
            child: Some(child),
        })
    }

    pub async fn initialize(&mut self, params: Value) -> Result<Value> {
        write(&mut self.writer, &json!({"jsonrpc":"2.0", "id":"sandhill-initialize", "method":"initialize", "params":params})).await?;
        loop {
            let message = read(&mut self.reader).await?;
            if message["id"] == "sandhill-initialize" {
                if let Some(error) = message.get("error") {
                    bail!("language server initialize failed: {error}");
                }
                let result = message
                    .get("result")
                    .context("missing initialize result")?
                    .clone();
                if result["capabilities"]
                    .get("positionEncoding")
                    .is_some_and(|encoding| encoding != "utf-16")
                {
                    bail!("language server did not negotiate UTF-16 positions");
                }
                write(
                    &mut self.writer,
                    &json!({"jsonrpc":"2.0","method":"initialized","params":{}}),
                )
                .await?;
                return Ok(result);
            }
            if let Some(reply) = server_reply(&message) {
                write(&mut self.writer, &reply).await?;
            }
        }
    }

    pub fn receive(
        self,
        endpoint: usize,
        to: mpsc::UnboundedSender<(usize, Result<Value>)>,
    ) -> (Writer, tokio::task::JoinHandle<()>) {
        let Self {
            mut reader,
            writer,
            child,
        } = self;
        let task = tokio::spawn(async move {
            // Own the child until the read loop is cancelled or the process exits.
            let _child = child;
            loop {
                let message = read(&mut reader).await;
                let failed = message.is_err();
                if to.send((endpoint, message)).is_err() || failed {
                    break;
                }
            }
        });
        (writer, task)
    }
}

pub(super) async fn write(writer: &mut Writer, message: &Value) -> Result<()> {
    let body = serde_json::to_vec(message)?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}

async fn read(reader: &mut Reader) -> Result<Value> {
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 {
            bail!("language server closed its transport");
        }
        header_bytes += line.len();
        if header_bytes > 8192 {
            bail!("language server header too large");
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = Some(value.trim().parse::<usize>()?);
        }
    }
    let length = length.context("language server omitted Content-Length")?;
    if length > MAX_MESSAGE {
        bail!("language server message too large");
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    let message: Value = serde_json::from_slice(&body)?;
    if !message.is_object() {
        bail!("language server message is not an object");
    }
    Ok(message)
}

/// The host is the LSP client: do not send configuration/registration requests to
/// every subscribing window, which would produce duplicate and conflicting replies.
pub(super) fn server_reply(message: &Value) -> Option<Value> {
    let id = message.get("id")?;
    let method = message.get("method")?.as_str()?;
    let result = match method {
        "workspace/configuration" => Value::Array(
            message["params"]["items"]
                .as_array()
                .map(|items| items.iter().map(|_| Value::Null).collect())
                .unwrap_or_default(),
        ),
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create" => Value::Null,
        "workspace/workspaceFolders" => Value::Array(Vec::new()),
        _ => {
            return Some(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Sandhill does not support this server request"}}),
            );
        }
    };
    Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
}
