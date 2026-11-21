//! Package updates without an open session.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sandhill_proto::{InstallTool, PackageUpdate};
use serde_json::Value;

use super::{
    environments::{self, Environment},
    install, package_plan, record,
};
use crate::kernelspecs::Search;

const TIMEOUT: Duration = Duration::from_secs(160);

pub(crate) fn updates(
    search: &Search,
    record_path: &Path,
    env: &str,
) -> Result<Vec<PackageUpdate>> {
    let env = environments::get(search, record_path, env)?;
    let mut output = std::thread::scope(|scope| {
        let conda = env
            .tool(InstallTool::Conda)
            .map(|t| scope.spawn(|| conda_updates(&env, Path::new(&t.path))));
        let language = if env.is_r() {
            r_updates(&env)
        } else {
            python_updates(&env)
        };
        match (language, conda.map(|job| job.join().unwrap())) {
            (Ok(mut native), Some(Ok(mut conda))) => {
                native.append(&mut conda);
                Ok(native)
            }
            (Ok(native), _) if !native.is_empty() => Ok(native),
            (Ok(_), Some(Err(error))) => Err(error),
            (Ok(native), None) => Ok(native),
            (_, Some(Ok(conda))) => Ok(conda),
            (Err(error), _) => Err(error),
        }
    })?;
    output.sort_by_key(|update| {
        (
            update.name.to_ascii_lowercase(),
            update.library.clone(),
            update.source.clone(),
        )
    });
    Ok(output)
}

fn r_updates(env: &Environment) -> Result<Vec<PackageUpdate>> {
    let mut libraries = env.card.libraries.iter().filter(|l| l.writable).peekable();
    if libraries.peek().is_none() {
        return Ok(Vec::new());
    }
    let libs = libraries
        .map(|l| package_plan::r_string(&l.path))
        .collect::<Vec<_>>()
        .join(",");
    let code = format!(
        r#"local({{
      options(timeout = 35)
      repos <- getOption('repos')
      if (is.null(repos['CRAN']) || is.na(repos['CRAN']) || repos['CRAN'] == '@CRAN@') repos['CRAN'] <- 'https://cloud.r-project.org'
      if (requireNamespace('BiocManager', quietly = TRUE)) {{
        bioc <- tryCatch(suppressWarnings(suppressMessages(BiocManager::repositories())), error = function(e) NULL)
        repos <- c(repos, bioc[setdiff(names(bioc), names(repos))])
      }} else {{
        root <- 'https://bioconductor.org/packages/release/'
        repos <- c(repos, BioCsoft = paste0(root, 'bioc'), BioCann = paste0(root, 'data/annotation'), BioCexp = paste0(root, 'data/experiment'))
      }}
      available <- suppressWarnings(utils::available.packages(repos = repos))
      if (!nrow(available)) stop('No package list could be reached', call. = FALSE)
      for (lib in c({libs})) {{
        old <- suppressWarnings(utils::old.packages(lib.loc = lib, repos = repos, available = available))
        if (!is.null(old) && nrow(old)) {{
          bioc <- grepl('bioconductor', old[, 'Repository'], ignore.case = TRUE) | old[, 'Repository'] %in% utils::contrib.url(repos[startsWith(names(repos), 'BioC')])
          writeLines(paste('SANDHILL_UPDATE', old[, 'Package'], old[, 'Installed'], old[, 'ReposVer'], old[, 'LibPath'], as.integer(bioc), sep = '\t'))
        }}
      }}
    }})"#
    );
    let mut updates = crate::packages::parse_updates(
        &install::probe_timeout(
            Path::new(&env.card.program),
            &["--no-save", "--no-restore", "-s", "-e", &code],
            env.own.as_deref(),
            TIMEOUT,
        )
        .context("R couldn't check for updates. Check your internet connection and try again.")?,
    )
    .updates;
    if let Some(prefix) = &env.conda {
        let managed = record::conda_packages(prefix).unwrap_or_default();
        let conda_library = prefix.join("lib/R/library");
        updates.retain(|u| {
            let name = u.name.to_ascii_lowercase();
            !package_plan::same_folder(Path::new(&u.library), &conda_library)
                || !(managed.contains_key(&format!("r-{name}"))
                    || managed.contains_key(&format!("bioconductor-{name}")))
        });
    }
    Ok(updates)
}

