use super::*;

impl Daemon {
    /// Serves one client until it disconnects.
    pub async fn serve_connection<R, W>(self: Arc<Self>, reader: R, writer: W)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out, out_rx) = mpsc::unbounded_channel::<ServerMessage>();
        let stop_reply = Arc::new(Mutex::new(None));
        let writer_task = tokio::spawn(write_loop(
            writer,
            out_rx,
            stop_reply.clone(),
            self.stop.clone(),
        ));
        let subscriptions: Subscriptions = Default::default();
        let tunnels = Tunnels::new(out.clone());
        let connection = self.next_connection.fetch_add(1, Ordering::Relaxed);
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.trim().is_empty() {
                continue;
            }
            let request = match serde_json::from_str::<Request>(&line) {
                Ok(request) => request,
                Err(err) => {
                    let id = serde_json::from_str::<Value>(&line)
                        .ok()
                        .and_then(|v| v.get("id").and_then(Value::as_u64))
                        .unwrap_or(0);
                    let _ = out.send(response(id, Err(anyhow!("bad request: {err}"))));
                    continue;
                }
            };
            // A tunnel's bytes are handled here, not in a task of their own,
            // so they keep their order.
            let request = match request.call {
                call @ (Call::LspStart { .. } | Call::LspSend { .. } | Call::LspStop { .. }) => {
                    // Preserve document synchronization order on the wire.
                    self.clone()
                        .handle(
                            Request {
                                id: request.id,
                                project: request.project,
                                call,
                            },
                            connection,
                            out.clone(),
                            subscriptions.clone(),
                            stop_reply.clone(),
                        )
                        .await;
                    continue;
                }
                Call::OpenTunnel { tunnel, port } => {
                    tunnels.open(request.id, tunnel, port);
                    continue;
                }
                Call::TunnelWrite { tunnel, data } => {
                    tunnels.write(tunnel, &data);
                    continue;
                }
                Call::CloseTunnel { tunnel } => {
                    tunnels.close(tunnel);
                    continue;
                }
                call => Request {
                    id: request.id,
                    project: request.project,
                    call,
                },
            };
            let large = line.len() > memory::LARGE;
            drop(line);
            let daemon = self.clone();
            let out = out.clone();
            let subscriptions = subscriptions.clone();
            let stop_reply = stop_reply.clone();
            tokio::spawn(async move {
                daemon
                    .handle(request, connection, out, subscriptions, stop_reply)
                    .await;
                if large {
                    memory::release_free_memory();
                }
            });
        }
        for (_, task) in subscriptions.lock().unwrap().drain() {
            task.abort();
        }
        drop(tunnels);
        self.windows.disconnected(connection);
        self.language_servers.disconnected(connection).await;
        drop(out);
        let _ = writer_task.await;
    }

    async fn handle(
        self: Arc<Self>,
        request: Request,
        connection: u64,
        out: mpsc::UnboundedSender<ServerMessage>,
        subscriptions: Subscriptions,
        stop_reply: Arc<Mutex<Option<u64>>>,
    ) {
        let id = request.id;
        let shutdown_request =
            matches!(request.call, Call::StopDaemon | Call::PrepareAttach { .. });
        let exclusive = matches!(
            request.call,
            Call::StopResources { .. }
                | Call::CleanStorage { .. }
                | Call::UninstallComponents { .. }
                | Call::StopDaemon
                | Call::PrepareAttach { .. }
        );
        let (_read, _write) = if exclusive {
            (None, Some(self.lifecycle_gate.write().await))
        } else {
            (Some(self.lifecycle_gate.read().await), None)
        };
        if self.stopping.load(Ordering::Acquire) && !matches!(request.call, Call::Ping) {
            let _ = out.send(response(
                id,
                Err(anyhow!(
                    "daemon shutdown is in progress; reconnect after it exits"
                )),
            ));
            return;
        }
        if self.maintenance.load(Ordering::Acquire)
            && !crate::lifecycle::allowed_during_maintenance(&request.call)
        {
            let _ = out.send(response(id, Err(anyhow!("resources are stopped for maintenance; reconnect after an explicit daemon restart"))));
            return;
        }
        if let Err(err) = self.check_trust(&request.call).await {
            let _ = out.send(response(id, Err(err)));
            return;
        }
        let result = match request.call {
            Call::LspStart {
                project,
                language,
                document_kind: _,
                session,
            } => {
                self.start_lsp(project, language, session, connection, out.clone())
                    .await
            }
            Call::LspSend { lsp, message } => self
                .language_servers
                .send(&lsp, connection, message)
                .await
                .map(|()| Value::Null),
            Call::LspStop { lsp } => self
                .language_servers
                .stop(&lsp, connection)
                .await
                .map(|()| Value::Null),
            Call::SetFocus { project, focus } => projects::normalize(&project).map(|project| {
                self.windows.set_focus(connection, &out, project, focus);
                Value::Null
            }),
            Call::Subscribe { session, since_seq } => {
                // Answers and starts streaming itself, so the response always
                // precedes the replayed events.
                if let Err(err) = self.subscribe(id, session, since_seq, &out, &subscriptions) {
                    let _ = out.send(response(id, Err(err)));
                }
                return;
            }
            Call::Unsubscribe { session } => {
                if let Some(task) = subscriptions.lock().unwrap().remove(&session) {
                    task.abort();
                }
                Ok(Value::Null)
            }
            Call::SubscribeTerminal {
                terminal,
                from,
                chat_from,
            } => {
                match self.terminals.open(&terminal, from, chat_from).await {
                    Ok(stream) => {
                        let _ = out.send(response(
                            id,
                            Ok(serde_json::to_value(stream.subscribed()).unwrap_or_default()),
                        ));
                        let task = tokio::spawn(stream.forward(out.clone()));
                        if let Some(old) = subscriptions
                            .lock()
                            .unwrap()
                            .insert(terminal_key(&terminal), task)
                        {
                            old.abort();
                        }
                    }
                    Err(err) => {
                        let _ = out.send(response(id, Err(err)));
                    }
                }
                return;
            }
            Call::UnsubscribeTerminal { terminal } => {
                if let Some(task) = subscriptions
                    .lock()
                    .unwrap()
                    .remove(&terminal_key(&terminal))
                {
                    task.abort();
                }
                Ok(Value::Null)
            }
            Call::GitRun { path, action } => self.git_run(path, action, &out).await,
            call => self.call(call, request.project).await,
        };
        let stopping = shutdown_request && result.is_ok() && self.stopping.load(Ordering::Acquire);
        if stopping {
            *stop_reply.lock().unwrap() = Some(id);
        }
        if out.send(response(id, result)).is_err() && stopping {
            // Even a vanished client must not cancel an accepted shutdown.
            self.stop.notify_one();
        }
    }
    fn subscribe(
        &self,
        id: u64,
        session: String,
        since_seq: Option<i64>,
        out: &mpsc::UnboundedSender<ServerMessage>,
        subscriptions: &Subscriptions,
    ) -> Result<()> {
        let known = self.journal.sessions()?.iter().any(|r| r.id == session);
        if !known {
            anyhow::bail!("no session {session}");
        }
        // Subscribe to live events before reading the journal so nothing
        // published in between is missed; duplicates are filtered by seq.
        let live = self
            .sessions
            .lock()
            .unwrap()
            .get(&session)
            .filter(|s| s.is_running())
            .map(|s| s.subscribe());
        let from_seq = match since_seq {
            Some(seq) => seq,
            None => self.journal.last_seq(&session)?,
        };
        let _ = out.send(response(
            id,
            Ok(serde_json::to_value(Subscribed {
                session: session.clone(),
                from_seq,
            })?),
        ));
        let task = tokio::spawn(forward_events(
            self.journal.clone(),
            session.clone(),
            from_seq,
            live,
            out.clone(),
        ));
        if let Some(old) = subscriptions.lock().unwrap().insert(session, task) {
            old.abort();
        }
        Ok(())
    }

    /// Has the window with `path` open make an edit, if there is one.
    pub(super) async fn ask_window(
        &self,
        path: &std::path::Path,
        project: Option<String>,
        method: &str,
        params: Value,
    ) -> Result<Option<CellEdited>> {
        let project = project.map(|p| projects::normalize(&p)).transpose()?;
        let Some(answer) = self
            .windows
            .ask(path, project.as_deref(), method, params)
            .await
        else {
            return Ok(None);
        };
        let mut answer = answer?;
        answer["file"] = json!(path.to_string_lossy());
        answer["in_window"] = json!(true);
        let edited = serde_json::from_value(answer).context("the window's answer made no sense")?;
        Ok(Some(edited))
    }
}
/// A connection's subscriptions: sessions by id, terminals by [`terminal_key`].
type Subscriptions = Arc<Mutex<HashMap<String, JoinHandle<()>>>>;

