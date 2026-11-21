use super::*;

impl Bridge {
    /// Runs agy on the user's message, or keeps it for the next turn while
    /// one runs. `shown`: the message shows in the chat. Boxed: a turn
    /// starts the next one.
    pub(super) fn say(
        self: Arc<Self>,
        text: String,
        shown: bool,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(self.say_now(text, shown))
    }

    async fn say_now(self: Arc<Self>, text: String, shown: bool) {
        let _work = self.work.read().await;
        let at_ms = now_ms();
        let id = uuid::Uuid::new_v4().to_string();
        let usage = text.trim() == "/usage";
        let (args, tell, planning) = {
            let mut state = self.state.lock().unwrap();
            if state.paused || state.turn.is_some() || state.waiting > 0 {
                state.queued.push(text);
                return;
            }
            state.turn = Some(id.clone());
            state.stopping = false;
            let tell = !usage && !state.told;
            if !usage {
                state.told = true;
            }
            let planning = !usage && state.permission.as_deref() == Some("plan");
            (turn_args(&self.help, &state), tell, planning)
        };
        let prompt = if shown && !usage {
            self.prompt().await
        } else {
            Default::default()
        };
        let mut input = String::new();
        if tell && crate::lifecycle::antigravity_tools_allowed() {
            input.push_str(&self.instructions);
            input.push_str("\n\n");
        }
        input.push_str(&text);
        if let Some(context) = &prompt.context {
            input.push_str("\n\n");
            input.push_str(context);
        }
        if prompt.report.as_object().is_some_and(|r| !r.is_empty()) {
            self.report(prompt.report.clone()).await;
        }
        let mut turn = Turn {
            id: id.clone(),
            shown,
            ..Turn::default()
        };
        if shown {
            println!("\n› {text}");
            self.chat(json!({"kind": "prompt", "turn": id, "text": text, "place": prompt.place, "at_ms": at_ms}))
                .await;
        }
        self.report(json!({"state": "busy"})).await;
        let errors = if usage {
            match usage::read_usage(&self.agy, Some(&self.cwd)).await {
                Ok(usage) => {
                    self.report(usage.report).await;
                    turn.result = Some(json!({"status":"SUCCESS", "response":usage.text}));
                    Vec::new()
                }
                Err(error) => vec![format!("{error:#}")],
            }
        } else {
            match self.run_agy(&mut turn, args, input).await {
                Ok(errors) => errors,
                Err(err) => vec![format!("{err:#}")],
            }
        };
        let plan = self.end_turn(&mut turn, &errors, planning).await;
        {
            let mut state = self.state.lock().unwrap();
            state.turn = None;
            state.pid = None;
            state.stopping = false;
        }
        match plan {
            // Register before releasing the turn's ownership guard.
            Some(plan) => {
                let generation = {
                    let mut state = self.state.lock().unwrap();
                    if state.paused {
                        None
                    } else {
                        state.waiting += 1;
                        Some(state.generation)
                    }
                };
                if let Some(generation) = generation {
                    tokio::spawn(self.clone().decide_plan(id, plan, generation));
                } else if turn.shown {
                    self.chat(json!({"kind":"stop","turn":id})).await;
                }
            }
            None => {
                // Idle before the turn shows as ended, so a client that sees
                // the end also sees the agent idle.
                self.report(json!({})).await;
                if turn.shown {
                    self.chat(json!({"kind": "stop", "turn": id})).await;
                }
                self.next();
            }
        }
    }

