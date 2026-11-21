//! Where packages are downloaded from (`docs/design/clean-footprint-plan.md`,
//! section 11): CRAN, Bioconductor, conda-forge and bioconda, and PyPI, each
//! from its main site or a mirror in mainland China. `auto` picks the fastest
//! that works, checked at most once a day; the system's proxy is followed.
//! Every command the planners build names the chosen sources.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, ensure};
use sandhill_proto::{InstallPrefs, SourceInfo, SourceOption, SourcePrefs};
use serde::{Deserialize, Serialize};

use super::install;
use super::package_plan::r_string;

/// One place a kind of package can come from.
struct Known {
    name: &'static str,
    url: &'static str,
    /// PyPI only: where a project's JSON description is, `{}` for its name.
    json: &'static str,
}

const fn known(name: &'static str, url: &'static str) -> Known {
    Known {
        name,
        url,
        json: "",
    }
}

/// Checked with `curl -I` on 2026-11-21. Each mirror serves the same paths
/// as its main site: CRAN's `src/contrib` and `web/packages`,
/// Bioconductor's `packages/release` (USTC's through a redirect), the
/// channels' `noarch/repodata.json`, and PyPI's simple index.
const CRAN: &[Known] = &[
    known("CRAN, worldwide", "https://cloud.r-project.org"),
    known(
        "Tsinghua University, China",
        "https://mirrors.tuna.tsinghua.edu.cn/CRAN",
    ),
    known("USTC, China", "https://mirrors.ustc.edu.cn/CRAN"),
];
const BIOCONDUCTOR: &[Known] = &[
    known("Bioconductor, worldwide", "https://bioconductor.org"),
    known(
        "Tsinghua University, China",
        "https://mirrors.tuna.tsinghua.edu.cn/bioconductor",
    ),
    known("USTC, China", "https://mirrors.ustc.edu.cn/bioc"),
];
const CONDA: &[Known] = &[
    known("Anaconda.org, worldwide", "https://conda.anaconda.org"),
    known(
        "Tsinghua University, China",
        "https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud",
    ),
    known("USTC, China", "https://mirrors.ustc.edu.cn/anaconda/cloud"),
];
const PYPI: &[Known] = &[
    Known {
        name: "PyPI, worldwide",
        url: "https://pypi.org/simple",
        json: "https://pypi.org/pypi/{}/json",
    },
    Known {
        name: "Tsinghua University, China",
        url: "https://pypi.tuna.tsinghua.edu.cn/simple",
        json: "https://pypi.tuna.tsinghua.edu.cn/pypi/{}/json",
    },
    Known {
        name: "USTC, China",
        url: "https://mirrors.ustc.edu.cn/pypi/simple",
        json: "https://mirrors.ustc.edu.cn/pypi/{}/json",
    },
];

/// The kinds, as [`SourceInfo::kind`] names them.
pub(crate) const KINDS: [&str; 4] = ["cran", "bioconductor", "conda", "pypi"];

fn options(kind: &str) -> &'static [Known] {
    match kind {
        "cran" => CRAN,
        "bioconductor" => BIOCONDUCTOR,
        "conda" => CONDA,
        _ => PYPI,
    }
}

/// A small file every copy of the source has, for the check.
fn probe_url(kind: &str, url: &str) -> String {
    match kind {
        "cran" => format!("{url}/src/contrib/PACKAGES.gz"),
        "bioconductor" => format!("{url}/packages/release/bioc/VIEWS"),
        "conda" => format!("{url}/conda-forge/noarch/repodata.json"),
        _ => format!("{url}/pip/"),
    }
}

const FRESH: Duration = Duration::from_secs(86_400);
const TIMEOUT: Duration = Duration::from_secs(4);

