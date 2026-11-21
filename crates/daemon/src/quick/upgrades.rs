//! R upgrades (`docs/design/clean-footprint-plan.md`, section 11): when R
//! moves to a new minor version (Homebrew's 4.6 to 4.7), the packages
//! installed for the old one stay in its libraries. Sandhill finds those
//! libraries, only reading them, so the R and Python page can offer to
//! reinstall the packages for the new R from where each came from.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use sandhill_proto::RUpgrade;

use super::{install, package_plan, record};

/// Where earlier R versions keep their libraries on this computer.
pub(super) struct Roots {
    pub home: Option<PathBuf>,
    /// Homebrew's prefixes: `<prefix>/lib/R/<x.y>/site-library`.
    pub brew: Vec<PathBuf>,
    /// CRAN's R for macOS: `Versions/<x.y>[-arm64]/Resources/library`.
    pub framework: Option<PathBuf>,
}

impl Roots {
    pub fn here() -> Self {
        Self {
            home: install::home(),
            brew: ["/opt/homebrew", "/usr/local", "/home/linuxbrew/.linuxbrew"]
                .into_iter()
                .map(PathBuf::from)
                .filter(|prefix| prefix.join("lib/R").is_dir())
                .collect(),
            framework: cfg!(target_os = "macos")
                .then(|| PathBuf::from("/Library/Frameworks/R.framework")),
        }
    }
}

/// `(4, 6)` from `4.6.2`, `4.6` or `4.6-arm64`.
pub(super) fn minor(version: &str) -> Option<(u32, u32)> {
    let version = version.split('-').next()?;
    let mut parts = version.split('.');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

/// The folders in `dir` named by an R version: `(x.y, name)`.
fn versions(dir: &Path) -> Vec<((u32, u32), String)> {
    fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((minor(&name)?, name))
        })
        .collect()
}

/// Every R library found for an R version, by that version.
fn libraries(roots: &Roots) -> Vec<((u32, u32), PathBuf)> {
    let mut found = Vec::new();
    for prefix in &roots.brew {
        let dir = prefix.join("lib/R");
        for (version, name) in versions(&dir) {
            found.push((version, dir.join(name).join("site-library")));
        }
    }
    if let Some(framework) = &roots.framework {
        let dir = framework.join("Versions");
        for (version, name) in versions(&dir) {
            found.push((version, dir.join(name).join("Resources/library")));
        }
    }
    if let Some(home) = &roots.home {
        // macOS: ~/Library/R/<arch>/<x.y>/library, and before R 4.2 ~/Library/R/<x.y>/library.
        let mac = home.join("Library/R");
        for (version, name) in versions(&mac) {
            found.push((version, mac.join(name).join("library")));
        }
        for arch in fs::read_dir(&mac).into_iter().flatten().flatten() {
            for (version, name) in versions(&arch.path()) {
                found.push((version, arch.path().join(name).join("library")));
            }
        }
        // R_LIBS_USER elsewhere: ~/R/<platform>-library/<x.y>.
        for platform in fs::read_dir(home.join("R")).into_iter().flatten().flatten() {
            if platform.file_name().to_string_lossy().ends_with("-library") {
                for (version, name) in versions(&platform.path()) {
                    found.push((version, platform.path().join(name)));
                }
            }
        }
    }
    found.retain(|(_, library)| library.is_dir());
    found
}

/// Where a package from an earlier R came from, by its DESCRIPTION.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Source {
    Cran,
    Bioconductor,
    /// `owner/repo`.
    Github(String),
    /// Built from a folder or file on this computer, or from a site
    /// Sandhill doesn't install from.
    Elsewhere,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OldPackage {
    pub name: String,
    pub source: Source,
}

fn source(description: &str) -> Source {
    let field = |name| record::r_field(description, name);
    if field("RemoteType").is_some_and(|t| t.eq_ignore_ascii_case("github"))
        && let (Some(owner), Some(repo)) = (field("RemoteUsername"), field("RemoteRepo"))
    {
        let repo = format!("{owner}/{repo}");
        return if package_plan::github_repo(&repo) {
            Source::Github(repo)
        } else {
            Source::Elsewhere
        };
    }
    match field("Repository") {
        Some("CRAN" | "RSPM") => Source::Cran,
        Some(repository) if repository.starts_with("Bioconductor") => Source::Bioconductor,
        _ if field("git_url").is_some_and(|url| url.contains("bioconductor.org")) => {
            Source::Bioconductor
        }
        _ => Source::Elsewhere,
    }
}

/// The packages in `libraries` this R lacks, without the ones shipped
/// with R. Each library is only read.
pub(super) fn missing(libraries: &[PathBuf], have: &HashSet<&str>) -> Vec<OldPackage> {
    let mut seen = BTreeSet::new();
    let mut found = Vec::new();
    for library in libraries {
        let mut entries: Vec<_> = fs::read_dir(library)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| record::r_name(name))
            .collect();
        entries.sort();
        for name in entries {
            if have.contains(name.as_str()) || seen.contains(&name) {
                continue;
            }
            let Ok(description) = fs::read_to_string(library.join(&name).join("DESCRIPTION"))
            else {
                continue;
            };
            if record::r_field(&description, "Package") != Some(name.as_str())
                || matches!(
                    record::r_field(&description, "Priority"),
                    Some("base" | "recommended")
                )
            {
                continue;
            }
            seen.insert(name.clone());
            found.push(OldPackage {
                source: source(&description),
                name,
            });
        }
    }
    found
}

