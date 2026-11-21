//! Public package indexes for the R and Python page.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use sandhill_proto::{InstallTool, PackageHit};
use serde::{Deserialize, Serialize};

use super::{environments, package_plan, record};
use crate::kernelspecs::Search;

const BIOC: [&str; 3] = ["bioc", "data/annotation", "data/experiment"];
const CRAN: &str = "https://cloud.r-project.org";
const FRESH: Duration = Duration::from_secs(86_400);
const LIMIT: u64 = 16 * 1024 * 1024;
static HTTP: LazyLock<ureq::Agent> = LazyLock::new(|| {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .new_agent()
});

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct Entry {
    name: String,
    title: String,
    version: Option<String>,
}

fn download(url: &str, query: &[(&str, &str)]) -> Result<Vec<u8>> {
    let mut req = HTTP.get(url);
    for (key, value) in query {
        req = req.query(key, value);
    }
    let mut out = Vec::new();
    req.call()?
        .body_mut()
        .as_reader()
        .take(LIMIT + 1)
        .read_to_end(&mut out)?;
    if out.len() as u64 > LIMIT {
        bail!("The package list is too large.");
    }
    Ok(out)
}

fn cache<T: Serialize + for<'de> Deserialize<'de>>(
    record: &Path,
    key: &str,
    fetch: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let path = environments::state_path(record, "package-index", key);
    let cached = fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    if path
        .metadata()
        .and_then(|m| m.modified())
        .is_ok_and(|date| date.elapsed().is_ok_and(|age| age < FRESH))
        && let Some(cached) = cached
    {
        return Ok(cached);
    }
    match fetch() {
        Ok(value) => {
            // Each download publishes a complete index. Another request can use the old one until then.
            if let Some(parent) = path.parent()
                && fs::create_dir_all(parent).is_ok()
                && let Ok(mut tmp) = tempfile::NamedTempFile::new_in(parent)
                && serde_json::to_writer(&mut tmp, &value).is_ok()
                && tmp.flush().is_ok()
            {
                let _ = tmp.persist(&path);
            }
            Ok(value)
        }
        Err(error) => cached.ok_or(error),
    }
}

fn cran(record: &Path) -> Result<Vec<Entry>> {
    cache(record, "cran", || {
        let (html, versions) = std::thread::scope(|scope| {
            let versions = scope.spawn(|| {
                let body = download(&format!("{CRAN}/src/contrib/PACKAGES.gz"), &[]).ok()?;
                let mut data = String::new();
                flate2::read::GzDecoder::new(body.as_slice())
                    .take(LIMIT * 3)
                    .read_to_string(&mut data)
                    .ok()?;
                Some(
                    dcf(&data)
                        .into_iter()
                        .filter_map(|p| Some((p.name, p.version?)))
                        .collect::<HashMap<_, _>>(),
                )
            });
            let html = download(
                &format!("{CRAN}/web/packages/available_packages_by_name.html"),
                &[],
            );
            (html, versions.join().ok().flatten().unwrap_or_default())
        });
        let mut list = parse_cran(&String::from_utf8_lossy(&html?));
        if list.is_empty() {
            bail!("CRAN did not return a package list.");
        }
        for entry in &mut list {
            entry.version = versions.get(&entry.name).cloned();
        }
        Ok(list)
    })
}

fn parse_cran(html: &str) -> Vec<Entry> {
    use scraper::{Html, Selector};
    let tr = Selector::parse("tr").unwrap();
    let td = Selector::parse("td").unwrap();
    let link = Selector::parse("a").unwrap();
    Html::parse_document(html)
        .select(&tr)
        .filter_map(|row| {
            let mut cells = row.select(&td);
            let name = cells
                .next()?
                .select(&link)
                .next()?
                .text()
                .collect::<String>();
            let name = name.trim();
            record::r_name(name).then_some(())?;
            let title = cells.next()?.text().collect::<String>();
            Some(Entry {
                name: name.to_owned(),
                title: one_line(&title),
                version: None,
            })
        })
        .collect()
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn dcf(text: &str) -> Vec<Entry> {
    text.split("\n\n")
        .filter_map(|record| {
            let mut name = None;
            let mut version = None;
            let mut title = String::new();
            let mut in_title = false;
            for line in record.lines() {
                if line.starts_with([' ', '\t']) {
                    if in_title {
                        title.push(' ');
                        title.push_str(line.trim());
                    }
                    continue;
                }
                in_title = false;
                match line.split_once(':') {
                    Some(("Package", value)) => name = Some(value.trim()),
                    Some(("Version", value)) => version = Some(value.trim().into()),
                    Some(("Title", value)) => {
                        in_title = true;
                        title.push_str(value.trim());
                    }
                    _ => {}
                }
            }
            let name = name?;
            record::r_name(name).then(|| Entry {
                name: name.into(),
                title,
                version,
            })
        })
        .collect()
}

fn bioc(record: &Path, category: &str) -> Result<Vec<Entry>> {
    cache(record, &format!("bioc:{category}"), || {
        let text = download(
            &format!("https://bioconductor.org/packages/release/{category}/VIEWS"),
            &[],
        )?;
        let list = dcf(&String::from_utf8_lossy(&text));
        if list.is_empty() {
            bail!("Bioconductor did not return a package list.");
        }
        Ok(list)
    })
}

pub(super) fn bioc_names(record: &Path) -> HashSet<String> {
    std::thread::scope(|scope| {
        BIOC.into_iter()
            .map(|category| scope.spawn(move || bioc(record, category)))
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|job| job.join().ok()?.ok())
            .flatten()
            .map(|p| p.name)
            .collect()
    })
}