/// The last check, kept in Sandhill's state for a day.
#[derive(Default, Serialize, Deserialize)]
struct Checked {
    /// Seconds since 1970.
    at: u64,
    results: BTreeMap<String, Vec<Timing>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Timing {
    url: String,
    ms: Option<u32>,
    works: bool,
}

fn cache_path(record: &Path) -> std::path::PathBuf {
    record.with_file_name("download-sources.json")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn read_cache(record: &Path) -> Checked {
    fs::read(cache_path(record))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

impl Checked {
    fn fresh(&self) -> bool {
        !self.results.is_empty() && now().saturating_sub(self.at) < FRESH.as_secs()
    }

    /// The fastest source that answered, else the main one.
    fn fastest(&self, kind: &str) -> &'static str {
        let main = options(kind)[0].url;
        self.results
            .get(kind)
            .into_iter()
            .flatten()
            .filter(|t| t.works)
            .min_by_key(|t| t.ms.unwrap_or(u32::MAX))
            .and_then(|t| options(kind).iter().find(|k| k.url == t.url))
            .map_or(main, |k| k.url)
    }
}

/// The sources every command uses now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Sources {
    pub cran: String,
    pub bioconductor: String,
    pub conda: String,
    pub pypi: String,
}

impl Default for Sources {
    fn default() -> Self {
        let main = |kind| options(kind)[0].url.to_string();
        Self {
            cran: main("cran"),
            bioconductor: main("bioconductor"),
            conda: main("conda"),
            pypi: main("pypi"),
        }
    }
}

impl Sources {
    fn get(&self, kind: &str) -> &str {
        match kind {
            "cran" => &self.cran,
            "bioconductor" => &self.bioconductor,
            "conda" => &self.conda,
            _ => &self.pypi,
        }
    }

    /// The main site, not a mirror.
    pub fn is_main(&self, kind: &str) -> bool {
        self.get(kind) == options(kind)[0].url
    }

    /// A conda channel at the chosen source: `conda-forge`, `bioconda`.
    pub fn channel(&self, name: &str) -> String {
        format!("{}/{name}", self.conda)
    }

    /// conda-forge, then bioconda, and nothing from the user's settings.
    pub fn conda_args(&self) -> Vec<String> {
        vec![
            "--override-channels".into(),
            "-c".into(),
            self.channel("conda-forge"),
            "-c".into(),
            self.channel("bioconda"),
        ]
    }

    /// R code that points CRAN and BiocManager at the chosen sources.
    pub fn r_options(&self) -> String {
        format!(
            "options(repos = c(CRAN = {}), BioC_mirror = {})",
            r_string(&self.cran),
            r_string(&self.bioconductor)
        )
    }

    /// pip's and uv's argument for the chosen PyPI.
    pub fn pip_index(&self) -> [&str; 2] {
        ["--index-url", &self.pypi]
    }

    /// A project's JSON description, from the chosen PyPI.
    pub fn pypi_json(&self, name: &str) -> String {
        PYPI.iter()
            .find(|k| k.url == self.pypi)
            .unwrap_or(&PYPI[0])
            .json
            .replace("{}", name)
    }