/// For an R at `version` holding `have`: the newest earlier R whose
/// libraries hold packages this R lacks. R versions still on this computer
/// (`present`) are skipped: their libraries are in use, not left behind.
pub(super) fn detect(
    roots: &Roots,
    version: &str,
    have: &HashSet<&str>,
    present: &BTreeSet<(u32, u32)>,
) -> Option<RUpgrade> {
    let this = minor(version)?;
    let mut by_version: BTreeMap<(u32, u32), Vec<PathBuf>> = BTreeMap::new();
    for (old, library) in libraries(roots) {
        if old < this && !present.contains(&old) {
            let libraries = by_version.entry(old).or_default();
            if !libraries.contains(&library) {
                libraries.push(library);
            }
        }
    }
    by_version
        .into_iter()
        .rev()
        .find_map(|((x, y), libraries)| {
            let libraries: Vec<_> = libraries
                .into_iter()
                .filter(|library| !missing(std::slice::from_ref(library), have).is_empty())
                .collect();
            let count = missing(&libraries, have).len();
            (count > 0).then(|| RUpgrade {
                version: format!("{x}.{y}"),
                libraries: libraries
                    .iter()
                    .map(|l| l.to_string_lossy().into_owned())
                    .collect(),
                packages: u32::try_from(count).unwrap_or(u32::MAX),
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quick::test_common::tempdir;

    fn package(library: &Path, name: &str, extra: &str) {
        let dir = library.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("DESCRIPTION"),
            format!("Package: {name}\nVersion: 1.0\n{extra}"),
        )
        .unwrap();
    }

    #[test]
    fn finds_packages_left_by_an_earlier_r_in_each_layout() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let brew = root.join("homebrew");
        let framework = root.join("R.framework");
        let home = root.join("home");
        let brew_old = brew.join("lib/R/4.6/site-library");
        let cran_old = framework.join("Versions/4.5-arm64/Resources/library");
        let user_old = home.join("Library/R/arm64/4.6/library");
        let linux_old = home.join("R/x86_64-pc-linux-gnu-library/4.6");
        package(&brew_old, "Seurat", "Repository: CRAN\n");
        package(&brew_old, "ggplot2", "Repository: CRAN\n");
        package(
            &user_old,
            "DESeq2",
            "git_url: https://git.bioconductor.org/packages/DESeq2\n",
        );
        package(
            &user_old,
            "presto",
            "RemoteType: github\nRemoteUsername: immunogenomics\nRemoteRepo: presto\n",
        );
        package(&linux_old, "mine", "");
        package(&cran_old, "stats", "Priority: base\n");
        package(&cran_old, "limma", "Repository: Bioconductor 3.21\n");
        // The new R's own library: not an earlier version.
        package(
            &brew.join("lib/R/4.7/site-library"),
            "fresh",
            "Repository: CRAN\n",
        );
        let roots = Roots {
            home: Some(home.clone()),
            brew: vec![brew.clone()],
            framework: Some(framework.clone()),
        };
        let have = HashSet::from(["ggplot2"]);
        let upgrade = detect(&roots, "4.7.0", &have, &BTreeSet::new()).unwrap();
        assert_eq!(upgrade.version, "4.6");
        assert_eq!(upgrade.packages, 4);
        assert_eq!(upgrade.libraries.len(), 3);
        let libraries: Vec<_> = upgrade.libraries.iter().map(PathBuf::from).collect();
        let old = missing(&libraries, &have);
        let by_name = |name: &str| old.iter().find(|p| p.name == name).unwrap().source.clone();
        assert_eq!(by_name("Seurat"), Source::Cran);
        assert_eq!(by_name("DESeq2"), Source::Bioconductor);
        assert_eq!(
            by_name("presto"),
            Source::Github("immunogenomics/presto".into())
        );
        assert_eq!(by_name("mine"), Source::Elsewhere);
        // An R 4.6 still on this computer uses those libraries: then 4.5's
        // non-base packages are what's left behind.
        let present = BTreeSet::from([(4, 6)]);
        let upgrade = detect(&roots, "4.7.0", &have, &present).unwrap();
        assert_eq!((upgrade.version.as_str(), upgrade.packages), ("4.5", 1));
        // Nothing is newer than an R 4.4.
        assert!(detect(&roots, "4.4.3", &have, &BTreeSet::new()).is_none());
        // Everything is installed already.
        let all = HashSet::from(["Seurat", "ggplot2", "DESeq2", "presto", "mine", "limma"]);
        assert!(detect(&roots, "4.7.0", &all, &BTreeSet::new()).is_none());
    }
}
