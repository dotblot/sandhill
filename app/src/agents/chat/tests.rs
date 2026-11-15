use super::step_details::display_name;
use super::*;
use serde_json::json;

fn entries(events: Vec<Value>) -> Vec<Entry> {
    events
        .into_iter()
        .enumerate()
        .map(|(seq, event)| Entry {
            seq: seq as u64,
            event,
        })
        .collect()
}

fn recorded() -> Vec<Entry> {
    let all: Vec<Value> = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/chat.recorded.json"
    ))
    .unwrap();
    all.into_iter()
        .map(|e| Entry {
            seq: e["seq"].as_u64().unwrap(),
            event: e["event"].clone(),
        })
        .collect()
}

fn kinds(chat: &Chat) -> Vec<&'static str> {
    chat.items
        .iter()
        .map(|i| match i {
            Item::User { .. } => "user",
            Item::Reply { .. } => "reply",
            Item::Step { .. } => "step",
            Item::Ask { .. } => "ask",
            Item::Footer { .. } => "footer",
            Item::Note { .. } => "note",
            Item::Account { .. } => "account",
            Item::Error { .. } => "error",
            Item::Thinking { .. } => "thinking",
        })
        .collect()
}

fn steps(chat: &Chat) -> Vec<&Step> {
    chat.items
        .iter()
        .filter_map(|i| match i {
            Item::Step { step, .. } => Some(step),
            _ => None,
        })
        .collect()
}

#[test]
fn acp_approval_options_keep_agent_labels_and_round_trip_every_choice() {
    let options = json!([
        {"optionId": "permit-once", "name": "Run just this time", "kind": "allow_once"},
        {"optionId": "permit-session", "name": "Allow in this session", "kind": "allow_always"},
        {"optionId": "decline-once", "name": "Do not run", "kind": "reject_once"},
        {"optionId": "decline-always", "name": "Never run this tool", "kind": "reject_always"}
    ]);
    let mut recorded = entries(vec![
        json!({"turn": "t", "kind": "prompt", "text": "Check my data"}),
        json!({"turn": "t", "kind": "step", "id": "tool", "tool": "execute",
            "input": {"title": "Inspect the dataset", "kind": "execute"}}),
        json!({"turn": "t", "kind": "ask", "id": "permission", "step": "tool",
            "tool": "execute", "input": {"title": "Inspect the dataset", "kind": "execute"},
            "options": options}),
    ]);
    let chat = build(&recorded, true);
    let ask = &chat.waiting[0];
    assert_eq!(
        ask_view(ask, None, "OpenScience").title,
        "Inspect the dataset"
    );
    let offered = ask.options.as_ref().unwrap();
    assert_eq!(offered.len(), 4);
    assert_eq!(steps(&chat)[0].ask.as_ref().unwrap().options, ask.options);
    for (index, option) in offered.iter().enumerate() {
        assert_eq!(
            Some(option.option_id.as_str()),
            options[index]["optionId"].as_str()
        );
        assert_eq!(Some(option.name.as_str()), options[index]["name"].as_str());
        assert_eq!(Some(option.kind.as_str()), options[index]["kind"].as_str());
        assert_eq!(option.rejects(), index >= 2);
        let answer = option.answer();
        assert_eq!(answer, json!({"option_id": options[index]["optionId"]}));
        recorded.push(Entry {
            seq: recorded.len() as u64,
            event: json!({"kind": "answered", "id": "permission", "answer": answer}),
        });
        let answered = build(&recorded, true);
        assert!(answered.waiting.is_empty());
        let answered_steps = steps(&answered);
        let saved = answered_steps[0].ask.as_ref().unwrap();
        assert_eq!(saved.answer.as_ref().unwrap(), &option.answer());
        assert_eq!(saved.options.as_ref().unwrap()[index].name, option.name);
        assert_eq!(saved.answered_option(), Some(option));
        assert_eq!(
            answered_steps[0].status,
            if option.rejects() {
                StepStatus::Deny
            } else {
                StepStatus::Run
            }
        );
        recorded.pop();
    }
}

#[test]
fn acp_empty_options_do_not_become_legacy_permission_scopes() {
    let chat = build(
        &entries(vec![
            json!({"turn": "t", "kind": "prompt", "text": "Check"}),
            json!({"turn": "t", "kind": "ask", "id": "acp", "tool": "Read", "options": []}),
            json!({"turn": "t", "kind": "ask", "id": "legacy", "tool": "Read",
                "scopes": ["chat"], "remember": ["Read"]}),
        ]),
        true,
    );
    assert_eq!(chat.waiting[0].options, Some(Vec::new()));
    assert_eq!(chat.waiting[1].options, None);
    assert_eq!(chat.waiting[1].scopes, Some(vec!["chat".into()]));
    assert_eq!(chat.waiting[1].remember, ["Read"]);
}

