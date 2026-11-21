use std::collections::BTreeSet;
use std::fs;
use std::io::Read;

use anyhow::ensure;
use sandhill_proto::ProjectFilePlan;

use super::package_calls::choose_spec;
use super::package_scan::{self, Language};
use super::*;
use crate::kernelspecs::{self, Search};
use crate::quick::{
    install,
    project_packages::{self, Runtime},
};

const MAX_SCRIPT: u64 = 4 * 1024 * 1024;

impl Daemon {
    async fn project_package_context(&self, project: String) -> Result<(PathBuf, bool)> {
        let journal = self.journal.clone();
        blocking(move || {
            let path = projects::normalize(&project)?;
            let root = PathBuf::from(&path)
                .canonicalize()
                .context("The project's folder couldn't be found.")?;
            ensure!(root.is_dir(), "Choose a project folder.");
            let trusted = projects::trust::is_trusted(&journal, &path)?;
            Ok((root, trusted))
        })
        .await
    }

    pub(super) async fn project_package_files(&self, project: String) -> Result<Value> {
        let (root, trusted) = self.project_package_context(project).await?;
        let own = self.own_folder();
        Ok(serde_json::to_value(
            blocking(move || {
                let search = Search::from_env().in_project(Some(&root)).in_own(own);
                Ok(project_packages::list(&root, &search, |language| {
                    ensure!(trusted, "Trust this project before checking its R.");
                    runtime(&search, language)
                }))
            })
            .await?,
        )?)
    }

    pub(super) async fn plan_project_file(
        &self,
        project: String,
        language: String,
    ) -> Result<Value> {
        let (root, trusted) = self.project_package_context(project).await?;
        let own = self.own_folder();
        let plan = blocking(move || {
            let search = Search::from_env().in_project(Some(&root)).in_own(own);
            Ok(project_packages::file_plan(
                &root,
                &search,
                &language,
                trusted,
                || runtime(&search, &language),
                || imports(&root, &language),
            ))
        })
        .await?;
        let json = serde_json::to_value(&plan)?;
        let key = (plan.project.clone(), plan.language.clone());
        let mut plans = self.project_package_plans.lock().unwrap();
        if plan.refused.is_none() {
            if plans.len() >= 32 && !plans.contains_key(&key) {
                plans.clear();
            }
            plans.insert(key, plan);
        } else {
            plans.remove(&key);
        }
        Ok(json)
    }

    pub(super) async fn write_project_file(
        &self,
        project: String,
        language: String,
    ) -> Result<Value> {
        let (root, trusted) = self.project_package_context(project).await?;
        let project = root.to_string_lossy().into_owned();
        let plan = self
            .project_package_plans
            .lock()
            .unwrap()
            .remove(&(project.clone(), language.clone()));
        if !trusted || plan.is_none() {
            return Ok(serde_json::to_value(ProjectFilePlan {
                project,
                language,
                path: String::new(),
                replaces: false,
                packages: Vec::new(),
                contents: String::new(),
                notes: Vec::new(),
                refused: Some(
                    if trusted {
                        "Review this package file before saving it."
                    } else {
                        "Trust this project before saving its packages."
                    }
                    .into(),
                ),
            })?);
        }
        Ok(serde_json::to_value(
            blocking(move || {
                let mut plan = plan.unwrap();
                if let Err(error) = project_packages::write(&plan) {
                    plan.refused = Some(error.to_string());
                }
                Ok(plan)
            })
            .await?,
        )?)
    }

    pub(super) async fn plan_project_setup(&self, project: String, file: String) -> Result<Value> {
        let (root, trusted) = self.project_package_context(project).await?;
        let record = self.install_record();
        let own = self.own_folder();
        let key = (root.to_string_lossy().into_owned(), file.clone());
        let (plan, work) = blocking(move || {
            let search = Search::from_env().in_project(Some(&root)).in_own(own);
            Ok(project_packages::prepare(
                &root,
                &search,
                &record,
                &file,
                trusted,
                |language| runtime(&search, language),
            ))
        })
        .await?;
        let mut plans = self.project_setup_plans.lock().unwrap();
        if let Some(work) = work {
            if plans.len() >= 16 && !plans.contains_key(&key) {
                plans.clear();
            }
            plans.insert(key, work);
        } else {
            plans.remove(&key);
        }
        Ok(serde_json::to_value(plan)?)
    }

    pub(super) async fn run_project_setup(&self, project: String, file: String) -> Result<Value> {
        let (root, trusted) = self.project_package_context(project).await?;
        let record = self.install_record();
        ensure!(
            trusted,
            "Trust this project before setting up its packages."
        );
        let project = root.to_string_lossy().into_owned();
        let work = self
            .project_setup_plans
            .lock()
            .unwrap()
            .remove(&(project.clone(), file))
            .context("Review this project's package setup before starting it.")?;
        let title = work.plan().what.clone();
        let exe = self.options.executable.clone();
        self.quick_job(move |log| {
            let mut launch =
                install::launch(&exe, &record, install::Job::ProjectSetup(work), log, &title)?;
            launch.project = Some(project);
            Ok(launch)
        })
        .await
    }
}

