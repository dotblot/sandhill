use super::package_scan::{Language, document, python_module, r_package};
use super::*;
use crate::kernelspecs::{self, Found, Search};
use crate::quick::{install, package_names, package_target};
use sandhill_proto::{InstallPrefs, KernelspecInfo, MissingPackage, MissingPackages, SessionInfo};

impl Daemon {
    pub(super) async fn missing_packages(
        &self,
        path: String,
        kernel: Option<String>,
        session: Option<String>,
        project: Option<String>,
    ) -> Result<MissingPackages> {
        let path = files::resolve(&path)?;
        let project = project.as_deref().map(projects::normalize).transpose()?;
        let project = project.map(PathBuf::from);
        let info = session
            .as_deref()
            .and_then(|id| self.session_info(id).ok())
            .filter(|info| kernel.as_ref().is_none_or(|kernel| kernel == &info.kernel))
            .filter(|info| {
                project
                    .as_ref()
                    .is_none_or(|p| info.project.as_deref().map(Path::new) == Some(p))
            })
            .filter(|info| self.live(&info.id).is_ok());
        let journal = self.journal.clone();
        let record = self.install_record();
        let (checked, search, prefs) = blocking(move || {
            let restricted_root = projects::trust::restricted_root(&journal, &path)?;
            let restricted = restricted_root.is_some()
                || match &project {
                    Some(project) => {
                        !projects::trust::is_trusted(&journal, &project.to_string_lossy())?
                    }
                    None => false,
                };
            let project = project.or_else(|| restricted_root.map(PathBuf::from));
            let own = crate::own_env::chosen(&record);
            let prefs = install::prefs(&record);
            let text = std::fs::read_to_string(&path)?;
            let search = Search::from_env()
                .in_project(project.as_deref())
                .in_own(own);
            if !restricted
                && let Some(info) = info
                && let Some(language) = Language::parse(&info.language)
            {
                let doc = document(&path, &text, Some(language))?;
                if language == doc.language {
                    let check = DocumentCheck::Live {
                        info,
                        language,
                        names: doc.imports,
                    };
                    return Ok((check, search, prefs));
                }
            }
            let specs = kernelspecs::discover(&search);
            let chosen_language = kernel.as_deref().and_then(|kernel| {
                specs
                    .iter()
                    .find(|k| k.dir.kernel_name == kernel)
                    .and_then(spec_language)
            });
            let mut doc = document(&path, &text, chosen_language)?;
            // A custom Quarto kernel name doesn't encode its language.
            if kernel.is_none()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("qmd"))
                && let Some(lang) = doc.kernel.as_deref().and_then(|name| {
                    specs
                        .iter()
                        .find(|k| k.dir.kernel_name == name)
                        .and_then(spec_language)
                })
                && lang != doc.language
            {
                doc = document(&path, &text, Some(lang))?;
            }
            let spec = choose_spec(
                &search,
                &specs,
                doc.language,
                kernel.as_deref(),
                doc.kernel.as_deref(),
            )?;
            let result = check_files(
                spec,
                &search,
                &prefs,
                doc.language,
                doc.imports,
                path.parent(),
                restricted,
            )?;
            Ok((DocumentCheck::Files(result), search, prefs))
        })
        .await?;
        match checked {
            DocumentCheck::Live {
                info,
                language,
                names,
            } => {
                self.missing_in_session(info, language, names, search, prefs)
                    .await
            }
            DocumentCheck::Files(result) => Ok(result),
        }
    }

    pub(super) async fn missing_in_error(
        &self,
        session: &str,
        error: &str,
    ) -> Result<MissingPackages> {
        let info = self.session_info(session)?;
        let language =
            Language::parse(&info.language).context("This session doesn't use R or Python.")?;
        let names = error_import(language, error)
            .map(str::to_owned)
            .into_iter()
            .collect();
        let project = info.project.as_deref().map(Path::new);
        self.trusted(
            std::iter::once(PathBuf::from(&info.cwd))
                .chain(project.map(Path::to_path_buf))
                .collect(),
            "check this error",
        )
        .await?;
        let (search, prefs) = {
            let record = self.install_record();
            let project = project.map(Path::to_path_buf);
            blocking(move || {
                let own = crate::own_env::chosen(&record);
                Ok((
                    Search::from_env()
                        .in_project(project.as_deref())
                        .in_own(own),
                    install::prefs(&record),
                ))
            })
            .await?
        };
        if self.live(session).is_ok() {
            self.missing_in_session(info, language, names, search, prefs)
                .await
        } else {
            blocking(move || {
                let specs = kernelspecs::discover(&search);
                let spec = choose_spec(&search, &specs, language, Some(&info.kernel), None)?;
                check_files(
                    spec,
                    &search,
                    &prefs,
                    language,
                    names,
                    Some(Path::new(&info.cwd)),
                    false,
                )
            })
            .await
        }
    }

    async fn missing_in_session(
        &self,
        info: SessionInfo,
        language: Language,
        names: Vec<String>,
        search: Search,
        prefs: InstallPrefs,
    ) -> Result<MissingPackages> {
        if let Some(folder) = &search.own {
            let connection = self.state.kernels_dir().join(format!("{}.json", info.id));
            let inside = crate::lifecycle::recorded_kernel(&connection)
                .and_then(|kernel| kernel.program())
                .is_some_and(|program| crate::own_env::inside(folder, &program));
            anyhow::ensure!(inside, "{}", crate::own_env::SWITCH_HINT);
        }
        // A file check can't see the session's current paths. Don't guess if it's busy.
        let printed = self
            .quietly(&info.id, "check for missing packages", |_| {
                Some(package_target::session_code(language.name(), &names))
            })
            .await?;
        blocking(move || {
            let checked = package_target::session_result(
                language.name(),
                &printed.stdout,
                &prefs,
                search.own.as_deref(),
            )?;
            check_own(&checked.target, &search)?;
            let specs = kernelspecs::discover(&search);
            let label = specs
                .iter()
                .find(|k| k.dir.kernel_name == info.kernel)
                .map_or_else(
                    || {
                        sandhill_proto::kernel_label(
                            language.name(),
                            info.language_version.as_deref(),
                            None,
                            None,
                        )
                    },
                    |found| kernel_label(found, info.language_version.as_deref()),
                );
            Ok(missing_reply(language, label, checked, &search))
        })
        .await
    }
}