fn conda_updates(env: &Environment, tool: &Path) -> Result<Vec<PackageUpdate>> {
    let prefix = env
        .conda
        .as_deref()
        .context("The conda folder couldn't be found.")?;
    let out = install::probe_conda_search_timeout(
        tool,
        &[
            "update",
            "--all",
            "--dry-run",
            "--json",
            "--prefix",
            &prefix.to_string_lossy(),
        ],
        TIMEOUT,
    )
    .context(
        "Couldn't check for updates from conda. Check your internet connection and try again.",
    )?;
    let result: Value =
        serde_json::from_str(&out).context("Couldn't read the updates from conda.")?;
    if result["success"].as_bool() == Some(false) || result.get("error").is_some() {
        bail!("Conda couldn't check for updates.");
    }
    let actions = result["actions"]
        .as_array()
        .and_then(|a| a.first())
        .unwrap_or(&result["actions"]);
    let unlinked: std::collections::HashMap<_, _> = actions["UNLINK"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| Some((p["name"].as_str()?, p["version"].as_str()?)))
        .collect();
    let managed = if env.is_r() {
        Default::default()
    } else {
        package_plan::conda_distribution_names(prefix)
    };
    let r_packages = if env.is_r() {
        environments::r_packages(&env.card.libraries)
    } else {
        Vec::new()
    };
    Ok(actions["LINK"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let name = p["name"].as_str()?;
            let available = p["version"].as_str()?;
            let installed = *unlinked.get(name)?;
            // Conda can propose downgrades to satisfy a different package. They aren't updates.
            if version_parts(available) < version_parts(installed) || available == installed {
                return None;
            }
            let channel = ["channel", "base_url", "url"]
                .iter()
                .find_map(|key| p[key].as_str())
                .unwrap_or("conda");
            let source = if channel.contains("conda-forge") {
                "conda-forge"
            } else if channel.contains("bioconda") {
                "bioconda"
            } else if channel
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
            {
                channel
            } else {
                "conda"
            };
            let (name, library, bioconductor) = if env.is_r() {
                let name = name
                    .strip_prefix("r-")
                    .or_else(|| name.strip_prefix("bioconductor-"))?;
                let package = r_packages.iter().find(|p| {
                    p.name.eq_ignore_ascii_case(name) && Path::new(&p.library).starts_with(prefix)
                })?;
                (
                    package.name.clone(),
                    package.library.clone(),
                    source == "bioconda",
                )
            } else {
                let name = managed
                    .iter()
                    .find_map(|(distribution, original)| {
                        (original == name).then_some(distribution.as_str())
                    })
                    .unwrap_or(name);
                (name.into(), env.card.folder.clone(), false)
            };
            Some(PackageUpdate {
                name,
                installed: installed.into(),
                available: available.into(),
                library,
                bioconductor,
                source: source.into(),
            })
        })
        .collect())
}

fn version_parts(text: &str) -> Vec<u64> {
    text.split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect()
}

fn python_updates(env: &Environment) -> Result<Vec<PackageUpdate>> {
    let program = &env.card.program;
    if env.packages.is_empty() {
        return Ok(Vec::new());
    }
    let out = env
        .tool(InstallTool::Uv)
        .and_then(|uv| {
            install::probe_timeout(
                Path::new(&uv.path),
                &[
                    "--no-cache",
                    "pip",
                    "list",
                    "--outdated",
                    "--format=json",
                    "--python",
                    program,
                ],
                env.own.as_deref(),
                TIMEOUT,
            )
        })
        .or_else(|| {
            install::probe_timeout(
                Path::new(program),
                &[
                    "-I",
                    "-m",
                    "pip",
                    "--no-cache-dir",
                    "--disable-pip-version-check",
                    "list",
                    "--outdated",
                    "--format=json",
                ],
                env.own.as_deref(),
                TIMEOUT,
            )
        });
    let managed = env
        .conda
        .as_deref()
        .map(package_plan::conda_distribution_names)
        .unwrap_or_default();
    if out.is_none() && env.conda.is_some() && env.packages.keys().all(|p| managed.contains_key(p))
    {
        return Ok(Vec::new());
    }
    let out = out.context(
        "Python couldn't check for updates. Check your internet connection and try again.",
    )?;
    let list: Vec<Value> =
        serde_json::from_str(&out).context("Couldn't read the updates from Python.")?;
    Ok(list
        .into_iter()
        .filter_map(|p| {
            let name = p["name"].as_str()?;
            let normalized = record::python_name(name);
            if !env.packages.contains_key(&normalized)
                || managed.contains_key(&normalized)
                || p.get("editable_project_location")
                    .is_some_and(|v| !v.is_null())
            {
                return None;
            }
            Some(PackageUpdate {
                name: name.into(),
                installed: p["version"].as_str()?.into(),
                available: p["latest_version"].as_str()?.into(),
                library: env.card.folder.clone(),
                bioconductor: false,
                source: "PyPI".into(),
            })
        })
        .collect())
}