fn runtime(search: &Search, language: &str) -> Result<Runtime> {
    let language = Language::parse(language).context("Choose R or Python.")?;
    let specs = kernelspecs::discover(search);
    if let Some(folder) = &search.own {
        let (name, bin) = match language {
            Language::R => ("r", "R"),
            Language::Python => ("python", "python"),
        };
        let program = crate::own_env::prefix(folder, name).join("bin").join(bin);
        ensure!(
            crate::own_env::executable(&program) && crate::own_env::inside(folder, &program),
            "Set up Sandhill's {bin} on the R and Python page first."
        );
        let spec = specs
            .into_iter()
            .find(|k| {
                Language::parse(&k.dir.kernelspec.language) == Some(language)
                    && k.env
                        .as_deref()
                        .is_some_and(|e| e.strip_prefix("sandhill-") == Some(name))
            })
            .map(|k| k.dir.kernelspec)
            .unwrap_or_else(|| bare_spec(&program, language));
        return Ok(Runtime { program, spec });
    }
    match choose_spec(search, &specs, language, None, None) {
        Ok(k) => {
            let spec = &k.dir.kernelspec;
            let program = match language {
                Language::R => crate::irkernel::spec_r(spec, search),
                Language::Python => kernelspecs::program(spec, search.path.as_deref()),
            }
            .with_context(|| {
                format!(
                    "The program for the project's {} kernel couldn't be found.",
                    language.name()
                )
            })?;
            Ok(Runtime {
                program,
                spec: spec.clone(),
            })
        }
        Err(error) => {
            let settings = search
                .project
                .as_ref()
                .and_then(|p| projects::settings::read(p).ok().flatten());
            if settings.is_some_and(|s| match language {
                Language::R => s.r.is_some(),
                Language::Python => s.python.is_some(),
            }) {
                return Err(error);
            }
            // A project can carry packages before this R or Python has a kernel.
            let program = if language == Language::R {
                crate::irkernel::find(search)
                    .into_iter()
                    .next()
                    .map(|c| c.r)
            } else {
                search
                    .path
                    .as_deref()
                    .into_iter()
                    .flat_map(std::env::split_paths)
                    .map(|p| p.join("python3"))
                    .find(|p| crate::own_env::executable(p))
            }
            .ok_or(error)?;
            Ok(Runtime {
                spec: bare_spec(&program, language),
                program,
            })
        }
    }
}

fn bare_spec(program: &Path, language: Language) -> jupyter_protocol::JupyterKernelspec {
    jupyter_protocol::JupyterKernelspec {
        argv: vec![program.to_string_lossy().into_owned()],
        display_name: language.name().into(),
        language: language.name().into(),
        metadata: None,
        interrupt_mode: None,
        env: None,
    }
}

fn imports(root: &Path, language: &str) -> BTreeSet<String> {
    let Some(language) = Language::parse(language) else {
        return BTreeSet::new();
    };
    let mut pending = vec![root.to_path_buf()];
    let mut names = BTreeSet::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let path = entry.path();
            if kind.is_dir() {
                if !matches!(name.to_str(), Some("renv" | "venv" | "__pycache__"))
                    && !path.join("pyvenv.cfg").is_file()
                    && !path.join("conda-meta").is_dir()
                {
                    pending.push(path);
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let Some(ext) = path
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase)
            else {
                continue;
            };
            if !matches!(ext.as_str(), "r" | "py" | "rmd" | "qmd" | "ipynb")
                || entry.metadata().is_ok_and(|m| m.len() > MAX_SCRIPT)
            {
                continue;
            }
            let mut text = String::new();
            if !fs::File::open(&path)
                .is_ok_and(|f| f.take(MAX_SCRIPT + 1).read_to_string(&mut text).is_ok())
                || text.len() as u64 > MAX_SCRIPT
            {
                continue;
            }
            if matches!(ext.as_str(), "rmd" | "qmd") {
                for chunk in crate::qmd::parse_chunks(&text)
                    .iter()
                    .filter(|c| Language::parse(&c.language) == Some(language))
                {
                    let mut imports = Vec::new();
                    package_scan::scan(language, &chunk.code, &mut imports);
                    names.extend(imports);
                }
            } else if let Ok(doc) = package_scan::document(&path, &text, None)
                && doc.language == language
            {
                names.extend(doc.imports);
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_mixed_documents_but_not_project_environments() {
        let root = crate::quick::test_common::tempdir().unwrap();
        for dir in ["source", "renv", ".sandhill", ".venv"] {
            fs::create_dir(root.path().join(dir)).unwrap();
        }
        fs::write(root.path().join("source/code.R"), "library(realR)\n").unwrap();
        fs::write(root.path().join("source/code.py"), "import realPython\n").unwrap();
        fs::write(root.path().join("renv/code.R"), "library(wrongR)\n").unwrap();
        fs::write(root.path().join(".venv/code.py"), "import wrongPython\n").unwrap();
        fs::write(root.path().join(".sandhill/code.R"), "library(wrongR)\n").unwrap();
        fs::write(
            root.path().join("source/report.qmd"),
            "```{r}\nonlyQmd::read()\n```\n\n```{python}\nimport qmdPython\n```\n",
        )
        .unwrap();
        assert_eq!(
            imports(root.path(), "r"),
            BTreeSet::from(["onlyQmd".into(), "realR".into()])
        );
        assert_eq!(
            imports(root.path(), "python"),
            BTreeSet::from(["qmdPython".into(), "realPython".into()])
        );
    }
}