    /// For a conda environment file, whose own channels are short names:
    /// the variables that point those names at the chosen source.
    pub fn channel_alias(&self) -> Option<[String; 2]> {
        (!self.is_main("conda")).then(|| {
            [
                format!("CONDA_CHANNEL_ALIAS={}", self.conda),
                format!("MAMBA_CHANNEL_ALIAS={}", self.conda),
            ]
        })
    }
}

fn resolve(prefs: &SourcePrefs, checked: &Checked) -> Sources {
    let pick = |kind: &str, chosen: &str| -> String {
        if options(kind).iter().any(|k| k.url == chosen) {
            chosen.into()
        } else {
            checked.fastest(kind).into()
        }
    };
    Sources {
        cran: pick("cran", &prefs.cran),
        bioconductor: pick("bioconductor", &prefs.bioconductor),
        conda: pick("conda", &prefs.conda),
        pypi: pick("pypi", &prefs.pypi),
    }
}

/// The sources chosen now, from the last check; never waits for the network.
pub(crate) fn chosen(record: &Path) -> Sources {
    resolve(&install::prefs(record).sources, &read_cache(record))
}

/// Each kind with its sources, their timings and the one used now. Checks
/// again when asked or when the last check is more than a day old.
pub(crate) fn list(record: &Path, check: bool) -> Vec<SourceInfo> {
    let mut checked = read_cache(record);
    if check || !checked.fresh() {
        checked = run_check(record);
    }
    let prefs = install::prefs(record).sources;
    let sources = resolve(&prefs, &checked);
    let proxy = proxy();
    KINDS
        .into_iter()
        .map(|kind| {
            let timings = checked.results.get(kind);
            SourceInfo {
                kind: kind.into(),
                chosen: sources.get(kind).into(),
                automatic: !options(kind).iter().any(|k| k.url == pref(&prefs, kind)),
                options: options(kind)
                    .iter()
                    .map(|k| {
                        let timing = timings.and_then(|t| t.iter().find(|t| t.url == k.url));
                        SourceOption {
                            name: k.name.into(),
                            url: k.url.into(),
                            ms: timing.and_then(|t| t.ms),
                            works: timing.is_some_and(|t| t.works),
                        }
                    })
                    .collect(),
                proxy: proxy.clone(),
            }
        })
        .collect()
}

fn pref<'a>(prefs: &'a SourcePrefs, kind: &str) -> &'a str {
    match kind {
        "cran" => &prefs.cran,
        "bioconductor" => &prefs.bioconductor,
        "conda" => &prefs.conda,
        _ => &prefs.pypi,
    }
}

/// Chooses the sources: `auto` or one of the listed URLs for each kind.
pub(crate) fn set(record: &Path, sources: SourcePrefs) -> Result<InstallPrefs> {
    for kind in KINDS {
        let chosen = pref(&sources, kind);
        ensure!(
            chosen == "auto" || options(kind).iter().any(|k| k.url == chosen),
            "Choose Automatic or one of the sources listed."
        );
    }
    install::update_prefs(record, |prefs| prefs.sources = sources)
}

/// Checks again in the background when the last check is a day old.
pub(crate) fn refresh_if_stale(record: &Path) {
    if !read_cache(record).fresh() {
        run_check(record);
    }
}

fn run_check(record: &Path) -> Checked {
    static CHECKING: AtomicBool = AtomicBool::new(false);
    if CHECKING.swap(true, Ordering::AcqRel) {
        // Another check is running; use what is there.
        return read_cache(record);
    }
    let checked = Checked {
        at: now(),
        results: check_all(proxy().as_deref()),
    };
    if let Ok(bytes) = serde_json::to_vec(&checked) {
        let _ = crate::files::write_atomic(&cache_path(record), &bytes);
    }
    CHECKING.store(false, Ordering::Release);
    checked
}

/// An HTTP client for Sandhill's own requests, through the same proxy.
pub(super) fn http(timeout: Duration) -> ureq::Agent {
    agent(timeout, proxy().as_deref())
}

fn agent(timeout: Duration, proxy: Option<&str>) -> ureq::Agent {
    let mut config = ureq::Agent::config_builder().timeout_global(Some(timeout));
    if let Some(proxy) = proxy.and_then(|p| ureq::Proxy::new(p).ok()) {
        config = config.proxy(Some(proxy));
    }
    config.build().new_agent()
}

fn check_all(proxy: Option<&str>) -> BTreeMap<String, Vec<Timing>> {
    let agent = agent(TIMEOUT, proxy);
    std::thread::scope(|scope| {
        let jobs: Vec<_> = KINDS
            .into_iter()
            .flat_map(|kind| options(kind).iter().map(move |k| (kind, k.url)))
            .map(|(kind, url)| {
                let agent = &agent;
                scope.spawn(move || {
                    let start = Instant::now();
                    let works = agent.head(&probe_url(kind, url)).call().is_ok();
                    let ms = u32::try_from(start.elapsed().as_millis()).unwrap_or(u32::MAX);
                    (
                        kind,
                        Timing {
                            url: url.into(),
                            ms: works.then_some(ms),
                            works,
                        },
                    )
                })
            })
            .collect();
        let mut results: BTreeMap<String, Vec<Timing>> = BTreeMap::new();
        for job in jobs {
            if let Ok((kind, timing)) = job.join() {
                results.entry(kind.into()).or_default().push(timing);
            }
        }
        results
    })
}

