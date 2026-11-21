//! Read installed package metadata without loading any package or project code.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use jupyter_protocol::JupyterKernelspec;
use sandhill_proto::{InstallTool, RLibrary};
use serde::Deserialize;

use super::{environments, install, package_names, package_plan, package_target, record};

pub(super) fn version(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".+!_-".contains(&c))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Source {
    Cran,
    Bioc,
    Github(Github),
    Conda(String),
    Pypi,
    Local,
    Unknown,
}

impl Source {
    pub fn label(&self) -> &str {
        match self {
            Self::Cran => "CRAN",
            Self::Bioc => "Bioconductor",
            Self::Github(_) => "GitHub",
            Self::Conda(channel) => channel,
            Self::Pypi => "PyPI",
            Self::Local | Self::Unknown => "this computer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Github {
    pub owner: String,
    pub repo: String,
    pub sha: String,
    pub subdir: Option<String>,
}

impl Github {
    fn new(owner: &str, repo: &str, sha: &str, subdir: Option<&str>) -> Option<Self> {
        let name = format!("{owner}/{repo}");
        (package_plan::github_repo(&name)
            && (7..=64).contains(&sha.len())
            && sha.bytes().all(|c| c.is_ascii_hexdigit())
            && subdir.is_none_or(|subdir| {
                subdir.split('/').all(|part| {
                    !part.is_empty()
                        && part != "."
                        && part != ".."
                        && part
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
                })
            }))
        .then(|| Self {
            owner: owner.into(),
            repo: repo.into(),
            sha: sha.into(),
            subdir: subdir.map(str::to_owned),
        })
    }

    pub fn spec(&self) -> String {
        format!(
            "{}/{}{}@{}",
            self.owner,
            self.repo,
            self.subdir
                .as_ref()
                .map_or_else(String::new, |s| format!("/{s}")),
            self.sha
        )
    }
}

#[derive(Debug)]
pub(super) struct RPackage {
    pub version: String,
    pub source: Source,
    pub dependencies: Vec<String>,
    pub base: bool,
    pub recommended: bool,
}

impl RPackage {
    fn parse(pkg: &environments::RPackage) -> Self {
        let field = |name| record::r_field(&pkg.description, name);
        let source = if field("RemoteType").is_some_and(|t| t.eq_ignore_ascii_case("github")) {
            let github = (|| {
                if field("RemoteHost")
                    .is_some_and(|h| !matches!(h, "api.github.com" | "github.com"))
                {
                    return None;
                }
                Github::new(
                    field("RemoteUsername")?,
                    field("RemoteRepo")?,
                    field("RemoteSha")?,
                    field("RemoteSubdir").filter(|s| !s.is_empty()),
                )
            })();
            github.map_or(Source::Unknown, Source::Github)
        } else if field("RemoteType").is_some() {
            Source::Local
        } else if field("biocViews").is_some()
            || field("git_url").is_some_and(|url| url.contains("bioconductor.org"))
            || field("Repository").is_some_and(|r| r.to_ascii_lowercase().contains("bioconductor"))
        {
            Source::Bioc
        } else if field("Repository").is_some_and(|r| r.eq_ignore_ascii_case("cran"))
            || field("Priority") == Some("recommended")
        {
            Source::Cran
        } else {
            Source::Unknown
        };
        let dependencies = ["Depends", "Imports", "LinkingTo"]
            .into_iter()
            .flat_map(|field| dcf_field(&pkg.description, field))
            .flat_map(|field| {
                field
                    .split(',')
                    .filter_map(|spec| {
                        let name = spec.trim().split([' ', '\t', '(']).next()?;
                        (name != "R" && record::r_name(name)).then(|| name.to_owned())
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        Self {
            version: pkg.version.clone(),
            source,
            dependencies,
            base: field("Priority") == Some("base")
                || package_target::R_BASE.contains(&pkg.name.as_str()),
            recommended: field("Priority") == Some("recommended"),
        }
    }
}

fn dcf_field(text: &str, wanted: &str) -> Option<String> {
    let mut lines = text.lines();
    let initial = lines.find_map(|line| {
        let (key, value) = line.split_once(':')?;
        (key == wanted).then(|| value.trim())
    })?;
    let mut out = initial.to_owned();
    for line in lines.take_while(|l| l.starts_with([' ', '\t'])) {
        out.push(' ');
        out.push_str(line.trim());
    }
    Some(out)
}

pub(super) fn r_packages(libraries: &[RLibrary]) -> BTreeMap<String, RPackage> {
    let mut result = BTreeMap::new();
    let mut conda = HashMap::new();
    for pkg in environments::r_packages(libraries)
        .iter()
        .filter(|pkg| version(&pkg.version))
    {
        if !result.contains_key(&pkg.name) {
            let mut info = RPackage::parse(pkg);
            if info.source == Source::Unknown
                && let Some(prefix) = Path::new(&pkg.library)
                    .ancestors()
                    .find(|path| path.join("conda-meta").is_dir())
            {
                let records = conda
                    .entry(prefix)
                    .or_insert_with(|| record::conda_packages(prefix).unwrap_or_default());
                for (bio, source) in [(true, Source::Bioc), (false, Source::Cran)] {
                    if records.get(&super::own::r_package(&pkg.name, bio)) == Some(&pkg.version) {
                        info.source = source;
                        break;
                    }
                }
            }
            result.insert(pkg.name.clone(), info);
        }
    }
    result
}

pub(super) struct RRuntime {
    pub version: String,
    pub libraries: Vec<RLibrary>,
    pub platform: String,
}

pub(super) fn r_runtime(
    program: &Path,
    spec: Option<&JupyterKernelspec>,
    project: Option<&Path>,
    own: Option<&Path>,
) -> Result<RRuntime> {
    let mut command = Command::new(program);
    // Keep .Renviron (which can select libraries), but never run R profiles.
    command.current_dir(project.unwrap_or(Path::new("/")));
    if let Some(spec) = spec.and_then(|s| s.env.as_ref()) {
        command.envs(spec.iter().filter(|(key, _)| {
            matches!(
                key.as_str(),
                "R_LIBS" | "R_LIBS_USER" | "R_LIBS_SITE" | "R_ENVIRON" | "R_ENVIRON_USER"
            )
        }));
    }
    if let Some(own) = own {
        crate::own_env::apply(&mut command, own);
    }
    let code = "cat(as.character(getRversion()), '\\t', R.version$platform, '\\n', sep=''); ";
    let code = format!("{code}{}", crate::irkernel::LIBRARIES_CODE);
    let output = install::probe_command_with(
        command,
        &[
            "--no-site-file",
            "--no-init-file",
            "--no-save",
            "--no-restore",
            "-s",
            "-e",
            &code,
        ],
        Duration::from_secs(8),
        false,
    )
    .context("R couldn't tell Sandhill where its packages are.")?;
    let (version, platform) = output
        .lines()
        .find_map(|s| {
            s.split_once('\t')
                .filter(|(v, _)| v.starts_with(|c: char| c.is_ascii_digit()))
        })
        .context("R's version couldn't be read.")?;
    Ok(RRuntime {
        version: version.into(),
        platform: platform.into(),
        libraries: crate::irkernel::parse_libraries(&output),
    })
}

#[derive(Debug, Deserialize)]
struct CondaPackage {
    name: String,
    version: String,
    #[serde(default)]
    channel: String,
    #[serde(default)]
    files: Vec<String>,
}

#[derive(Debug)]
pub(super) struct PythonPackage {
    pub name: String,
    pub version: String,
    pub source: Source,
    pub conda: Option<(String, String)>,
    pub imports: HashSet<String>,
}

#[derive(Deserialize)]
struct DirectUrl {
    url: String,
    vcs_info: Option<VcsInfo>,
    subdirectory: Option<String>,
}

#[derive(Deserialize)]
struct VcsInfo {
    vcs: String,
    commit_id: String,
}

fn direct_source(text: &str) -> Source {
    let Ok(data) = serde_json::from_str::<DirectUrl>(text) else {
        return Source::Unknown;
    };
    if data.url.starts_with("file:") {
        return Source::Local;
    }
    let github = (|| {
        let vcs = data.vcs_info.as_ref().filter(|v| v.vcs == "git")?;
        let path = data
            .url
            .strip_prefix("https://github.com/")
            .or_else(|| data.url.strip_prefix("ssh://git@github.com/"))?;
        let (owner, repo) = path
            .trim_end_matches('/')
            .trim_end_matches(".git")
            .split_once('/')?;
        Github::new(owner, repo, &vcs.commit_id, data.subdirectory.as_deref())
    })();
    github.map_or(Source::Unknown, Source::Github)
}

pub(super) fn channel(text: &str) -> &str {
    if text.split('/').any(|part| part == "bioconda") {
        "bioconda"
    } else if text.split('/').any(|part| part == "conda-forge") {
        "conda-forge"
    } else {
        "conda"
    }
}

pub(super) fn python_packages(
    prefix: &Path,
    sites: impl IntoIterator<Item = PathBuf>,
    for_imports: bool,
) -> BTreeMap<String, PythonPackage> {
    let conda: Vec<_> = fs::read_dir(prefix.join("conda-meta"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|e| e == "json"))
        .filter_map(|entry| {
            serde_json::from_slice::<CondaPackage>(&fs::read(entry.path()).ok()?).ok()
        })
        .collect();
    let mut owners = HashMap::new();
    let mut names = HashMap::new();
    for package in &conda {
        names.insert(record::python_name(&package.name), package);
        for file in &package.files {
            if let Some((dist, _)) = file.split_once(".dist-info/") {
                owners.insert(dist, package);
            }
        }
    }
    let mut packages = BTreeMap::new();
    for site in sites {
        for dist in fs::read_dir(site)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
        {
            let metadata = match dist.extension().and_then(|e| e.to_str()) {
                Some("dist-info") => dist.join("METADATA"),
                Some("egg-info") if dist.is_file() => dist.clone(),
                Some("egg-info") => dist.join("PKG-INFO"),
                _ => continue,
            };
            let Ok(text) = fs::read_to_string(metadata) else {
                continue;
            };
            let (Some(name), Some(ver)) = (
                record::r_field(&text, "Name"),
                record::r_field(&text, "Version"),
            ) else {
                continue;
            };
            let key = record::python_name(name);
            if packages.contains_key(&key)
                || !version(ver)
                || package_plan::python_spec(name) != Some(name)
            {
                continue;
            }
            let installer = fs::read_to_string(dist.join("INSTALLER")).unwrap_or_default();
            let direct = fs::read_to_string(dist.join("direct_url.json")).ok();
            let owner = (direct.is_none() && !matches!(installer.trim(), "pip" | "uv"))
                .then(|| {
                    dist.strip_prefix(prefix)
                        .ok()
                        .and_then(|path| {
                            let stem = path.to_str()?.strip_suffix(".dist-info")?;
                            owners.get(stem).copied()
                        })
                        .or_else(|| {
                            dist.starts_with(prefix)
                                .then(|| {
                                    names
                                        .get(&record::python_name(package_names::distribution(
                                            name,
                                            InstallTool::Conda,
                                        )))
                                        .copied()
                                })
                                .flatten()
                        })
                        .filter(|owner| owner.version == ver || installer.trim() == "conda")
                })
                .flatten();
            let (source, conda) = if let Some(owner) = owner {
                (
                    Source::Conda(channel(&owner.channel).into()),
                    Some((owner.name.clone(), owner.version.clone())),
                )
            } else if let Some(direct) = direct {
                (direct_source(&direct), None)
            } else if matches!(installer.trim(), "pip" | "uv" | "") {
                (Source::Pypi, None)
            } else {
                (Source::Unknown, None)
            };
            let mut imports = HashSet::new();
            if for_imports {
                for module in fs::read_to_string(dist.join("top_level.txt"))
                    .iter()
                    .flat_map(|text| text.lines())
                {
                    imports.insert(module.trim().into());
                }
                for module in fs::read_to_string(dist.join("RECORD"))
                    .iter()
                    .flat_map(|text| text.lines())
                    .filter_map(package_target::record_module)
                {
                    if module
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    {
                        imports.insert(module.into());
                    }
                }
            }
            packages.insert(
                key,
                PythonPackage {
                    name: name.into(),
                    version: ver.into(),
                    source,
                    conda,
                    imports,
                },
            );
        }
    }
    packages
}

pub(super) fn prefix_sites(prefix: &Path) -> Vec<PathBuf> {
    let mut sites: Vec<_> = ["lib", "lib64", "local/lib"]
        .into_iter()
        .flat_map(|lib| package_target::subdirs(&prefix.join(lib)))
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("python"))
        })
        .flat_map(|p| [p.join("site-packages"), p.join("dist-packages")])
        .filter(|p| p.is_dir())
        .collect();
    sites.sort_unstable();
    sites.dedup();
    sites
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quick::test_common::tempdir;

    #[test]
    fn reads_description_origins_and_dependencies() {
        let dir = tempdir().unwrap();
        let cases = [
            (
                "Cran",
                "Repository: CRAN\nDepends: methods,\n    R (>= 4.2),\n    first (>= 1)\nImports: second\nLinkingTo: third\n",
                "CRAN",
            ),
            ("Bioc", "biocViews: GeneExpression\n", "Bioconductor"),
            (
                "Remote",
                "RemoteType: github\nRemoteUsername: someone\nRemoteRepo: example\nRemoteSha: f0a42ffaab\n",
                "GitHub",
            ),
            ("Local", "RemoteType: local\n", "this computer"),
            ("stats", "Priority: base\n", "this computer"),
            ("MASS", "Priority: recommended\n", "CRAN"),
        ];
        for (name, metadata, _) in cases {
            let dir = dir.path().join(name);
            fs::create_dir(&dir).unwrap();
            fs::write(
                dir.join("DESCRIPTION"),
                format!("Package: {name}\nVersion: 1.0\n{metadata}"),
            )
            .unwrap();
        }
        let packages = r_packages(&[RLibrary {
            path: dir.path().display().to_string(),
            writable: false,
            packages: 0,
        }]);
        for (name, _, origin) in cases {
            assert_eq!(packages[name].source.label(), origin);
        }
        assert_eq!(
            packages["Cran"].dependencies,
            ["methods", "first", "second", "third"]
        );
        assert!(packages["stats"].base);
        assert!(packages["MASS"].recommended);
        assert!(
            matches!(&packages["Remote"].source, Source::Github(g) if g.spec() == "someone/example@f0a42ffaab")
        );
    }

    #[test]
    fn reads_python_direct_urls_and_conda_ownership() {
        let dir = tempdir().unwrap();
        let prefix = dir.path();
        let site = prefix.join("lib/python3.12/site-packages");
        let records = [
            ("igraph", "igraph", "", None),
            ("Example_Pkg", "Example-Pkg", "pip", None),
            (
                "Local",
                "Local",
                "pip",
                Some(r#"{"url":"file:///not-for-copy","dir_info":{"editable":true}}"#),
            ),
            (
                "Git",
                "Git",
                "uv",
                Some(
                    r#"{"url":"https://github.com/person/repo.git","vcs_info":{"vcs":"git","commit_id":"0123456789ab"}}"#,
                ),
            ),
        ];
        for (dir, name, installer, direct) in records {
            let dist = site.join(format!("{dir}-1.0.dist-info"));
            fs::create_dir_all(&dist).unwrap();
            fs::write(
                dist.join("METADATA"),
                format!("Name: {name}\nVersion: 1.0\n"),
            )
            .unwrap();
            fs::write(dist.join("INSTALLER"), installer).unwrap();
            fs::write(dist.join("RECORD"), "testing/__init__.py,sha256=123,100\n").unwrap();
            if let Some(direct) = direct {
                fs::write(dist.join("direct_url.json"), direct).unwrap();
            }
        }
        let conda = prefix.join("conda-meta");
        fs::create_dir(&conda).unwrap();
        fs::write(conda.join("python-igraph-1.0-build.json"), r#"{"name":"python-igraph","version":"1.0","channel":"https://conda.anaconda.org/conda-forge","files":["lib/python3.12/site-packages/igraph-1.0.dist-info/METADATA"]}"#).unwrap();
        let packages = python_packages(prefix, [site], true);
        assert_eq!(
            packages["igraph"].conda.as_ref().unwrap().0,
            "python-igraph"
        );
        assert_eq!(packages["igraph"].source.label(), "conda-forge");
        assert_eq!(packages["example-pkg"].source, Source::Pypi);
        assert_eq!(packages["local"].source, Source::Local);
        assert!(matches!(packages["git"].source, Source::Github(_)));
        assert!(packages["example-pkg"].imports.contains("testing"));
    }
}