    /// Runs agy once and turns what it says into the chat; returns what it
    /// said on stderr.
    async fn run_agy(
        &self,
        turn: &mut Turn,
        args: Vec<String>,
        input: String,
    ) -> Result<Vec<String>> {
        let mut command = self.conversation_command();
        command
            .args(&args)
            .current_dir(&self.cwd)
            // For Sandhill's MCP server, which agy starts with its own
            // environment: the daemon and the project.
            .env("SANDHILL_SOCKET", &self.daemon)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Its own session and group: stopping it stops what it runs too.
        headless(&mut command);
        if let Some(project) = &self.project {
            command.env("SANDHILL_PROJECT", project);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("cannot start {}", self.agy.display()))?;
        let stopping = {
            let mut state = self.state.lock().unwrap();
            state.pid = child.id();
            state.stopping
        };
        // Stopped while it was starting.
        if stopping && let Some(pid) = child.id() {
            signal_group(pid, libc::SIGINT);
        }
        let mut stdin = child.stdin.take().context("no stdin")?;
        let mut input = serde_json::to_vec(&json!({
            "event": "user",
            "message": {
                "role": "user",
                "content": [{"type": "text", "text": input}],
            },
        }))?;
        input.push(b'\n');
        tokio::spawn(async move {
            let _ = stdin.write_all(&input).await;
            let _ = stdin.shutdown().await;
        });
        let stderr = child.stderr.take().context("no stderr")?;
        let said = Arc::new(Mutex::new(Vec::new()));
        let errors = tokio::spawn({
            let said = said.clone();
            async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    println!("  {line}");
                    said.lock().unwrap().push(line);
                }
            }
        });
        let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
        let mut tick = tokio::time::interval(FLUSH);
        // agy says `init` as soon as it is up. Silence means it is stuck
        // before the message (a sign-in waiting for a browser, say): it is
        // stopped, and the turn fails with what it said.
        let start = tokio::time::sleep(START_TIMEOUT);
        tokio::pin!(start);
        let mut started = false;
        let mut stalled = false;
        loop {
            tokio::select! {
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    started = true;
                    self.event(turn, &line).await;
                }
                _ = tick.tick() => self.flush(turn).await,
                _ = &mut start, if !started => {
                    if let Some(pid) = child.id() {
                        signal_group(pid, libc::SIGKILL);
                    }
                    stalled = true;
                    break;
                }
            }
        }
        let status = child.wait().await?;
        // What it runs may keep stderr open after it ends.
        let _ = tokio::time::timeout(STOP_GRACE, errors).await;
        let mut said = std::mem::take(&mut *said.lock().unwrap());
        if stalled {
            let text = stalled_text(&said);
            said.push(text);
        } else if turn.result.is_none() && !status.success() && said.is_empty() {
            said.push(format!("agy exited ({status})"));
        }
        Ok(said)
    }

    /// One line of agy's NDJSON: `{"event": kind, kind: {...}}`.
    async fn event(&self, turn: &mut Turn, line: &str) {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            if !line.trim().is_empty() {
                println!("{line}");
            }
            return;
        };
        let kind = message["event"].as_str().unwrap_or("");
        let body = &message[kind];
        match kind {
            "init" => {
                // Known from the start, so the command line can open the
                // conversation while its first message runs.
                self.conversation(&message["conversation_id"]).await;
                if let Some(model) = body["model"].as_str() {
                    self.set_model(Some(model));
                    self.report(json!({})).await;
                }
            }
            "step_update" => self.step(turn, body).await,
            "result" => {
                self.conversation(&body["conversation_id"]).await;
                turn.result = Some(body.clone());
            }
            _ => {}
        }
    }

    async fn step(&self, turn: &mut Turn, step: &Value) {
        self.conversation(&step["conversation_id"]).await;
        let index = step["step_index"].as_i64().unwrap_or(-1);
        let done = step["state"] == "DONE";
        match step["step_type"].as_str().unwrap_or("") {
            "agent_response" => {
                let at = match turn.replies.iter().position(|r| r.step == index) {
                    Some(at) => at,
                    None => {
                        turn.replies.push(Reply {
                            step: index,
                            ..Reply::default()
                        });
                        turn.replies.len() - 1
                    }
                };
                let reply = &mut turn.replies[at];
                if let Some(delta) = step["text_delta"].as_str() {
                    reply.pending.push_str(delta);
                    turn.text.push_str(delta);
                }
                if done {
                    reply.done = true;
                    self.flush(turn).await;
                }
            }
            "tool" => {
                let info = &step["tool_info"];
                if !turn.steps.contains_key(&index) && !turn.ended.contains(&index) {
                    // What it wrote before the step shows above it.
                    self.flush(turn).await;
                    let name = info["name"]
                        .as_str()
                        .or(step["tool_name"].as_str())
                        .unwrap_or("tool");
                    let (tool, input) = tool_step(name, &info["parameters"]);
                    println!("  ▸ {tool} {}", one_line(&input));
                    let id = format!("{}-{index}", turn.id);
                    turn.shown = true;
                    self.chat(json!({"kind": "step", "turn": turn.id, "id": id, "tool": tool, "input": input}))
                        .await;
                    turn.steps.insert(index, (id, tool));
                }
                if done && let Some((id, tool)) = turn.steps.remove(&index) {
                    turn.ended.insert(index);
                    let mut end = step_end(step, &tool);
                    if end["kind"] == "step_failed" {
                        println!("    failed");
                    }
                    end["turn"] = json!(turn.id);
                    end["id"] = json!(id);
                    self.chat(end).await;
                }
            }
            _ => {}
        }
    }

    /// Notes the conversation agy says it is in, for the next message and
    /// for going on with it after a restart.
    pub(super) async fn conversation(&self, id: &Value) {
        let Some(id) = id.as_str().filter(|id| !id.is_empty()) else {
            return;
        };
        {
            let mut state = self.state.lock().unwrap();
            if state.conversation.as_deref() == Some(id) {
                return;
            }
            state.conversation = Some(id.to_string());
        }
        self.report(json!({"thread": id})).await;
    }

    /// Sends what agy has written of its replies since the last time.
    async fn flush(&self, turn: &mut Turn) {
        let mut pieces = Vec::new();
        for reply in turn.replies.iter_mut() {
            let ending = reply.done && !reply.finished;
            if reply.pending.is_empty() && !ending {
                continue;
            }
            pieces.push((
                reply.step,
                reply.index,
                std::mem::take(&mut reply.pending),
                ending,
            ));
            reply.index += 1;
            reply.finished |= ending;
        }
        for (step, index, text, last) in pieces {
            turn.shown = true;
            self.chat(json!({
                "kind": "text",
                "turn": turn.id,
                "message": format!("{}-r{step}", turn.id),
                "index": index,
                "text": text,
                "final": last,
            }))
            .await;
        }
    }

    /// Ends the turn in the chat: the rest of the reply, steps that never
    /// ended, and what went wrong. Returns the plan to put to the user, in
    /// plan mode.
    async fn end_turn(&self, turn: &mut Turn, errors: &[String], planning: bool) -> Option<String> {
        for reply in turn.replies.iter_mut() {
            reply.done = true;
        }
        self.flush(turn).await;
        let result = turn.result.clone().unwrap_or(Value::Null);
        let status = result["status"].as_str().unwrap_or("");
        let stopped =
            self.state.lock().unwrap().stopping || matches!(status, "CANCELED" | "INTERRUPTED");
        let open: Vec<(String, String)> = turn.steps.drain().map(|(_, step)| step).collect();
        for (id, tool) in open {
            self.chat(json!({"kind": "step_failed", "turn": turn.id, "id": id, "tool": tool, "error": "Stopped", "interrupted": stopped}))
                .await;
        }
        let response = result["response"].as_str().unwrap_or("");
        if turn.text.trim().is_empty() && !response.trim().is_empty() {
            turn.shown = true;
            self.chat(json!({"kind": "text", "turn": turn.id, "message": format!("{}-r", turn.id), "index": 0, "text": response, "final": true}))
                .await;
        }
        if !response.is_empty() || !turn.text.is_empty() {
            println!(
                "{}",
                if response.is_empty() {
                    &turn.text
                } else {
                    response
                }
                .trim_end()
            );
        }
        let mut problems = Vec::new();
        if !stopped && status != "SUCCESS" {
            problems.push(failure(&result, errors));
        }
        if let Some(denied) = denials(&result, errors) {
            problems.push(denied);
        }
        for text in problems {
            turn.shown = true;
            println!("  Error: {text}");
            self.chat(json!({"kind": "error", "turn": turn.id, "text": text}))
                .await;
        }
        let plan = if response.trim().is_empty() {
            turn.text.trim()
        } else {
            response.trim()
        };
        (planning && status == "SUCCESS" && !plan.is_empty()).then(|| plan.to_string())
    }
}
/// A tool step as the chat shows it: Sandhill's tools by their MCP names,
/// and agy's commands and new files as Claude Code's are; the rest as agy
/// names them.
fn tool_step(name: &str, parameters: &Value) -> (String, Value) {
    if let Some(tool) = sandhill_tool(name) {
        return (format!("mcp__{SERVER}__{tool}"), shorten(parameters));
    }
    match name {
        "run_command" if parameters["CommandLine"].is_string() => {
            ("Bash".into(), json!({"command": parameters["CommandLine"]}))
        }
        "write_to_file" if parameters["TargetFile"].is_string() => (
            "Write".into(),
            shorten(
                &json!({"file_path": parameters["TargetFile"], "content": parameters["CodeContent"]}),
            ),
        ),
        _ => (name.to_string(), shorten(parameters)),
    }
}