const PROXY_VARS: [&str; 4] = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"];

/// The proxy downloads go through: the one the environment already sets,
/// else the system's (macOS's network settings).
pub(crate) fn proxy() -> Option<String> {
    env_proxy().or_else(system_proxy)
}

fn env_proxy() -> Option<String> {
    PROXY_VARS
        .into_iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
}

/// macOS's HTTPS proxy, else its web proxy. Elsewhere the system's proxy
/// is the one already in the environment.
fn system_proxy() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = install::probe(Path::new("/usr/sbin/scutil"), &["--proxy"])?;
    let (https, http) = parse_scutil(&out);
    https.or(http)
}

/// `(HTTPS proxy, web proxy)` from `scutil --proxy`.
fn parse_scutil(text: &str) -> (Option<String>, Option<String>) {
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(" : ") {
            fields.insert(key.trim(), value.trim());
        }
    }
    let proxy = |prefix: &str| {
        (fields.get(format!("{prefix}Enable").as_str()) == Some(&"1"))
            .then(|| {
                let host = fields.get(format!("{prefix}Proxy").as_str())?;
                let port = fields.get(format!("{prefix}Port").as_str())?;
                (!host.is_empty()
                    && host
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
                    && port.parse::<u16>().is_ok())
                .then(|| format!("http://{host}:{port}"))
            })
            .flatten()
    };
    (proxy("HTTPS"), proxy("HTTP"))
}