fn terminal_key(terminal: &str) -> String {
    format!("terminal:{terminal}")
}

/// Replays journaled events after `from_seq`, then forwards live ones.
async fn forward_events(
    journal: Arc<Journal>,
    session: String,
    mut last_seq: i64,
    live: Option<broadcast::Receiver<Event>>,
    out: mpsc::UnboundedSender<ServerMessage>,
) {
    let replay = |last_seq: &mut i64| -> bool {
        loop {
            let page = match journal.events_since(&session, *last_seq, REPLAY_PAGE) {
                Ok(page) => page,
                Err(err) => {
                    tracing::error!("replay of {session} failed: {err}");
                    return false;
                }
            };
            let done = page.len() < REPLAY_PAGE as usize;
            for event in page {
                *last_seq = event.seq;
                if out.send(ServerMessage::Event { event }).is_err() {
                    return false;
                }
            }
            if done {
                return true;
            }
        }
    };
    if !replay(&mut last_seq) {
        return;
    }
    let Some(mut live) = live else { return };
    loop {
        match live.recv().await {
            Ok(event) if event.seq > last_seq => {
                last_seq = event.seq;
                if out.send(ServerMessage::Event { event }).is_err() {
                    return;
                }
            }
            Ok(_) => {}
            // A slow client fell behind: catch up from the journal.
            Err(broadcast::error::RecvError::Lagged(_)) => {
                if !replay(&mut last_seq) {
                    return;
                }
            }
            Err(broadcast::error::RecvError::Closed) => {
                replay(&mut last_seq);
                return;
            }
        }
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut messages: mpsc::UnboundedReceiver<ServerMessage>,
    stop_reply: Arc<Mutex<Option<u64>>>,
    stop: Arc<Notify>,
) {
    while let Some(message) = messages.recv().await {
        let shutdown = matches!(&message, ServerMessage::Response { id, .. }
            if Some(*id) == *stop_reply.lock().unwrap());
        let Ok(mut line) = serde_json::to_vec(&message) else {
            continue;
        };
        drop(message);
        line.push(b'\n');
        let failed = writer.write_all(&line).await.is_err() || writer.flush().await.is_err();
        if shutdown || (failed && stop_reply.lock().unwrap().is_some()) {
            // A response is an acknowledgement, not proof of exit. Notify only
            // after attempting its flush; attach verifies the process exit.
            stop.notify_one();
        }
        if failed {
            return;
        }
        if line.len() > memory::LARGE {
            drop(line);
            memory::release_free_memory();
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use sandhill_proto::client::Client;
    use std::time::Duration;

    fn connect(daemon: Arc<Daemon>) -> Client {
        let (server, client) = tokio::io::duplex(8192);
        let (reader, writer) = tokio::io::split(server);
        tokio::spawn(daemon.serve_connection(reader, writer));
        let (reader, writer) = tokio::io::split(client);
        Client::new(reader, writer)
    }

    #[tokio::test]
    async fn accepted_shutdown_rejects_later_destructive_requests() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateDir::new(temp.path().join("state"));
        let daemon = Daemon::open(state.clone()).await.unwrap();
        let client = connect(daemon);
        std::fs::create_dir_all(state.root().join("pages/keep")).unwrap();
        std::fs::write(state.root().join("pages/keep/output"), b"preserve").unwrap();
        client.call::<()>(Call::StopDaemon).await.unwrap();
        let error = client
            .call::<Value>(Call::CleanStorage {
                categories: vec!["cache".into()],
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("shutdown is in progress"), "{error}");
        assert!(state.root().join("pages/keep/output").exists());
        assert_eq!(client.ping().await.unwrap().lifecycle, "stopping");
    }

    #[tokio::test]
    async fn attach_cannot_interrupt_maintenance_or_restart_an_uninstall() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateDir::new(temp.path().join("state"));
        let daemon = Daemon::open(state.clone()).await.unwrap();
        let client = connect(daemon.clone());
        // The same exclusive guard held by CleanStorage/UninstallComponents.
        let active = daemon.lifecycle_gate.write().await;
        let mut attach = tokio::spawn(async move {
            client
                .call::<Value>(Call::PrepareAttach {
                    build: "different".into(),
                })
                .await
        });
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut attach)
                .await
                .is_err()
        );
        assert!(!daemon.stopping.load(Ordering::Acquire));
        std::fs::write(state.root().join("uninstalled"), b"fixture uninstall").unwrap();
        drop(active);
        let error = attach.await.unwrap().unwrap_err().to_string();
        assert!(error.contains("uninstalled"), "{error}");
        assert!(!daemon.stopping.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn shutdown_notification_waits_for_reply_flush() {
        let (writer, reader) = tokio::io::duplex(1);
        let (out, incoming) = mpsc::unbounded_channel();
        let stop = Arc::new(Notify::new());
        let task = tokio::spawn(write_loop(
            writer,
            incoming,
            Arc::new(Mutex::new(Some(0))),
            stop.clone(),
        ));
        out.send(response(0, Ok(Value::Null))).unwrap();
        // One byte of capacity keeps the acknowledgement from being flushed.
        assert!(
            tokio::time::timeout(Duration::from_millis(30), stop.notified())
                .await
                .is_err()
        );
        let mut line = String::new();
        BufReader::new(reader).read_line(&mut line).await.unwrap();
        assert!(
            line.contains("\"id\":0") && line.contains(":null"),
            "{line}"
        );
        tokio::time::timeout(Duration::from_secs(1), stop.notified())
            .await
            .unwrap();
        drop(out);
        task.await.unwrap();
    }
}
