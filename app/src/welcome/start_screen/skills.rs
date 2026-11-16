//! Welcome → Skills: device-owned instructions shared by every agent
//! (Huan, 2026-11-16; `docs/design/done/skills-plan.md`).

use super::*;
use sandhill_proto::{SkillExport, SkillFetch, SkillInfo, SkillList, SkillPreview, SkillSource};

mod card;
mod menu;
mod page;
mod upload;

#[derive(Clone)]
struct Destination {
    host: String,
    daemon: Rc<Daemon>,
}

#[derive(Clone)]
enum Completion {
    Reload,
    Copied { name: String, device: String },
}

struct Confirmation {
    preview: SkillPreview,
    selected: Vec<bool>,
    daemon: Rc<Daemon>,
    completion: Completion,
    update: bool,
    title: Option<String>,
}

enum Card {
    Github,
    Busy(String),
    Confirm(Confirmation),
    Files {
        skill: SkillInfo,
        paths: Vec<String>,
    },
    Remove(String),
    Error(String),
}

#[derive(Clone)]
enum Menu {
    Add,
    Skill(String),
    Copy(String),
}

pub(crate) struct SkillsView {
    daemon: Rc<Daemon>,
    host: String,
    destinations: Vec<Destination>,
    list: Option<SkillList>,
    problem: Option<String>,
    loading: bool,
    busy: bool,
    generation: u64,
    notice: Option<(String, String)>,
    card: Option<Card>,
    menu: Option<(Menu, Point<Pixels>)>,
    focus: FocusHandle,
    input: Entity<InputState>,
    triggers: crate::shell::popup::PopupTriggers,
    _input_subscription: Subscription,
}