#[test]
fn acp_tool_metadata_preserves_original_input_and_location() {
    let view = step_view(
        "Bash",
        &json!({
            "title": "Inspect the dataset",
            "kind": "execute",
            "locations": [{"path": "/p/data.csv", "line": 1}],
            "rawInput": {"command": "wc -l data.csv"},
            "content": [{"type": "content", "content": {"type": "text", "text": "Count rows"}}]
        }),
        None,
        None,
    );
    assert_eq!(view.desc, "Inspect the dataset");
    assert_eq!(view.code.as_deref(), Some("wc -l data.csv"));
    assert_eq!(view.file.as_deref(), Some("/p/data.csv"));
    assert_eq!(view.output.as_deref(), Some("Count rows"));
}

#[test]
fn a_streamed_step_repeated_with_more_text_stays_one_row() {
    let chat = build(
        &entries(vec![
            json!({"turn": "t", "kind": "prompt", "text": "hi", "at_ms": 1}),
            json!({"turn": "t", "kind": "step", "id": "r", "tool": "reasoning", "input": {"text": "The"}, "at_ms": 2}),
            json!({"turn": "t", "kind": "step", "id": "r", "tool": "reasoning", "input": {"text": "The user"}, "at_ms": 3}),
            json!({"turn": "t", "kind": "step_done", "id": "r", "tool": "reasoning", "output": {"text": "The user asks"}, "at_ms": 4}),
            json!({"turn": "t", "kind": "stop", "at_ms": 5}),
        ]),
        false,
    );
    let steps = steps(&chat);
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].input["text"], "The user");
    assert!(steps[0].output.is_some() && steps[0].error.is_none());
}

#[test]
fn shows_a_recorded_conversation() {
    let chat = build(&recorded(), false);
    assert_eq!(
        kinds(&chat),
        ["user", "step", "step", "step", "step", "reply", "footer"]
    );
    assert!(
        chat.title
            .as_deref()
            .unwrap()
            .starts_with("Use add_cell to add a cell")
    );
    assert!(chat.waiting.is_empty());
    let steps = steps(&chat);
    assert_eq!(
        steps
            .iter()
            .map(|s| display_name(&s.tool))
            .collect::<Vec<_>>(),
        ["get_focus", "list_sessions", "bash", "add_cell"]
    );
    assert!(steps.iter().all(|s| s.status == StepStatus::Done));
    assert_eq!(
        steps[3].ask.as_ref().unwrap().answer,
        Some(json!({"behavior": "allow"}))
    );
    assert!(steps[2].ask.is_none());
    let Item::Footer { text, ms, .. } = &chat.items[6] else {
        panic!()
    };
    assert!(text.starts_with("I added an R cell"));
    assert!(ms.unwrap() > 0);
}

#[test]
fn waits_for_an_answer_while_the_turn_goes_on() {
    let unanswered: Vec<Entry> = recorded()
        .into_iter()
        .filter(|e| e.event["kind"] != "stop" && e.event["kind"] != "text" && e.seq < 14)
        .collect();
    let chat = build(&unanswered, true);
    assert_eq!(chat.waiting.len(), 1);
    assert_eq!(display_name(&chat.waiting[0].tool), "add_cell");
    let add = steps(&chat)
        .into_iter()
        .find(|s| s.tool.ends_with("add_cell"))
        .unwrap();
    assert_eq!(add.status, StepStatus::Wait);
    assert_eq!(kinds(&chat).last(), Some(&"step"));
}

#[test]
fn puts_parts_together_and_marks_stopped_and_denied_steps() {
    let chat = build(
        &entries(vec![
            json!({"turn": "t1", "kind": "prompt", "text": "hi", "at_ms": 1}),
            json!({"turn": "t1", "kind": "text", "message": "m", "index": 1, "text": "world", "at_ms": 3}),
            json!({"turn": "t1", "kind": "text", "message": "m", "index": 0, "text": "Hello ", "at_ms": 2}),
            json!({"turn": "t1", "kind": "stop", "at_ms": 4}),
        ]),
        false,
    );
    assert_eq!(
        chat.items[2],
        Item::Footer {
            key: "ft1".into(),
            at_ms: Some(4),
            ms: Some(3),
            text: "Hello world".into()
        }
    );
    let chat = build(
        &entries(vec![
            json!({"turn": "t1", "kind": "prompt", "text": "one", "at_ms": 1}),
            json!({"turn": "t1", "kind": "step", "id": "a", "tool": "Bash", "input": {"command": "rm x"}, "at_ms": 2}),
            json!({"turn": "t1", "kind": "ask", "id": "q1", "tool": "Bash", "input": {"command": "rm x"}, "at_ms": 3}),
            json!({"kind": "answered", "id": "q1", "answer": {"behavior": "deny"}, "at_ms": 4}),
            json!({"turn": "t2", "kind": "prompt", "text": "two", "at_ms": 5}),
            json!({"turn": "t2", "kind": "step", "id": "b", "tool": "Bash", "input": {"command": "sleep 100"}, "at_ms": 6}),
        ]),
        false,
    );
    assert_eq!(
        steps(&chat).iter().map(|s| s.status).collect::<Vec<_>>(),
        [StepStatus::Deny, StepStatus::Cancel]
    );
    assert!(chat.waiting.is_empty());
}