/// "add_cell" for one of Sandhill's tools, however agy writes its name:
/// with the server first, joined by `__`, `_`, `/` or `.`.
fn sandhill_tool(name: &str) -> Option<String> {
    let rest = name.strip_prefix("mcp").unwrap_or(name);
    let rest = rest.trim_start_matches(['_', '/', '.']);
    let rest = rest
        .strip_prefix("sandhill-managed")
        .or_else(|| rest.strip_prefix(SERVER))?;
    let tool = rest.trim_start_matches(['_', '/', '.']);
    crate::mcp::tool_names()
        .iter()
        .any(|t| t == tool)
        .then(|| tool.to_string())
}

/// How a tool step ended: `step_done` with its output, or `step_failed`
/// with agy's error. Without the turn and the step's id.
fn step_end(step: &Value, tool: &str) -> Value {
    let info = &step["tool_info"];
    let ms = step["duration_seconds"]
        .as_f64()
        .map(|s| (s * 1000.0).round() as u64);
    let error = &info["error"];
    if !error.is_null() {
        let message = error["message"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| {
                error
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or(error.to_string())
            });
        return json!({"kind": "step_failed", "tool": tool, "error": shorten(&json!(message)), "ms": ms});
    }
    let output = &info["output"];
    let output = match tool {
        "Bash" => {
            json!({"stdout": output.as_str().map(str::to_string).unwrap_or_else(|| text_of(output))})
        }
        _ => output.clone(),
    };
    json!({"kind": "step_done", "tool": tool, "output": shorten(&output), "ms": ms})
}

