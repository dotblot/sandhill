use super::*;

impl Bridge {
    /// Reads what Codex says until it exits.
    pub(super) async fn read(self: Arc<Self>, stdout: ChildStdout) {
        let mut lines = BufReader::new(stdout).lines();
        let mut tick = tokio::time::interval(FLUSH);
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    let Ok(message) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    self.clone().handle(message).await;
                }
                _ = tick.tick() => self.flush_replies(None).await,
            }
        }
        // Codex is gone: nothing it was asked will be answered.
        self.rpc.pending.lock().unwrap().clear();
    }

    async fn handle(self: Arc<Self>, message: Value) {
        let method = message["method"].as_str().map(str::to_string);
        match (method, message.get("id")) {
            // A request: Codex asks the user something.
            (Some(method), Some(id)) => {
                self.spawn(
                    self.clone()
                        .request(id.clone(), method, message["params"].clone()),
                );
            }
            (Some(method), None) => self.notification(&method, &message["params"]).await,
            (None, Some(id)) => {
                let Some(id) = id.as_i64() else { return };
                let result = match message.get("error") {
                    Some(error) => Err(error["message"].as_str().unwrap_or("failed").to_string()),
                    None => Ok(message["result"].clone()),
                };
                self.rpc.resolve(id, result);
            }
            (None, None) => {}
        }
    }

    async fn notification(self: &Arc<Self>, method: &str, params: &Value) {
        let turn = params["turnId"].as_str().unwrap_or("").to_string();
        match method {
            "turn/started" => {
                let id = params["turn"]["id"].as_str().unwrap_or("").to_string();
                let stopping = {
                    let mut state = self.state.lock().unwrap();
                    state.turn = Some(id);
                    state.started = true;
                    std::mem::take(&mut state.stopping)
                };
                if stopping {
                    let me = self.clone();
                    self.spawn(async move { me.interrupt().await });
                }
                self.report(json!({})).await;
            }
            "item/started" => self.item_started(&turn, &params["item"]).await,
            "item/completed" => self.item_completed(&turn, &params["item"]).await,
            "item/agentMessage/delta" => {
                let id = params["itemId"].as_str().unwrap_or("").to_string();
                let delta = params["delta"].as_str().unwrap_or("");
                let mut state = self.state.lock().unwrap();
                let reply = state.replies.entry(id).or_default();
                reply.turn = turn;
                reply.pending.push_str(delta);
            }
            "turn/plan/updated" => {
                let n = {
                    let mut state = self.state.lock().unwrap();
                    state.todos += 1;
                    state.todos
                };
                let id = format!("{turn}-todo-{n}");
                let input = items::todos(params);
                self.show_turn(&turn);
                self.chat(json!({"kind": "step", "turn": turn, "id": id, "tool": "TodoWrite", "input": input}))
                    .await;
                self.chat(json!({"kind": "step_done", "turn": turn, "id": id, "tool": "TodoWrite", "output": {}}))
                    .await;
            }
            "error" => {
                let text = params["error"]["message"]
                    .as_str()
                    .unwrap_or("Codex failed");
                if params["willRetry"] == true {
                    println!("  ({text}; trying again)");
                } else {
                    self.state.lock().unwrap().errored = true;
                    self.show_turn(&turn);
                    self.chat(json!({"kind": "error", "turn": turn, "text": text}))
                        .await;
                    println!("  Error: {text}");
                }
            }
            "turn/completed" => self.turn_completed(&params["turn"]).await,
            "thread/tokenUsage/updated" => {
                if let Some(context) = items::context(&params["tokenUsage"]) {
                    let mut state = self.state.lock().unwrap();
                    for (key, value) in context.as_object().into_iter().flatten() {
                        state.status.insert(key.clone(), value.clone());
                    }
                }
                self.report(json!({})).await;
            }
            "account/rateLimits/updated" => {
                self.limits(&params["rateLimits"]);
                self.report(json!({})).await;
            }
            "account/login/completed" => {
                if params["success"] == true {
                    self.report(json!({"login": null})).await;
                    let me = self.clone();
                    self.spawn(async move { me.account().await });
                } else {
                    let error = params["error"].as_str().unwrap_or("Signing in failed");
                    self.report(json!({"login": {"error": error}})).await;
                }
            }
            "account/updated" => {
                let me = self.clone();
                self.spawn(async move { me.account().await });
            }
            "serverRequest/resolved" => {
                // Settled without the user (the turn was stopped): the ask goes.
                let id = params["requestId"].to_string();
                if let Some(waiting) = self.state.lock().unwrap().requests.remove(&id) {
                    waiting.abort();
                }
            }
            "model/rerouted" => {
                let mut state = self.state.lock().unwrap();
                state
                    .status
                    .insert("model".into(), params["toModel"].clone());
            }
            "warning" | "configWarning" | "deprecationNotice" => {
                let text = params["message"]
                    .as_str()
                    .or(params["summary"].as_str())
                    .unwrap_or("");
                println!("  {text}");
            }
            _ => {}
        }
    }

    async fn item_started(&self, turn: &str, item: &Value) {
        let id = item["id"].as_str().unwrap_or("").to_string();
        if item["type"] == "agentMessage" {
            self.state
                .lock()
                .unwrap()
                .replies
                .entry(id)
                .or_default()
                .turn = turn.to_string();
            return;
        }
        if !items::shown_at_start(item) {
            return;
        }
        let Some((tool, input)) = items::step(item, &self.cwd) else {
            return;
        };
        println!("  ▸ {tool} {}", one_line(&input));
        self.show_turn(turn);
        self.state
            .lock()
            .unwrap()
            .steps
            .push((id.clone(), tool.clone(), input.clone()));
        self.chat(json!({"kind": "step", "turn": turn, "id": id, "tool": tool, "input": input}))
            .await;
    }

    pub(super) async fn item_completed(&self, turn: &str, item: &Value) {
        let id = item["id"].as_str().unwrap_or("").to_string();
        match item["type"].as_str().unwrap_or("") {
            "agentMessage" => {
                {
                    // A reply that came whole, with no pieces before.
                    let mut state = self.state.lock().unwrap();
                    let reply = state.replies.entry(id.clone()).or_default();
                    if reply.turn.is_empty() {
                        reply.turn = turn.to_string();
                    }
                }
                self.flush_replies(Some((&id, item["text"].as_str().unwrap_or(""))))
                    .await;
                return;
            }
            "plan" => {
                self.state.lock().unwrap().plan = item["text"].as_str().map(str::to_string);
                return;
            }
            "contextCompaction" => {
                self.chat(json!({"kind": "session", "source": "compact"}))
                    .await;
                println!("  (conversation compacted)");
                return;
            }
            _ => {}
        }
        let started = {
            let mut state = self.state.lock().unwrap();
            let at = state.steps.iter().position(|(step, _, _)| *step == id);
            at.map(|at| state.steps.remove(at))
        };
        let tool = match started {
            Some((_, tool, _)) => tool,
            None => {
                // Shown now that it is complete.
                let Some((tool, input)) = items::step(item, &self.cwd) else {
                    return;
                };
                self.show_turn(turn);
                self.chat(
                    json!({"kind": "step", "turn": turn, "id": id, "tool": tool, "input": input}),
                )
                .await;
                tool
            }
        };
        let mut end = items::step_end(item, &tool);
        if end["kind"] == "step_failed" {
            println!("    failed");
        }
        end["turn"] = json!(turn);
        end["id"] = json!(id);
        self.chat(end).await;
    }

    /// Sends what Codex has written of its replies. With `done` (its id and
    /// text), that reply is complete: its last piece goes, and if the
    /// pieces don't add up to what Codex says it wrote, the whole text
    /// replaces them.
    async fn flush_replies(&self, done: Option<(&str, &str)>) {
        let pieces = {
            let mut state = self.state.lock().unwrap();
            let mut pieces = Vec::new();
            for (id, reply) in state.replies.iter_mut() {
                let finished = done.is_some_and(|(done, _)| done == id);
                if reply.pending.is_empty() && !finished {
                    continue;
                }
                let text = std::mem::take(&mut reply.pending);
                reply.sent.push_str(&text);
                pieces.push((reply.turn.clone(), id.clone(), reply.index, text, finished));
                reply.index += 1;
            }
            if let Some((id, full)) = done {
                let reply = state.replies.remove(id).unwrap_or_default();
                if reply.sent != full {
                    // Replace every piece sent: the first with the whole text.
                    pieces.retain(|(_, piece, ..)| piece != id);
                    for index in 0..reply.index.max(1) {
                        let text = if index == 0 { full } else { "" };
                        pieces.push((
                            reply.turn.clone(),
                            id.to_string(),
                            index,
                            text.to_string(),
                            true,
                        ));
                    }
                }
            }
            pieces
        };
        for (turn, id, index, text, finished) in pieces {
            self.show_turn(&turn);
            self.chat(json!({
                "kind": "text",
                "turn": turn,
                "message": id,
                "index": index,
                "text": text,
                "final": finished,
            }))
            .await;
        }
        if let Some((_, full)) = done {
            println!("{}", full.trim_end());
        }
    }

    pub(super) async fn turn_completed(self: &Arc<Self>, turn: &Value) {
        let id = turn["id"].as_str().unwrap_or("").to_string();
        self.flush_replies(None).await;
        let (plan, errored, shown) = {
            let mut state = self.state.lock().unwrap();
            state.history.insert(id.clone());
            if !state.completed.contains(&id) {
                state.completed.push(id.clone());
            }
            state.turn = None;
            state.started = false;
            state.stopping = false;
            state.steps.clear();
            state.replies.clear();
            let shown = state.shown.contains(&id);
            (state.plan.take(), state.errored, shown)
        };
        if turn["status"] == "failed" && !errored {
            let text = turn["error"]["message"].as_str().unwrap_or("Codex failed");
            self.chat(json!({"kind": "error", "turn": id, "text": text}))
                .await;
            println!("  Error: {text}");
        }
        let planning = self.state.lock().unwrap().permission.as_deref() == Some("plan");
        match plan {
            // A plan to carry out: the turn ends once the user decides.
            Some(plan)
                if planning
                    && turn["status"] == "completed"
                    && !self.state.lock().unwrap().paused =>
            {
                self.spawn(self.clone().decide_plan(id, plan));
            }
            _ => {
                // Idle before the turn shows as ended, so a client that sees
                // the end also sees the agent idle.
                self.report(json!({})).await;
                if shown || turn["status"] == "failed" {
                    self.chat(json!({"kind": "stop", "turn": id})).await;
                }
                self.next();
            }
        }
    }

    /// Asks the user what Codex asks, and gives Codex the answer.
    async fn request(self: Arc<Self>, id: Value, method: String, params: Value) {
        let steps = self.state.lock().unwrap().steps.clone();
        let Some(ask) = items::ask(&method, &params, &steps) else {
            self.rpc
                .reply_error(id, &format!("Sandhill doesn't handle {method}"))
                .await;
            return;
        };
        let ask_id = short_id();
        let turn = params["turnId"]
            .as_str()
            .map(str::to_string)
            .or_else(|| self.state.lock().unwrap().turn.clone());
        let mut event = json!({
            "kind": "ask",
            "turn": turn,
            "id": ask_id,
            "tool": ask.tool,
            "input": ask.input,
            "step": ask.step,
            "at_ms": now_ms(),
        });
        if !ask.remember.is_empty() {
            event["remember"] = json!(ask.remember);
            event["scopes"] = json!(ask.scopes);
        }
        if let Some(map) = event.as_object_mut() {
            map.retain(|_, v| !v.is_null());
        }
        println!("  ? {} (waiting for you)", ask.tool);
        let key = id.to_string();
        // Asked once the agent reports waiting, so a client that sees the
        // ask also sees the agent waiting.
        let (go, ready) = tokio::sync::oneshot::channel::<()>();
        let waiting = tokio::spawn({
            let dir = self.dir.clone();
            async move {
                let _ = ready.await;
                super::ask(&dir, &ask_id, event).await
            }
        });
        {
            let mut state = self.state.lock().unwrap();
            state.waiting += 1;
            state.requests.insert(key.clone(), waiting.abort_handle());
        }
        self.report(json!({"message": format!("Codex asks to use {}", short_tool(&ask.tool))}))
            .await;
        let _ = go.send(());
        let answer = waiting.await;
        {
            let mut state = self.state.lock().unwrap();
            state.waiting -= 1;
            state.requests.remove(&key);
        }
        self.report(json!({})).await;
        match answer {
            Ok(Ok(Some(answer))) => {
                let response = items::response(&method, &params, &answer);
                self.rpc.reply(id, response).await;
            }
            // Codex settled it itself.
            Err(_) => {}
            // The terminal is going away: say no.
            Ok(_) => {
                let response = items::response(&method, &params, &json!({"behavior": "deny"}));
                self.rpc.reply(id, response).await;
            }
        }
    }

    /// Notes that the chat shows this turn, so it ends with `stop`.
    pub(super) fn show_turn(&self, turn: &str) {
        if turn.is_empty() {
            return;
        }
        let mut state = self.state.lock().unwrap();
        if !state.shown.iter().any(|t| t == turn) {
            state.shown.push(turn.to_string());
        }
    }

    pub(super) async fn chat(&self, mut event: Value) {
        if event.get("at_ms").is_none() {
            event["at_ms"] = json!(now_ms());
        }
        if let Some(map) = event.as_object_mut() {
            map.retain(|_, v| !v.is_null());
        }
        let _ = self.host.send(&ToHost::Chat { event }).await;
    }

    /// Reports the agent's state with `extra`: busy, waiting for the user or
    /// idle; the mode; and the status the app shows.
    pub(super) async fn report(&self, extra: Value) {
        let report = {
            let mut state = self.state.lock().unwrap();
            if let Some(mode) = extra["view_mode"].as_str() {
                state.view_mode = mode.to_string();
            }
            let busy = state.turn.is_some();
            let mut status = state.status.clone();
            if let Some(effort) = &state.effort {
                status.insert("effort".into(), json!(effort));
            }
            let mut report = json!({
                "state": if state.waiting > 0 { "waiting" } else if busy { "busy" } else { "idle" },
                "permission": state.permission.as_deref().unwrap_or("supervised"),
                "thread": state.thread,
                "view_mode": if state.view_mode.is_empty() { "chat" } else { &state.view_mode },
                "status": status,
            });
            if !state.models.is_null() {
                report["models"] = state.models.clone();
            }
            if state.waiting > 0 && extra.get("message").is_none() {
                report["message"] = json!("Codex is waiting for you");
            }
            for (key, value) in extra.as_object().into_iter().flatten() {
                report[key] = value.clone();
            }
            report
        };
        let _ = self.host.send(&ToHost::Agent { value: report }).await;
    }
}
/// "add_cell" for Sandhill's tools, "bash" for a command.
fn short_tool(tool: &str) -> String {
    match tool.rsplit_once("__") {
        Some((_, name)) => name.to_string(),
        None if tool == "Bash" => "a command".to_string(),
        None if tool == "apply_patch" => "edit files".to_string(),
        None => tool.to_string(),
    }
}

/// A step's input on one line, for the terminal.
fn one_line(input: &Value) -> String {
    let text = input["command"]
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            input["changes"].as_array().map(|changes| {
                changes
                    .iter()
                    .filter_map(|c| c["path"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
        })
        .unwrap_or_else(|| input.to_string());
    let line = text.lines().next().unwrap_or("");
    match line.char_indices().nth(100) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}