#[test]
fn shows_questions_codex_requests_and_errors() {
    let chat = build(
        &entries(vec![
            json!({"turn": "t1", "kind": "prompt", "text": "plan it", "at_ms": 1}),
            json!({"turn": "t1", "kind": "ask", "id": "q", "tool": "AskUserQuestion", "input": {"questions": [{"question": "Which?", "options": [{"label": "A"}]}]}, "at_ms": 2}),
        ]),
        true,
    );
    assert_eq!(kinds(&chat), ["user", "ask"]);
    let view = ask_view(&chat.waiting[0], None, "Claude");
    assert_eq!(view.questions[0].question, "Which?");

    let chat = build(
        &entries(vec![
            json!({"turn": "t1", "kind": "prompt", "text": "make a file", "at_ms": 1}),
            json!({"turn": "t1", "kind": "step", "id": "call_1", "tool": "Bash", "input": {"command": "echo hi"}, "at_ms": 2}),
            json!({"turn": "t1", "kind": "ask", "id": "q1", "step": "call_1", "tool": "Bash", "input": {"command": "echo hi", "description": "Say hi"}, "scopes": ["chat"], "at_ms": 3}),
        ]),
        true,
    );
    assert_eq!(kinds(&chat), ["user", "step"]);
    assert_eq!(steps(&chat)[0].status, StepStatus::Wait);

    let chat = build(
        &entries(vec![
            json!({"kind": "session", "source": "startup"}),
            json!({"kind": "error", "text": "Codex couldn't compact"}),
            json!({"turn": "t1", "kind": "prompt", "text": "hi", "at_ms": 1}),
            json!({"turn": "t1", "kind": "error", "text": "Usage limit", "at_ms": 2}),
            json!({"turn": "t1", "kind": "stop", "at_ms": 3}),
            json!({"kind": "session", "source": "compact"}),
        ]),
        false,
    );
    assert_eq!(kinds(&chat), ["error", "user", "error", "footer", "note"]);
}

#[test]
fn describes_steps() {
    let edit = step_view(
        "Edit",
        &json!({"file_path": "/p/a.R", "old_string": "x <- 1", "new_string": "x <- 2\ny <- 3"}),
        None,
        None,
    );
    assert_eq!(edit.desc, "Edit a.R");
    assert_eq!(edit.detail.as_deref(), Some("+2 −1"));
    let run = step_view(
        "mcp__sandhill__run_code",
        &json!({"code": "dim(seu)"}),
        Some(
            &json!([{"type": "text", "text": "Ran in R session 1a2b (R 4.4.1) · idle · ok · 0.5 s\n[1] 10 20"}]),
        ),
        None,
    );
    assert_eq!(run.desc, "dim(seu)");
    assert_eq!(run.detail.as_deref(), Some("R · 0.5 s"));
    assert_eq!(run.output.as_deref(), Some("[1] 10 20"));
    let patch = step_view(
        "apply_patch",
        &json!({"changes": [{"path": "/p/x.py", "kind": "update", "diff": "@@ -1 +1 @@\n-a\n+b\n@@ -9 +9 @@\n c"}]}),
        None,
        None,
    );
    assert_eq!(patch.desc, "Edit x.py");
    assert_eq!(patch.diff.unwrap(), ["-a", "+b", " ⋯", " c"]);
    assert_eq!(
        display_name("mcp__github__create_issue"),
        "github · create_issue"
    );
}

#[test]
fn titles_from_the_first_message() {
    assert_eq!(
        title_of(
            "The `markers` chunk is stale. Re-run it and tell me which cell type each cluster is."
        ),
        "The markers chunk is stale"
    );
    assert_eq!(
        title_of("Please check the QC thresholds in 01_qc.R against this dataset"),
        "Check the QC thresholds"
    );
    assert_eq!(
        title_of("can you plot @analysis.qmd the UMAP of"),
        "Plot the UMAP"
    );
    assert_eq!(
        title_of("Write results/cd14_stim_vs_ctrl.csv from the DE notebook"),
        "Write from the DE notebook"
    );
    assert_eq!(title_of("data.csv"), "New conversation");
}