fn text_of(value: &Value) -> String {
    if value.is_null() {
        String::new()
    } else {
        value.to_string()
    }
}

/// Why a turn failed: agy's error in its result, else what it said on
/// stderr (`AGY_ERROR: {...}` when it can say), with how to sign in when
/// that is what it needs.
fn failure(result: &Value, errors: &[String]) -> String {
    let error = &result["error"];
    let mut text = error["message"]
        .as_str()
        .or(error.as_str())
        .map(str::to_string)
        .or_else(|| {
            errors.iter().find_map(|line| {
                let said = line.strip_prefix("AGY_ERROR:")?.trim();
                let value: Value = serde_json::from_str(said).ok()?;
                Some(
                    value["message"]
                        .as_str()
                        .or(value["error"].as_str())
                        .map(str::to_string)
                        .unwrap_or(said.to_string()),
                )
            })
        })
        .or_else(|| {
            errors
                .iter()
                .rev()
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string())
        })
        .unwrap_or_else(|| "Antigravity stopped without replying".to_string());
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    let lower = text.to_lowercase();
    if [
        "auth",
        "sign in",
        "signed in",
        "login",
        "log in",
        "keyring",
        "credential",
    ]
    .iter()
    .any(|w| lower.contains(w))
    {
        text.push_str(
            " Sign Antigravity in by running `agy` once in a terminal on this machine, or set it up with a Gemini API key.",
        );
    }
    text
}

