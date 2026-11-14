//! Exclusive ownership handoff between headless turns and the official TUI.

use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{BufReader, Lines};
use tokio::net::unix::OwnedReadHalf;

use super::{
    Bridge, COMMAND_TIMEOUT, account_command, conversation_args, headless, history, turn_args,
};

type Commands = Lines<BufReader<OwnedReadHalf>>;

impl Bridge {
    pub(super) async fn native(self: Arc<Self>, commands: &mut Commands) -> Vec<Value> {
        {
            let mut state = self.state.lock().unwrap();
            state.paused = true;
            state.view_mode = Some("switching_terminal");
            state.generation += 1;
            state.waiting = 0;
        }
        self.plan_cancel.notify_waiters();
        self.report(json!({})).await;
        self.stop_active(false);
        // A turn may still be preparing context or spawning its process. Its
        // read guard is not released until that process has actually exited.
        let ownership = self.work.write().await;
        let mut queued = Vec::new();
        // Said once, in the app's notice the user dismisses, not in the chat.
        let error = self
            .native_owned(commands, &mut queued)
            .await
            .err()
            .map(|error| format!("Antigravity's command line failed: {error:#}"));
        if let Some(error) = &error {
            println!("  Error: {error}");
        }
        {
            let mut state = self.state.lock().unwrap();
            state.paused = false;
            state.view_mode = Some("chat");
        }
        self.report(json!({"view_error": error})).await;
        drop(ownership);
        // The helper has reaped native and history loading has released ACP
        // before anything can launch another headless conversation turn.
        self.next();
        queued
    }

    async fn native_owned(&self, commands: &mut Commands, queued: &mut Vec<Value>) -> Result<()> {
        let known = self.state.lock().unwrap().conversation.clone();
        let conversation = match known {
            Some(conversation) => conversation,
            None => self.start_conversation().await?,
        };
        let args = conversation_args(&self.help, &self.state.lock().unwrap());
        let before = history::load(self, &conversation).await?;
        let mut command = account_command(&self.agy);
        command
            .args(args)
            .current_dir(&self.cwd)
            .env("SANDHILL_SOCKET", &self.daemon);
        if let Some(project) = &self.project {
            command.env("SANDHILL_PROJECT", project);
        }
        self.state.lock().unwrap().view_mode = Some("terminal");
        self.report(json!({})).await;
        let native = super::super::codex::native::run(&mut command, &self.dir, commands).await;
        self.state.lock().unwrap().view_mode = Some("switching_chat");
        self.report(json!({})).await;
        let native = native.map(|commands| queued.extend(commands));
        // Import even after a nonzero native exit: it can have saved turns.
        let imported = history::load(self, &conversation).await;
        if let Ok(after) = &imported {
            history::import(self, &conversation, &before, after).await?;
        }
        native?;
        imported?;
        Ok(())
    }

    /// A conversation for the command line when no message has started one:
    /// agy's headless mode given no message starts one, names it in `init`
    /// and ends, without a model turn. Reaped before the command line opens.
    async fn start_conversation(&self) -> Result<String> {
        let args = turn_args(&self.help, &self.state.lock().unwrap());
        let mut command = account_command(&self.agy);
        command
            .args(args)
            .current_dir(&self.cwd)
            .env("SANDHILL_SOCKET", &self.daemon)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(project) = &self.project {
            command.env("SANDHILL_PROJECT", project);
        }
        headless(&mut command);
        let child = command
            .spawn()
            .with_context(|| format!("cannot start {}", self.agy.display()))?;
        let output = tokio::time::timeout(COMMAND_TIMEOUT, child.wait_with_output())
            .await
            .context("agy took too long to start a conversation")??;
        let started = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .find(|event| event["event"] == "init")
            .and_then(|init| init["conversation_id"].as_str().map(str::to_string))
            .filter(|id| !id.is_empty());
        let Some(conversation) = started else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            match stderr.lines().map(str::trim).find(|l| !l.is_empty()) {
                Some(said) => bail!("agy started no conversation ({}): {said}", output.status),
                None => bail!("agy started no conversation ({})", output.status),
            }
        };
        self.conversation(&json!(conversation)).await;
        Ok(conversation)
    }
}