enum DocumentCheck {
    Live {
        info: SessionInfo,
        language: Language,
        names: Vec<String>,
    },
    Files(MissingPackages),
}

fn spec_language(spec: &Found) -> Option<Language> {
    Language::parse(&spec.dir.kernelspec.language)
}

/// Match project defaults and the daemon's usual language choices without starting a kernel.
pub(super) fn choose_spec<'a>(
    search: &Search,
    specs: &'a [Found],
    language: Language,
    kernel: Option<&str>,
    metadata_kernel: Option<&str>,
) -> Result<&'a Found> {
    let available = |k: &&Found| !k.duplicate && kernelspecs::runnable(&k.dir.kernelspec, search);
    let mut choices: Vec<_> = specs.iter().filter(available).collect();
    choices.sort_unstable_by_key(|k| &k.dir.kernel_name);
    let named = |name: &str| {
        specs.iter().find(|k| {
            k.dir.kernel_name == name
                && spec_language(k) == Some(language)
                && kernelspecs::runnable(&k.dir.kernelspec, search)
        })
    };
    if let Some(kernel) = kernel {
        return named(kernel)
            .with_context(|| format!("The kernel {kernel:?} wasn't found for this file."));
    }
    if let Some(root) = &search.project {
        let settings = projects::settings::read(root)
            .ok()
            .flatten()
            .unwrap_or_default();
        let saved = match language {
            Language::R => settings.r,
            Language::Python => settings.python,
        };
        if let Some(kernel) = saved {
            return named(&kernel)
                .with_context(|| format!("The project's kernel {kernel:?} wasn't found."));
        }
        if search.own.is_some() {
            return choices
                .into_iter()
                .find(|k| spec_language(k) == Some(language))
                .context("No kernel for this file was found in Sandhill's folder.");
        }
        let kernels: Vec<_> = choices
            .iter()
            .map(|k| KernelspecInfo {
                name: k.dir.kernel_name.clone(),
                language: k.dir.kernelspec.language.clone(),
                env: k.env.clone(),
                display_name: String::new(),
                version: None,
                engine: None,
            })
            .collect();
        if let Some(found) = projects::environments::find(root, &kernels, search)
            .into_iter()
            .filter(|found| found.language == language.name())
            .find_map(|found| found.kernel.and_then(|k| named(&k)))
        {
            return Ok(found);
        }
    }
    metadata_kernel
        .and_then(named)
        .or_else(|| {
            language
                .preferred_kernels()
                .iter()
                .find_map(|name| named(name))
        })
        .or_else(|| {
            choices
                .into_iter()
                .find(|k| spec_language(k) == Some(language))
        })
        .with_context(|| format!("No {} kernel was found for this file.", language.name()))
}