/// What agy turned down because it couldn't ask, from its result's
/// `denied_actions` or its notices on stderr, as one line for the chat.
fn denials(result: &Value, errors: &[String]) -> Option<String> {
    let mut denied: Vec<String> = result["denied_actions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|a| a.as_str().map(str::to_string).unwrap_or(a.to_string()))
        .collect();
    if denied.is_empty() {
        denied = errors
            .iter()
            .filter(|l| {
                let lower = l.to_lowercase();
                (lower.contains("denied") || lower.contains("not allowed"))
                    && !l.starts_with("AGY_ERROR:")
            })
            .map(|l| l.trim().to_string())
            .collect();
    }
    if denied.is_empty() {
        return None;
    }
    Some(format!(
        "Antigravity can't ask for permission here, so it was turned down: {}. Switch to Skip permissions to let it, or allow it in Antigravity's own settings.",
        denied.join("; ")
    ))
}

/// Why a message failed whose agy never started: how long it had, and the
/// first thing it said (agy asks for a sign-in on stderr, then waits).
fn stalled_text(said: &[String]) -> String {
    let mut text = format!(
        "Antigravity didn't start within {} s, so Sandhill stopped it",
        START_TIMEOUT.as_secs()
    );
    if let Some(line) = said.iter().map(|l| l.trim()).find(|l| !l.is_empty()) {
        let line = line.trim_end_matches(':');
        let line = match line.char_indices().nth(200) {
            Some((at, _)) => format!("{}…", &line[..at]),
            None => line.to_string(),
        };
        text.push_str(". It said: ");
        text.push_str(&line);
    }
    text
}

/// A step's input on one line, for the terminal.
fn one_line(input: &Value) -> String {
    let text = input["command"]
        .as_str()
        .or(input["file_path"].as_str())
        .map(str::to_string)
        .unwrap_or_else(|| input.to_string());
    let line = text.lines().next().unwrap_or("");
    match line.char_indices().nth(100) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_show_as_the_chat_knows_them() {
        for name in [
            "mcp__sandhill__add_cell",
            "mcp_sandhill_add_cell",
            "sandhill/add_cell",
            "sandhill.add_cell",
        ] {
            let (tool, input) = tool_step(name, &json!({"code": "1"}));
            assert_eq!(tool, "mcp__sandhill__add_cell", "{name}");
            assert_eq!(input["code"], "1");
        }
        assert_eq!(
            tool_step("sandhill_nothing", &json!({})).0,
            "sandhill_nothing"
        );
        let (tool, input) = tool_step("run_command", &json!({"CommandLine": "ls", "Cwd": "/p"}));
        assert_eq!((tool.as_str(), &input), ("Bash", &json!({"command": "ls"})));
        assert_eq!(
            tool_step("view_file", &json!({"AbsolutePath": "/a"})).0,
            "view_file"
        );

        let done = json!({"tool_info": {"name": "run_command", "output": "hi\n"}, "duration_seconds": 0.25});
        assert_eq!(
            step_end(&done, "Bash"),
            json!({"kind": "step_done", "tool": "Bash", "output": {"stdout": "hi\n"}, "ms": 250})
        );
        let failed = json!({"tool_info": {"name": "run_command", "error": {"type": "PERMISSION", "message": "Needs approval"}}});
        assert_eq!(step_end(&failed, "Bash")["error"], "Needs approval");
    }

    #[test]
    fn failures_say_why_and_how_to_sign_in() {
        let result = json!({"status": "ERROR", "error": {"message": "model not found"}});
        assert_eq!(failure(&result, &[]), "model not found.");
        let errors = ["AGY_ERROR: {\"message\": \"authentication required\"}".to_string()];
        assert!(
            failure(&Value::Null, &errors)
                .starts_with("authentication required. Sign Antigravity in")
        );
        assert_eq!(failure(&Value::Null, &["boom".into()]), "boom.");

        let notices = ["Tool run_command was denied: allow it with command(ls)".to_string()];
        assert!(
            denials(&json!({}), &notices)
                .unwrap()
                .contains("run_command was denied")
        );
        assert!(
            denials(&json!({"denied_actions": ["command(rm)"]}), &[])
                .unwrap()
                .contains("command(rm)")
        );
        assert_eq!(denials(&json!({}), &["warning: slow".into()]), None);
    }
}