fn parse_anaconda(text: &[u8]) -> Result<Vec<(Entry, String)>> {
    #[derive(Deserialize)]
    struct Conda {
        owner: String,
        name: String,
        latest_version: Option<String>,
        summary: Option<String>,
    }
    let list: Vec<Conda> = serde_json::from_slice(text)?;
    Ok(list
        .into_iter()
        .filter(|p| matches!(p.owner.as_str(), "conda-forge" | "bioconda"))
        .filter(|p| package_plan::python_spec(&p.name) == Some(&p.name))
        .map(|p| {
            (
                Entry {
                    name: p.name,
                    title: one_line(p.summary.as_deref().unwrap_or_default()),
                    version: p.latest_version,
                },
                p.owner,
            )
        })
        .collect())
}

fn anaconda(record: &Path, query: &str) -> Result<Vec<(Entry, String)>> {
    cache(
        record,
        &format!("conda:{}", query.to_ascii_lowercase()),
        || {
            parse_anaconda(&download(
                "https://api.anaconda.org/search",
                &[("name", query), ("type", "conda")],
            )?)
        },
    )
}

fn parse_pypi(text: &[u8]) -> Result<Entry> {
    #[derive(Deserialize)]
    struct Project {
        info: Info,
    }
    #[derive(Deserialize)]
    struct Info {
        name: String,
        version: String,
        summary: Option<String>,
    }
    let info = serde_json::from_slice::<Project>(text)?.info;
    Ok(Entry {
        name: info.name,
        title: one_line(info.summary.as_deref().unwrap_or_default()),
        version: Some(info.version),
    })
}

fn pypi(record: &Path, query: &str) -> Result<Option<Entry>> {
    if package_plan::python_spec(query) != Some(query) {
        return Ok(None);
    }
    let normalized = record::python_name(query);
    cache(record, &format!("pypi:{normalized}"), || {
        let mut last = None;
        for name in [query, &normalized]
            .into_iter()
            .take(if normalized == query { 1 } else { 2 })
        {
            let response = HTTP
                .get(&format!("https://pypi.org/pypi/{name}/json"))
                .call();
            match response {
                Ok(mut response) => {
                    let mut bytes = Vec::new();
                    response
                        .body_mut()
                        .as_reader()
                        .take(LIMIT)
                        .read_to_end(&mut bytes)?;
                    return Ok(Some(parse_pypi(&bytes)?));
                }
                Err(ureq::Error::StatusCode(404)) => continue,
                Err(error) => last = Some(error),
            }
        }
        match last {
            Some(error) => Err(error.into()),
            None => Ok(None),
        }
    })
}

fn rank(name: &str, title: &str, query: &str) -> Option<u8> {
    let name = name.to_lowercase();
    if name == query {
        Some(0)
    } else if name.starts_with(query) {
        Some(1)
    } else if name.contains(query) {
        Some(2)
    } else if title.to_lowercase().contains(query) {
        Some(3)
    } else {
        None
    }
}

