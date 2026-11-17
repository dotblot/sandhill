use super::*;
use std::collections::BTreeMap;

pub(super) struct Form {
    pub name: Entity<InputState>,
    pub address: Entity<InputState>,
    pub header: Entity<InputState>,
    pub secret: Entity<InputState>,
    pub client_id: Entity<InputState>,
    pub command: Entity<InputState>,
    pub env: Entity<TextareaState>,
}

impl Form {
    pub fn new(window: &mut Window, cx: &mut Context<ConnectorsView>) -> (Self, Vec<Subscription>) {
        let form = Self {
            name: cx.new(|cx| InputState::new(window, cx).placeholder("Name")),
            address: cx.new(|cx| InputState::new(window, cx).placeholder("https://…/mcp")),
            header: cx
                .new(|cx| InputState::new(window, cx).placeholder("Authorization or X-API-Key")),
            secret: cx.new(|cx| {
                InputState::new(window, cx)
                    .masked(true)
                    .placeholder("Header value")
            }),
            client_id: cx.new(|cx| InputState::new(window, cx).placeholder("Client ID")),
            command: cx.new(|cx| InputState::new(window, cx).placeholder("Command and arguments")),
            env: cx.new(|cx| TextareaState::new(window, cx).placeholder("KEY=value, one per line")),
        };
        let mut subscriptions = Vec::new();
        for input in [
            &form.name,
            &form.address,
            &form.header,
            &form.secret,
            &form.client_id,
            &form.command,
        ] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                |this, _, event: &InputEvent, window, cx| {
                    if matches!(event, InputEvent::PressEnter { .. }) {
                        this.primary(window, cx);
                    }
                    cx.notify();
                },
            ));
        }
        subscriptions.push(cx.subscribe(&form.env, |_, _, _: &InputEvent, cx| cx.notify()));
        (form, subscriptions)
    }

    fn reset(&self, window: &mut Window, cx: &mut Context<ConnectorsView>) {
        for input in [
            &self.name,
            &self.address,
            &self.header,
            &self.secret,
            &self.client_id,
            &self.command,
        ] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.env
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.name.update(cx, |input, cx| input.focus(window, cx));
    }

    pub fn ready(&self, card: &Card, cx: &App) -> bool {
        match card {
            Card::Web => {
                !self.name.read(cx).value().trim().is_empty()
                    && !self.address.read(cx).value().trim().is_empty()
            }
            Card::Command => {
                !self.name.read(cx).value().trim().is_empty()
                    && !self.command.read(cx).value().trim().is_empty()
            }
            Card::Secret { .. } => !self.secret.read(cx).value().is_empty(),
            _ => true,
        }
    }

    pub fn web(&self, cx: &App) -> ConnectorAdd {
        let header = self.header.read(cx).value().trim().to_string();
        let client_id = self.client_id.read(cx).value().trim().to_string();
        ConnectorAdd::Custom {
            title: self.name.read(cx).value().trim().to_string(),
            spec: ConnectorSpec::Http {
                url: self.address.read(cx).value().trim().to_string(),
                header: (!header.is_empty()).then_some(header.clone()),
            },
            secret: (!header.is_empty()).then(|| self.secret.read(cx).value().to_string()),
            client_id: (!client_id.is_empty()).then_some(client_id),
        }
    }

    pub fn command(&self, cx: &App) -> anyhow::Result<ConnectorAdd> {
        let mut words = split_command(&self.command.read(cx).value())?.into_iter();
        let command = words
            .next()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Enter a command."))?;
        Ok(ConnectorAdd::Custom {
            title: self.name.read(cx).value().trim().to_string(),
            spec: ConnectorSpec::Command {
                command,
                args: words.collect(),
                env: environment(&self.env.read(cx).value())?,
            },
            secret: None,
            client_id: None,
        })
    }
}

impl ConnectorsView {
    pub(super) fn open_form(&mut self, command: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.open_card(if command { Card::Command } else { Card::Web }, window, cx);
        self.form.reset(window, cx);
    }
}

/// Only tokenization: no shell expansion, pipes, or command substitution.
fn split_command(line: &str) -> anyhow::Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for ch in line.chars() {
        if escaped {
            word.push(ch);
            escaped = false;
            started = true;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                word.push(ch);
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
            started = true;
        } else if ch.is_whitespace() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(ch);
            started = true;
        }
    }
    anyhow::ensure!(!escaped, "The command ends with an unfinished escape.");
    anyhow::ensure!(quote.is_none(), "Close the command's quoted text.");
    if started {
        words.push(word);
    }
    Ok(words)
}

fn environment(text: &str) -> anyhow::Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for line in text.lines().filter(|s| !s.trim().is_empty()) {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("Environment lines must be KEY=value."))?;
        let key = key.trim();
        anyhow::ensure!(!key.is_empty(), "Environment names cannot be empty.");
        env.insert(key.to_string(), value.to_string());
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::{environment, split_command};

    #[test]
    fn command_quotes_and_escapes() {
        assert_eq!(
            split_command(r#"uvx --with 'a b' "c d" escaped\ space "" a"b"c"#).unwrap(),
            ["uvx", "--with", "a b", "c d", "escaped space", "", "abc"]
        );
        assert_eq!(
            split_command(r#"tool "a\"b" 'c\d'"#).unwrap(),
            ["tool", "a\"b", "c\\d"]
        );
        assert!(split_command("tool 'unfinished").is_err());
        assert!(split_command("tool \\").is_err());
        assert!(split_command("   ").unwrap().is_empty());
    }

    #[test]
    fn environment_lines_keep_values() {
        let env = environment("A=x=y\n\n B = spaced \nEMPTY=").unwrap();
        assert_eq!(env["A"], "x=y");
        assert_eq!(env["B"], " spaced ");
        assert_eq!(env["EMPTY"], "");
        assert!(environment("broken").is_err());
        assert!(environment("=value").is_err());
    }
}