fn check_files(
    found: &Found,
    search: &Search,
    prefs: &InstallPrefs,
    language: Language,
    names: Vec<String>,
    document_folder: Option<&Path>,
    restricted: bool,
) -> Result<MissingPackages> {
    let spec = &found.dir.kernelspec;
    let project = search.project.as_deref();
    let own = search.own.as_deref();
    let checked = match language {
        Language::R => {
            let program = if let Some(folder) = own
                && crate::kernel_versions::engine(spec) == Some("Ark")
            {
                Some(crate::own_env::prefix(folder, "r").join("bin/R"))
            } else {
                crate::irkernel::spec_r(spec, search)
            }
            .context("The R for this kernel wasn't found.")?;
            package_target::r_files(&program, spec, names, project, prefs, own, restricted)?
        }
        Language::Python => {
            let program = kernelspecs::program(spec, search.path.as_deref())
                .context("The Python for this kernel wasn't found.")?;
            package_target::python_files(
                &program,
                spec,
                names,
                document_folder,
                project,
                own,
                restricted,
            )?
        }
    };
    check_own(&checked.target, search)?;
    let version = (!restricted)
        .then(|| {
            crate::kernel_versions::versions(&[spec], search.path.as_deref())
                .pop()
                .flatten()
        })
        .flatten();
    Ok(missing_reply(
        language,
        kernel_label(found, version.as_deref()),
        checked,
        search,
    ))
}

fn check_own(target: &sandhill_proto::InstallTarget, search: &Search) -> Result<()> {
    use sandhill_proto::InstallTarget::*;
    if let Some(folder) = &search.own {
        let inside = |p: &str| crate::own_env::inside(folder, Path::new(p));
        let allowed = match target {
            R { r, library } => inside(r) && (library.is_empty() || inside(library)),
            Python { prefix, python } => inside(python) && inside(prefix),
        };
        anyhow::ensure!(allowed, "{}", crate::own_env::SWITCH_HINT);
    }
    Ok(())
}

fn kernel_label(found: &Found, version: Option<&str>) -> String {
    let spec = &found.dir.kernelspec;
    sandhill_proto::kernel_label(
        &spec.language,
        version,
        found.env.as_deref(),
        crate::kernel_versions::engine(spec),
    )
}

fn missing_reply(
    language: Language,
    kernel: String,
    checked: package_target::Checked,
    search: &Search,
) -> MissingPackages {
    let missing = checked
        .absent
        .into_iter()
        .map(|import| {
            let package = package_names::import_name(&import, &checked.target, search).to_owned();
            MissingPackage { import, package }
        })
        .collect();
    MissingPackages {
        language: language.name().into(),
        target: checked.target,
        kernel,
        missing,
    }
}

fn error_import(language: Language, error: &str) -> Option<&str> {
    error.lines().find_map(|line| match language {
        Language::R => {
            if let Some((_, tail)) = line.split_once("there is no package called ") {
                return quoted_name(tail)
                    .map(|(name, _)| name)
                    .filter(|name| r_package(name));
            }
            let (_, tail) = line.split_once("package ")?;
            let (name, tail) = quoted_name(tail)?;
            (tail.trim_start().starts_with("required by") && r_package(name)).then_some(name)
        }
        Language::Python => {
            let tail = line
                .split_once("ModuleNotFoundError:")
                .or_else(|| line.split_once("ImportError:"))?
                .1
                .trim_start()
                .strip_prefix("No module named ")?;
            let name = quoted_name(tail)
                .map(|(name, _)| name)
                .or_else(|| tail.split_whitespace().next())?;
            let top = name.split('.').next()?;
            python_module(top).then_some(top)
        }
    })
}

fn quoted_name(text: &str) -> Option<(&str, &str)> {
    let quote = match text.chars().next()? {
        '‘' => '’',
        '“' => '”',
        c @ ('\'' | '"' | '`') => c,
        _ => return None,
    };
    text[text.chars().next()?.len_utf8()..].split_once(quote)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_r_errors_and_python_top_level_imports() {
        for (lang, error, expected) in [
            (
                Language::R,
                "Error in library(x): there is no package called ‘x’",
                Some("x"),
            ),
            (
                Language::R,
                "there is no package called 'harmony'",
                Some("harmony"),
            ),
            (
                Language::R,
                "error: package ‘SeuratObject’ required by ‘Seurat’ could not be found",
                Some("SeuratObject"),
            ),
            (Language::R, "package ‘dplyr’ was built under R 4.6.1", None),
            (
                Language::Python,
                "Traceback…\nModuleNotFoundError: No module named 'sklearn.cluster'",
                Some("sklearn"),
            ),
            (
                Language::Python,
                "ImportError: No module named cv2",
                Some("cv2"),
            ),
            (
                Language::Python,
                "ImportError: cannot import name 'x' from 'pandas'",
                None,
            ),
            (Language::Python, "ValueError: No module named 'fake'", None),
        ] {
            assert_eq!(error_import(lang, error), expected, "{error}");
        }
    }
}