pub(crate) fn search(
    search: &Search,
    record_path: &Path,
    env: &str,
    query: &str,
) -> Result<Vec<PackageHit>> {
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    if query.len() > 240 && !Path::new(query).is_absolute() {
        bail!("Enter a shorter package name.");
    }
    let env = environments::get(search, record_path, env)?;
    let r = env.is_r();
    let pip = env
        .card
        .tools
        .iter()
        .map(|t| t.tool)
        .find(|t| matches!(t, InstallTool::Pip | InstallTool::Uv));
    if Path::new(query).is_absolute() {
        package_plan::package_file(query, r)?;
        let tool = if r { Some(InstallTool::R) } else { pip }
            .context("No tool can install a package file into this folder.")?;
        return Ok(vec![PackageHit {
            name: query.into(),
            title: Path::new(query)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            version: None,
            source: "This file".into(),
            tool,
            installed: None,
        }]);
    }
    if r && package_plan::github_repo(query) {
        return Ok(vec![PackageHit {
            name: query.into(),
            title: "Package on GitHub".into(),
            version: None,
            source: "GitHub".into(),
            tool: InstallTool::R,
            installed: None,
        }]);
    }
    let conda = env.tool(InstallTool::Conda).is_some();
    let lookup = query.to_lowercase();
    let mut searched = false;
    let mut hits = Vec::new();
    let mut add = |list: Result<Vec<Entry>>, source: &str, tool: InstallTool| {
        if let Ok(list) = list {
            searched = true;
            for entry in list {
                let Some(rank) = rank(&entry.name, &entry.title, &lookup) else {
                    continue;
                };
                let installed = if r {
                    env.packages
                        .iter()
                        .find(|(n, _)| n.eq_ignore_ascii_case(&entry.name))
                        .map(|(_, v)| v.clone())
                } else {
                    env.packages.get(&record::python_name(&entry.name)).cloned()
                };
                hits.push((
                    rank,
                    entry.name.to_lowercase(),
                    PackageHit {
                        name: entry.name,
                        title: entry.title,
                        version: entry.version,
                        source: source.into(),
                        tool,
                        installed,
                    },
                ));
            }
        }
    };
    std::thread::scope(|scope| {
        let conda_job = (!r || conda).then(|| scope.spawn(|| anaconda(record_path, query)));
        if r {
            let cran_job = scope.spawn(|| cran(record_path));
            let jobs = BIOC
                .into_iter()
                .map(|category| scope.spawn(move || bioc(record_path, category)))
                .collect::<Vec<_>>();
            add(cran_job.join().unwrap(), "CRAN", InstallTool::R);
            for job in jobs {
                add(job.join().unwrap(), "Bioconductor", InstallTool::R);
            }
        } else if let Some(tool) = pip {
            add(
                pypi(record_path, query).map(|item| item.into_iter().collect()),
                "PyPI",
                tool,
            );
        }
        if let Some(Ok(list)) = conda_job.map(|job| job.join().unwrap()) {
            if conda {
                let mut channels = HashMap::<String, Vec<Entry>>::new();
                for (mut item, source) in list {
                    if r {
                        let Some(trimmed) = item
                            .name
                            .strip_prefix("r-")
                            .or_else(|| item.name.strip_prefix("bioconductor-"))
                        else {
                            continue;
                        };
                        item.name = trimmed.into();
                    }
                    channels.entry(source).or_default().push(item);
                }
                for source in ["conda-forge", "bioconda"] {
                    add(
                        Ok(channels.remove(source).unwrap_or_default()),
                        source,
                        InstallTool::Conda,
                    );
                }
            } else if let Some(tool) = pip {
                // Anaconda has a search index. Its results are usable here only when PyPI has the same package.
                let mut names: Vec<_> = list
                    .into_iter()
                    .filter_map(|(item, _)| {
                        Some((rank(&item.name, &item.title, &lookup)?, item.name))
                    })
                    .collect();
                names.sort_unstable();
                names.dedup();
                let jobs = names
                    .into_iter()
                    .take(30)
                    .map(|(_, name)| scope.spawn(move || pypi(record_path, &name)))
                    .collect::<Vec<_>>();
                add(Ok(Vec::new()), "PyPI", tool);
                for job in jobs {
                    add(
                        job.join().unwrap().map(|p| p.into_iter().collect()),
                        "PyPI",
                        tool,
                    );
                }
            }
        }
    });
    if !searched {
        bail!("Couldn't reach a package list. Check your internet connection and try again.");
    }
    hits.sort_by(|(a_rank, a_name, a), (b_rank, b_name, b)| {
        (a_rank, a_name, &a.source).cmp(&(b_rank, b_name, &b.source))
    });
    let mut seen = HashSet::new();
    Ok(hits
        .into_iter()
        .filter(|(_, name, hit)| seen.insert((name.clone(), hit.source.clone())))
        .take(30)
        .map(|(_, _, hit)| hit)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_index_snippets_and_search_order() {
        assert_eq!(parse_cran("<table><tr><td><a href='praise/index.html'>praise</a></td><td> Praise   Users </td></tr></table>")[0].title, "Praise Users");
        let bioc = dcf(
            "Package: DESeq2\nTitle: Differential gene\n expression analysis\nVersion: 1.46.0\n\nPackage: GenomicRanges\nTitle: Ranges\n",
        );
        assert_eq!(bioc[0].title, "Differential gene expression analysis");
        assert_eq!(bioc[0].version.as_deref(), Some("1.46.0"));
        assert_eq!(parse_anaconda(br#"[{"owner":"bioconda","name":"gseapy","latest_version":"1.2","summary":"Pathways"},{"owner":"someone","name":"gseapy"}]"#).unwrap().len(), 1);
        assert_eq!(
            parse_pypi(br#"{"info":{"name":"numpy","version":"2.2","summary":"Array\ndata"}}"#)
                .unwrap()
                .title,
            "Array data"
        );
        assert_eq!(
            [
                rank("Num", "", "num"),
                rank("Numpy", "", "num"),
                rank("Anum", "", "num"),
                rank("other", "NUM", "num")
            ],
            [Some(0), Some(1), Some(2), Some(3)]
        );
    }
}
