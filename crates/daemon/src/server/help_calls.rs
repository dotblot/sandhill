//! Calls that search help topics, read help pages and source, and list packages and updates.

use super::*;

impl Daemon {
    pub(super) async fn help_topics(
        &self,
        session: String,
        query: String,
        package: Option<String>,
        limit: Option<u32>,
    ) -> Result<Value> {
        let ask = help::Ask {
            mode: help::Mode::Topics,
            query: query.trim(),
            package: package.as_deref().unwrap_or("").trim(),
            topic: "",
            limit: limit.unwrap_or(help::TOPICS).min(help::MAX_TOPICS),
        };
        let printed = self
            .quietly(&session, "search the help", |language| {
                help::code(language, &ask)
            })
            .await?;
        let found = help::parse_topics(&printed.stdout);
        Ok(serde_json::to_value(HelpTopics {
            session,
            more: found.matched.saturating_sub(found.topics.len() as u64),
            topics: found.topics,
            vignettes: found.vignettes,
        })?)
    }

    pub(super) async fn help_page(
        &self,
        session: String,
        topic: String,
        package: Option<String>,
    ) -> Result<Value> {
        let ask = help::Ask {
            mode: help::Mode::Page,
            query: "",
            package: package.as_deref().unwrap_or("").trim(),
            topic: topic.trim(),
            limit: 0,
        };
        let printed = self
            .quietly(&session, "read the help", |language| {
                help::code(language, &ask)
            })
            .await?;
        let page =
            help::parse_page(&printed.stdout).with_context(|| format!("no help for {topic}"))?;
        Ok(serde_json::to_value(HelpPage {
            session,
            topic: page.topic,
            package: page.package,
            title: page.title,
            version: page.version,
            html: page.html,
            kind: page.kind,
            signature: page.signature,
            text: page.text,
            members: page.members,
            alternatives: page.alternatives,
        })?)
    }

    pub(super) async fn help_source(
        &self,
        session: String,
        topic: String,
        package: Option<String>,
    ) -> Result<Value> {
        let ask = help::Ask {
            mode: help::Mode::Source,
            query: "",
            package: package.as_deref().unwrap_or("").trim(),
            topic: topic.trim(),
            limit: 0,
        };
        let printed = self
            .quietly(&session, "read the source", |language| {
                help::code(language, &ask)
            })
            .await?;
        let source = help::parse_source(&printed.stdout)
            .with_context(|| format!("no source for {topic}"))?;
        Ok(serde_json::to_value(HelpSource {
            session,
            topic: source.topic,
            package: source.package,
            text: source.text,
            file: source.file,
            line: source.line,
        })?)
    }

    pub(super) async fn list_packages(&self, session: String) -> Result<Value> {
        let printed = self
            .quietly(&session, "list the packages", packages::code)
            .await?;
        let listed = packages::parse(&printed.stdout);
        Ok(serde_json::to_value(Packages {
            session,
            packages: listed.packages,
            libraries: listed.libraries,
            cran: listed.cran,
            bioconductor: listed.bioconductor,
            conda: listed.conda,
            program: listed.program,
            prefix: listed.prefix,
            writable_libraries: listed.writable_libraries,
        })?)
    }

    pub(super) async fn package_updates(&self, session: String) -> Result<Value> {
        let printed = self
            .quietly_for(
                &session,
                "check for updates",
                packages::UPDATES_TIMEOUT,
                packages::updates_code,
            )
            .await?;
        let found = packages::parse_updates(&printed.stdout);
        Ok(serde_json::to_value(PackageUpdates {
            session,
            updates: found.updates,
            repos: found.repos,
            unreached: found.unreached,
        })?)
    }
}
