use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use sandhill_proto::{ConnectorSpec, SuggestedConnector};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub(super) struct Program {
    pub version: String,
    pub url: String,
    pub sha256: BTreeMap<String, String>,
    pub binary: String,
    pub args: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct Command {
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    needs: String,
    needs_url: String,
}

#[derive(Deserialize)]
struct Http {
    url: String,
}

#[derive(Deserialize)]
pub(super) struct Entry {
    pub id: String,
    pub title: String,
    pub description: String,
    pub read_only: bool,
    by: String,
    licence: String,
    homepage: String,
    terms: String,
    pub program: Option<Program>,
    command: Option<Command>,
    http: Option<Http>,
}

pub(super) fn entries() -> Result<Vec<Entry>> {
    #[derive(Deserialize)]
    struct Catalog {
        connectors: Vec<Entry>,
    }
    Ok(
        serde_json::from_str::<Catalog>(include_str!("../../assets/connectors/catalog.json"))?
            .connectors,
    )
}

impl Entry {
    pub fn suggested(&self, added: bool) -> SuggestedConnector {
        let (action, needs, needs_url) = if let Some(program) = &self.program {
            (
                format!(
                    "Downloads BioMCP {} from github.com/genomoncology/biomcp and runs it on this device",
                    program.version
                ),
                None,
                None,
            )
        } else if let Some(command) = &self.command {
            let missing = !super::programs::available(&command.needs);
            (
                format!(
                    "Runs `{} {}` on this device",
                    command.command,
                    command.args.join(" ")
                ),
                missing.then(|| command.needs.clone()),
                missing.then(|| command.needs_url.clone()),
            )
        } else {
            (
                format!("Calls {}", self.http.as_ref().unwrap().url),
                None,
                None,
            )
        };
        SuggestedConnector {
            id: self.id.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            by: self.by.clone(),
            licence: self.licence.clone(),
            homepage: self.homepage.clone(),
            terms: self.terms.clone(),
            action,
            needs,
            needs_url,
            added,
        }
    }

    pub fn spec(&self, root: &Path) -> Result<(ConnectorSpec, bool)> {
        if let Some(program) = &self.program {
            let (binary, downloaded) = super::programs::download(root, program)?;
            Ok((
                ConnectorSpec::Command {
                    command: binary.to_string_lossy().into_owned(),
                    args: program.args.clone(),
                    env: BTreeMap::new(),
                },
                downloaded,
            ))
        } else if let Some(command) = &self.command {
            if !super::programs::available(&command.needs) {
                bail!(
                    "{} is needed on this device; install it from {}",
                    command.needs,
                    command.needs_url
                );
            }
            Ok((
                ConnectorSpec::Command {
                    command: command.command.clone(),
                    args: command.args.clone(),
                    env: command.env.clone(),
                },
                false,
            ))
        } else {
            Ok((
                ConnectorSpec::Http {
                    url: self
                        .http
                        .as_ref()
                        .context("connector has no address")?
                        .url
                        .clone(),
                    header: None,
                },
                false,
            ))
        }
    }
}