/// The variables install jobs get when the user's environment sets no
/// proxy and the system has one.
pub(crate) fn proxy_env() -> Vec<(String, String)> {
    if env_proxy().is_some() {
        return Vec::new();
    }
    system_proxy().map_or_else(Vec::new, |proxy| {
        PROXY_VARS
            .into_iter()
            .map(|key| (key.to_string(), proxy.clone()))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timing(url: &str, ms: Option<u32>) -> Timing {
        Timing {
            url: url.into(),
            ms,
            works: ms.is_some(),
        }
    }

    #[test]
    fn sources_list_the_main_site_and_the_china_mirrors() {
        for kind in KINDS {
            let urls: Vec<_> = options(kind).iter().map(|k| k.url).collect();
            assert_eq!(urls.len(), 3, "{kind}");
            assert!(urls[1].contains("tsinghua") && urls[2].contains("ustc"));
            assert!(
                urls.iter()
                    .all(|u| u.starts_with("https://") && !u.ends_with('/'))
            );
        }
        assert_eq!(
            probe_url("conda", CONDA[1].url),
            "https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/conda-forge/noarch/repodata.json"
        );
        assert_eq!(
            probe_url("pypi", PYPI[2].url),
            "https://mirrors.ustc.edu.cn/pypi/simple/pip/"
        );
    }

    #[test]
    fn auto_picks_the_fastest_that_works_and_a_choice_wins() {
        let checked = Checked {
            at: now(),
            results: BTreeMap::from([
                (
                    "cran".into(),
                    vec![
                        timing(CRAN[0].url, None),
                        timing(CRAN[1].url, Some(120)),
                        timing(CRAN[2].url, Some(80)),
                    ],
                ),
                (
                    "pypi".into(),
                    vec![
                        timing(PYPI[0].url, Some(40)),
                        timing(PYPI[1].url, Some(900)),
                    ],
                ),
            ]),
        };
        let mut prefs = SourcePrefs::default();
        let sources = resolve(&prefs, &checked);
        assert_eq!(sources.cran, CRAN[2].url);
        assert_eq!(sources.pypi, PYPI[0].url);
        // Nothing checked yet, or nothing answered: the main site.
        assert_eq!(sources.conda, CONDA[0].url);
        assert_eq!(resolve(&prefs, &Checked::default()), Sources::default());
        prefs.cran = CRAN[1].url.into();
        assert_eq!(resolve(&prefs, &checked).cran, CRAN[1].url);
        // A URL that isn't listed is ignored.
        prefs.cran = "https://example.org".into();
        assert_eq!(resolve(&prefs, &checked).cran, CRAN[2].url);
    }

    #[test]
    fn commands_carry_the_chosen_sources() {
        let sources = Sources {
            cran: CRAN[1].url.into(),
            bioconductor: BIOCONDUCTOR[2].url.into(),
            conda: CONDA[1].url.into(),
            pypi: PYPI[2].url.into(),
        };
        assert_eq!(
            sources.conda_args(),
            [
                "--override-channels",
                "-c",
                "https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/conda-forge",
                "-c",
                "https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud/bioconda"
            ]
        );
        assert_eq!(
            sources.r_options(),
            "options(repos = c(CRAN = \"https://mirrors.tuna.tsinghua.edu.cn/CRAN\"), BioC_mirror = \"https://mirrors.ustc.edu.cn/bioc\")"
        );
        assert_eq!(
            sources.pip_index(),
            ["--index-url", "https://mirrors.ustc.edu.cn/pypi/simple"]
        );
        assert_eq!(
            sources.pypi_json("numpy"),
            "https://mirrors.ustc.edu.cn/pypi/numpy/json"
        );
        assert_eq!(
            Sources::default().pypi_json("numpy"),
            "https://pypi.org/pypi/numpy/json"
        );
        assert!(Sources::default().channel_alias().is_none());
        assert_eq!(
            sources.channel_alias().unwrap()[0],
            "CONDA_CHANNEL_ALIAS=https://mirrors.tuna.tsinghua.edu.cn/anaconda/cloud"
        );
    }

    #[test]
    fn set_accepts_auto_or_a_listed_source_and_list_reports_the_choice() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join(super::super::record::FILE);
        let mut prefs = SourcePrefs {
            cran: "https://example.org".into(),
            ..SourcePrefs::default()
        };
        assert!(set(&record, prefs.clone()).is_err());
        prefs.cran = CRAN[2].url.into();
        assert_eq!(set(&record, prefs.clone()).unwrap().sources, prefs);
        assert_eq!(chosen(&record).cran, CRAN[2].url);
        // A fresh check, so listing doesn't reach the network.
        let checked = Checked {
            at: now(),
            results: BTreeMap::from([("cran".into(), vec![timing(CRAN[0].url, Some(50))])]),
        };
        fs::write(cache_path(&record), serde_json::to_vec(&checked).unwrap()).unwrap();
        let listed = list(&record, false);
        assert_eq!(listed.len(), 4);
        let cran = &listed[0];
        assert_eq!((cran.chosen.as_str(), cran.automatic), (CRAN[2].url, false));
        assert_eq!(cran.options[0].ms, Some(50));
        assert!(cran.options[0].works && !cran.options[1].works);
        assert!(listed[1].automatic);
    }

    #[test]
    fn scutil_proxy_settings_are_read() {
        let text = "<dictionary> {\n  HTTPEnable : 1\n  HTTPPort : 7890\n  HTTPProxy : 127.0.0.1\n  HTTPSEnable : 0\n  HTTPSPort : 7891\n  HTTPSProxy : 127.0.0.1\n  ProxyAutoConfigEnable : 1\n}";
        assert_eq!(
            parse_scutil(text),
            (None, Some("http://127.0.0.1:7890".into()))
        );
        let text = "<dictionary> {\n  HTTPSEnable : 1\n  HTTPSPort : 3128\n  HTTPSProxy : proxy.example.cn\n}";
        assert_eq!(
            parse_scutil(text).0.as_deref(),
            Some("http://proxy.example.cn:3128")
        );
        assert_eq!(parse_scutil("<dictionary> {\n}"), (None, None));
    }
}