impl SkillsView {
    fn new(daemon: Rc<Daemon>, host: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("owner/repo or a GitHub address"));
        let subscription =
            cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. })
                    && matches!(this.card, Some(Card::Github))
                {
                    this.continue_github(window, cx);
                }
                cx.notify();
            });
        let mut this = Self {
            daemon,
            host,
            destinations: Vec::new(),
            list: None,
            problem: None,
            loading: false,
            busy: false,
            generation: 0,
            notice: None,
            card: None,
            menu: None,
            focus: cx.focus_handle(),
            input,
            triggers: Default::default(),
            _input_subscription: subscription,
        };
        this.load(cx);
        this
    }

    fn error_text(error: anyhow::Error) -> String {
        let text = format!("{error:#}");
        if text.contains("unknown variant") {
            "Update Sandhill on this device to manage its skills.".into()
        } else {
            text
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let asked = self.daemon.call::<SkillList>(Call::ListSkills);
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(list) => {
                        this.list = Some(list);
                        this.problem = None;
                    }
                    Err(error) => this.problem = Some(Self::error_text(error)),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_card(&mut self, card: Card, window: &mut Window, cx: &mut Context<Self>) {
        self.notice = None;
        self.menu = None;
        self.card = Some(card);
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.generation += 1;
        self.card = None;
        self.menu = None;
        cx.notify();
    }

    fn fail(&mut self, error: anyhow::Error, cx: &mut Context<Self>) {
        let text = Self::error_text(error);
        if text.starts_with("Update Sandhill") {
            self.problem = Some(text.clone());
        }
        self.card = Some(Card::Error(text));
        self.busy = false;
        cx.notify();
    }

    fn preview(&mut self, fetch: SkillFetch, window: &mut Window, cx: &mut Context<Self>) {
        let update_name = match &fetch {
            SkillFetch::Update { name } => Some(name.clone()),
            _ => None,
        };
        self.open_card(Card::Busy("Fetching…".into()), window, cx);
        self.generation += 1;
        let generation = self.generation;
        let asked = self
            .daemon
            .call::<SkillPreview>(Call::PreviewSkills { fetch });
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok(preview) => {
                        if let Some(name) = update_name.as_ref()
                            && preview.refused.is_empty()
                            && !preview.skills.is_empty()
                            && preview.skills.iter().all(|s| !s.changed)
                        {
                            this.card = None;
                            this.notice = Some((name.clone(), "Up to date".into()));
                        } else {
                            this.card = Some(Card::Confirm(Confirmation {
                                selected: vec![true; preview.skills.len()],
                                preview,
                                daemon: this.daemon.clone(),
                                completion: Completion::Reload,
                                update: update_name.is_some(),
                                title: None,
                            }));
                        }
                    }
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn continue_github(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let url = self.input.read(cx).value().trim().to_string();
        if !url.is_empty() {
            self.preview(SkillFetch::Git { url, commit: None }, window, cx);
        }
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        let Some(Card::Confirm(confirm)) = self.card.as_ref() else {
            return;
        };
        let names: Vec<_> = confirm
            .preview
            .skills
            .iter()
            .zip(&confirm.selected)
            .filter(|(_, selected)| **selected)
            .map(|(s, _)| s.name.clone())
            .collect();
        if names.is_empty() || self.busy {
            return;
        }
        let asked = confirm.daemon.call::<Vec<SkillInfo>>(Call::InstallSkills {
            token: confirm.preview.token.clone(),
            names,
        });
        let completion = confirm.completion.clone();
        self.busy = true;
        self.card = Some(Card::Busy("Installing…".into()));
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        this.card = None;
                        match completion {
                            Completion::Reload => this.load(cx),
                            Completion::Copied { name, device } => {
                                this.notice = Some((name, format!("Copied to {device}")))
                            }
                        }
                    }
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn mutate(&mut self, call: Call, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.notice = None;
        self.busy = true;
        let asked = self.daemon.call::<serde_json::Value>(call);
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(_) => {
                        this.card = None;
                        this.load(cx);
                    }
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn show_files(&mut self, skill: SkillInfo, window: &mut Window, cx: &mut Context<Self>) {
        self.open_card(Card::Busy("Reading files…".into()), window, cx);
        self.generation += 1;
        let generation = self.generation;
        let asked = self.daemon.call::<SkillExport>(Call::ExportSkill {
            name: skill.name.clone(),
        });
        cx.spawn(async move |this, cx| {
            let result = asked.await;
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok(export) => {
                        this.card = Some(Card::Files {
                            skill,
                            paths: export.files.into_iter().map(|f| f.path).collect(),
                        })
                    }
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn copy(
        &mut self,
        name: String,
        destination: Destination,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_card(Card::Busy("Fetching…".into()), window, cx);
        self.generation += 1;
        let generation = self.generation;
        let export = self
            .daemon
            .call::<SkillExport>(Call::ExportSkill { name: name.clone() });
        cx.spawn(async move |this, cx| {
            let result = async {
                let export = export.await?;
                destination
                    .daemon
                    .call::<SkillPreview>(Call::PreviewSkills {
                        fetch: SkillFetch::Files {
                            label: export.name,
                            files: export.files,
                            source: Some(export.source),
                        },
                    })
                    .await
            }
            .await;
            this.update(cx, |this, cx| {
                if generation != this.generation {
                    return;
                }
                match result {
                    Ok(preview) => {
                        let replaces = preview.skills.iter().any(|s| s.replaces);
                        let direct =
                            !replaces && preview.refused.is_empty() && !preview.skills.is_empty();
                        let device = host_label(&destination.host);
                        this.card = Some(Card::Confirm(Confirmation {
                            selected: vec![true; preview.skills.len()],
                            preview,
                            daemon: destination.daemon,
                            completion: Completion::Copied {
                                name: name.clone(),
                                device: device.clone(),
                            },
                            update: false,
                            title: replaces.then(|| format!("Replace {name} on {device}?")),
                        }));
                        if direct {
                            this.install(cx);
                        }
                    }
                    Err(error) => this.fail(error, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// An installed skill's grey line: where it came from and its licence.
fn source_label(skill: &SkillInfo) -> String {
    let from = match &skill.source {
        SkillSource::Git { url, .. } => {
            let url = url.trim_end_matches('/').trim_end_matches(".git");
            url.strip_prefix("https://")
                .or_else(|| url.strip_prefix("http://"))
                .map(str::to_owned)
                .unwrap_or_else(|| url.replace("git@github.com:", "github.com/"))
        }
        SkillSource::Upload { label } => label.clone(),
    };
    let licence = skill.licence.as_deref().unwrap_or("No licence file");
    format!("{from} · {licence}")
}
