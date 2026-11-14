//! Journal authority, project ownership, freshness, and managed atomic exports.
use super::{checks, guides};
use crate::{Daemon, journal::now_ms};
use anyhow::{Context, Result, bail};
use sandhill_proto::SessionStatus;
use sandhill_proto::analysis::{
    AnalysisCheck, AnalysisRecord, AnalysisSpec, CheckStatus, FileEvidence, Workflow,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};

fn project(path: &str) -> Result<String> {
    let path = Path::new(path);
    if !path.is_absolute() || !path.is_dir() {
        bail!("project must be an existing absolute directory");
    }
    Ok(path.canonicalize()?.to_string_lossy().into_owned())
}

fn identifier(name: &str, language: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_' || language == "r" && first == '.')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || language == "r" && c == '.')
        && !(language == "r" && (name == "." || name.starts_with("..")))
}

fn absolute(path: &str) -> Result<PathBuf> {
    let path = Path::new(path);
    if !path.is_absolute() || path.components().any(|c| matches!(c, Component::ParentDir)) {
        bail!("file paths must be absolute and cannot contain '..'");
    }
    Ok(path.to_path_buf())
}

fn evidence(path: &str) -> Result<FileEvidence> {
    let path = absolute(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let before = file.metadata()?;
    if !before.is_file() {
        bail!("not a regular input file: {}", path.display());
    }
    let mut hash = Sha256::new();
    let mut buffer = [0; 128 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let after = file.metadata()?;
    let current = std::fs::metadata(&path)?;
    if (
        before.dev(),
        before.ino(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
    ) != (
        after.dev(),
        after.ino(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
    ) || (after.dev(), after.ino()) != (current.dev(), current.ino())
    {
        bail!("file changed while hashing: {}", path.display());
    }
    Ok(FileEvidence {
        path: path.to_string_lossy().into_owned(),
        sha256: format!("sha256:{}", sandhill_proto::hex(&hash.finalize())),
        size: after.len(),
    })
}

async fn snapshot(paths: Vec<String>) -> Result<Vec<FileEvidence>> {
    tokio::task::spawn_blocking(move || paths.iter().map(|p| evidence(p)).collect()).await?
}

fn owned_path(record: &AnalysisRecord, path: &str) -> Result<()> {
    let path = absolute(path)?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("missing analysis file {}", path.display()))?;
    let output = record
        .spec
        .output_dir
        .as_deref()
        .context("missing output directory")?;
    if !canonical.starts_with(&record.project) && !canonical.starts_with(output) {
        bail!(
            "analysis file is not owned by the project or declared output directory: {}",
            path.display()
        );
    }
    Ok(())
}

fn detail_paths(record: &AnalysisRecord, details: &Value) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for key in [
        "result_file",
        "marker_file",
        "annotation_file",
        "qc_exclusions_file",
    ] {
        if let Some(path) = details.get(key) {
            let path = path
                .as_str()
                .with_context(|| format!("{key} must be an absolute filename"))?;
            owned_path(record, path)?;
            paths.push(path.to_owned());
        }
    }
    for key in ["documents", "code_files"] {
        if let Some(documents) = details.get(key) {
            for path in documents
                .as_array()
                .with_context(|| format!("{key} must be a list"))?
            {
                let path = path.as_str().context("document must be a filename")?;
                owned_path(record, path)?;
                paths.push(path.to_owned());
            }
        }
    }
    Ok(paths)
}

fn current_contract(record: &AnalysisRecord) -> Result<()> {
    if record.spec.guide_digest != guides::digest(&record.spec.guide_id)?
        || record.spec.checker_digest != checks::digest(&record.language)
    {
        bail!(
            "guide/checker version changed; restart the Agent session, reread the guide and begin a new analysis"
        );
    }
    Ok(())
}

fn reject_managed_aliases(
    record: &AnalysisRecord,
    paths: &[String],
    checks: &[AnalysisCheck],
) -> Result<()> {
    let root = Path::new(&record.project)
        .join(".sandhill/analysis")
        .join(&record.id);
    let mut targets = vec![
        root.join("manifest.json"),
        root.join("study-design.json"),
        root.join("report.md"),
    ];
    targets.extend(
        checks
            .iter()
            .map(|check| root.join("checks").join(format!("{}.json", check.id))),
    );
    let identities: Vec<_> = targets
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|m| (m.dev(), m.ino()))
        .collect();
    for path in paths {
        let path = absolute(path)?;
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if path.starts_with(&root) || canonical.starts_with(&root) {
            bail!(
                "checked file overlaps managed analysis exports: {}",
                path.display()
            );
        }
        if let Ok(meta) = std::fs::metadata(&path)
            && identities.contains(&(meta.dev(), meta.ino()))
        {
            bail!(
                "checked file aliases a managed analysis export: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn same_metadata(a: &Metadata, b: &Metadata) -> bool {
    (
        a.dev(),
        a.ino(),
        a.len(),
        a.nlink(),
        a.mode(),
        a.mtime(),
        a.mtime_nsec(),
        a.ctime(),
        a.ctime_nsec(),
    ) == (
        b.dev(),
        b.ino(),
        b.len(),
        b.nlink(),
        b.mode(),
        b.mtime(),
        b.mtime_nsec(),
        b.ctime(),
        b.ctime_nsec(),
    )
}

fn revalidate_report(
    dir: &File,
    name: &std::ffi::CStr,
    existing: &mut File,
    expected: &Metadata,
    digest: &str,
) -> Result<()> {
    if !same_metadata(expected, &existing.metadata()?) {
        bail!("managed report changed during publication; preserved");
    }
    existing.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 128 * 1024];
    loop {
        let n = existing.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    let mut current: libc::stat = unsafe { std::mem::zeroed() };
    if format!("sha256:{}", sandhill_proto::hex(&hash.finalize())) != digest
        || !same_metadata(expected, &existing.metadata()?)
        || unsafe {
            libc::fstatat(
                dir.as_raw_fd(),
                name.as_ptr(),
                &mut current,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        || current.st_dev as u64 != expected.dev()
        || current.st_ino as u64 != expected.ino()
        || current.st_nlink != 1
        || current.st_mode & libc::S_IFMT != libc::S_IFREG
    {
        bail!("managed report changed during publication; preserved");
    }
    Ok(())
}

impl Daemon {
    pub async fn begin_analysis(&self, root: &str, spec: Value) -> Result<Value> {
        let _guard = self.analysis_gate.lock().await;
        if serde_json::to_vec(&spec)?.len() > 128 * 1024 {
            bail!("analysis declaration exceeds 128KiB");
        }
        let spec: AnalysisSpec =
            serde_json::from_value(spec).context("invalid analysis declaration")?;
        let root = project(root)?;
        if spec.goal.trim().is_empty() || !spec.design.is_object() || !spec.parameters.is_object() {
            bail!("goal, design object and parameters object are required");
        }
        let language = match spec.workflow {
            Workflow::StudyDesign => {
                if spec.guide_id != "bio-study-design"
                    || spec.session.is_some()
                    || spec.object.is_some()
                {
                    bail!("study_design requires bio-study-design and no kernel session/object");
                }
                "design".to_string()
            }
            Workflow::Scrna => {
                let session = spec
                    .session
                    .as_deref()
                    .context("scrna requires an explicit existing session")?;
                let record = self
                    .journal
                    .sessions()?
                    .into_iter()
                    .find(|s| s.id == session)
                    .context("unknown session")?;
                if record.status == SessionStatus::Ended
                    || record
                        .project
                        .as_deref()
                        .map(project)
                        .transpose()?
                        .as_deref()
                        != Some(root.as_str())
                {
                    bail!("session is ended or belongs to another project");
                }
                let language = match crate::cells::normalize_language(&record.language).as_str() {
                    "python" => "python",
                    "r" => "r",
                    _ => bail!("scrna requires an existing Python or R session"),
                };
                let expected = if language == "python" {
                    "scrna-scanpy"
                } else {
                    "scrna-seurat"
                };
                if spec.guide_id != expected {
                    bail!("guide does not match selected kernel/workflow");
                }
                if !identifier(
                    spec.object
                        .as_deref()
                        .context("object variable is required")?,
                    language,
                ) {
                    bail!("object must be a plain variable identifier, never an expression");
                }
                if spec.counts_source["kind"] != "raw_counts"
                    || spec.counts_source["processing_state"] != "raw"
                    || spec.counts_source["provenance"]
                        .as_str()
                        .is_none_or(|p| p.trim().is_empty())
                {
                    bail!(
                        "explicit raw counts kind, processing_state='raw' and provenance are required"
                    );
                }
                for key in if language == "python" {
                    vec!["layer"]
                } else {
                    vec!["assay", "seurat_layer"]
                } {
                    if spec.counts_source[key]
                        .as_str()
                        .is_none_or(|s| s.trim().is_empty())
                    {
                        bail!("counts_source.{key} is required");
                    }
                }
                if spec.species.as_deref().is_none_or(|s| s.trim().is_empty())
                    || !matches!(spec.gene_id_type.as_deref(), Some("symbol" | "ensembl"))
                {
                    bail!("species and gene_id_type symbol|ensembl are required");
                }
                language.to_owned()
            }
        };
        let id = uuid::Uuid::new_v4().simple().to_string();
        let mut record = AnalysisRecord {
            format_version: 1,
            id: id.clone(),
            project: root.clone(),
            created_ms: now_ms(),
            language,
            spec,
            input_files: vec![],
            baseline: Value::Null,
            exports: BTreeMap::new(),
            pending_exports: BTreeMap::new(),
        };
        current_contract(&record)?;
        let output = record
            .spec
            .output_dir
            .clone()
            .unwrap_or_else(|| format!("{root}/results/{id}"));
        let output = absolute(&output)?;
        // Reject links in every output ancestor, including explicitly selected external locations.
        let _ = directory(&output, true)?;
        record.spec.output_dir = Some(output.to_string_lossy().into_owned());
        for document in &record.spec.documents {
            owned_path(&record, document)?;
        }
        record.input_files = snapshot(record.spec.source_files.clone()).await?;
        self.journal.save_analysis(&record, None)?;
        self.publish_analysis(record, None).await
    }

    pub async fn check_analysis(
        &self,
        root: &str,
        id: &str,
        checkpoint: &str,
        details: Value,
    ) -> Result<Value> {
        let _guard = self.analysis_gate.lock().await;
        let mut record = self.owned_analysis(root, id)?;
        current_contract(&record)?;
        if !details.is_object() || serde_json::to_vec(&details)?.len() > 128 * 1024 {
            bail!("details must be an object no larger than 128KiB");
        }
        match record.spec.workflow {
            Workflow::StudyDesign if checkpoint != "design" => {
                bail!("study_design supports only design checkpoint")
            }
            Workflow::Scrna if !matches!(checkpoint, "input" | "qc" | "result") => {
                bail!("scrna supports only input, qc, result checkpoints")
            }
            _ => {}
        }
        let history = self.journal.analysis_checks(id)?;
        let mut paths: Vec<String> = record.input_files.iter().map(|f| f.path.clone()).collect();
        let qc_details = history
            .iter()
            .rev()
            .find(|c| c.checkpoint == "qc")
            .map(|c| c.details.clone())
            .unwrap_or_else(|| json!({}));
        if checkpoint == "result" {
            paths.extend(detail_paths(&record, &qc_details)?);
        }
        paths.extend(detail_paths(&record, &details)?);
        for document in &record.spec.documents {
            owned_path(&record, document)?;
            paths.push(document.clone());
        }
        paths.sort();
        paths.dedup();
        reject_managed_aliases(&record, &paths, &history)?;
        let before = snapshot(paths.clone()).await;
        let mut prerequisites = Vec::new();
        if let Ok(files) = &before {
            for input in &record.input_files {
                if !files
                    .iter()
                    .any(|f| f.path == input.path && f.sha256 == input.sha256)
                {
                    prerequisites.push(checks::incomplete(
                        "input_freshness",
                        "declared input or code changed since begin_analysis; begin a new record",
                    ));
                }
            }
        } else {
            prerequisites.push(checks::incomplete(
                "file_evidence",
                format!("{}", before.as_ref().unwrap_err()),
            ));
        }
        let execution_ids: Vec<String> = match details.get("execution_run_ids") {
            Some(ids) => serde_json::from_value(ids.clone())
                .context("execution_run_ids must be a list of run IDs")?,
            None => vec![],
        };
        if execution_ids.len() > 32 {
            bail!("at most 32 execution run IDs are accepted");
        }
        let execution_evidence = match record.spec.session.as_deref() {
            Some(session) => self
                .journal
                .analysis_execution_evidence(session, &execution_ids)?,
            None if execution_ids.is_empty() => json!([]),
            None => bail!("design checks have no kernel execution runs"),
        };
        if checkpoint == "result" && execution_ids.is_empty() {
            prerequisites.push(checks::incomplete("execution_provenance","result requires actual completed analysis execution_run_ids from the bound session"));
        }
        if record.spec.workflow == Workflow::Scrna
            && checkpoint != "input"
            && record.baseline.is_null()
        {
            prerequisites.push(checks::incomplete(
                "input_baseline",
                "a successful measured input checkpoint is required",
            ));
        }
        if record.spec.workflow == Workflow::Scrna && checkpoint == "result" {
            let input_time = history
                .iter()
                .rev()
                .find(|c| c.checkpoint == "input")
                .map(|c| c.created_ms);
            let qc = history.iter().rev().find(|c| c.checkpoint == "qc");
            if !qc
                .is_some_and(|c| c.technical_pass && input_time.is_some_and(|t| c.created_ms >= t))
            {
                prerequisites.push(checks::incomplete("qc_checkpoint","a successful qc checkpoint after the latest input check is required before final result"));
            }
        }
        let measured = if record.language == "design" {
            checks::Measured {
                run_id: None,
                code_digest: None,
                result: checks::design(&record.spec.design),
            }
        } else if prerequisites.is_empty() {
            let session = record.spec.session.as_deref().context("missing session")?;
            let session_record = self
                .journal
                .sessions()?
                .into_iter()
                .find(|s| s.id == session)
                .context("session no longer exists")?;
            if session_record
                .project
                .as_deref()
                .map(project)
                .transpose()?
                .as_deref()
                != Some(record.project.as_str())
            {
                bail!("session belongs to another project");
            }
            let handle = self.sessions.lock().unwrap().get(session).cloned();
            match handle {
                Some(handle) if handle.is_running() => {
                    let mut design = record.spec.design.clone();
                    design["goal"] = json!(record.spec.goal);
                    design["objective"] = json!(record.spec.goal);
                    let request = json!({"checkpoint":checkpoint,"object":record.spec.object,"counts_source":record.spec.counts_source,"species":record.spec.species,"gene_id_type":record.spec.gene_id_type,"source_files":record.spec.source_files,"parameters":record.spec.parameters,"design":design,"details":details,"baseline":record.baseline,"execution_evidence":execution_evidence,"qc_details":qc_details});
                    checks::run(&handle, &self.journal, &record.language, &request).await
                }
                _ => checks::Measured {
                    run_id: None,
                    code_digest: None,
                    result: json!({"items":[],"facts":{"execution_error":"selected kernel is ended or disconnected; no alternate environment started"}}),
                },
            }
        } else {
            checks::Measured {
                run_id: None,
                code_digest: None,
                result: json!({"items":[],"facts":{"execution_error":"prerequisites not satisfied; kernel not executed"}}),
            }
        };
        let mut items = if record.language == "design" {
            serde_json::from_value(measured.result["items"].clone())?
        } else {
            checks::items(&measured.result, &record.language, checkpoint)
        };
        items.extend(prerequisites);
        if checkpoint == "input" && measured.result.get("baseline").is_none_or(Value::is_null) {
            items.push(checks::incomplete(
                "input_baseline",
                "checker could not establish a source-backed input baseline",
            ));
        }
        let after = snapshot(paths).await;
        if before
            .as_ref()
            .ok()
            .map(|v| serde_json::to_value(v).unwrap_or_default())
            != after
                .as_ref()
                .ok()
                .map(|v| serde_json::to_value(v).unwrap_or_default())
            || after.is_err()
        {
            items.push(checks::incomplete(
                "file_freshness",
                "a checked file changed or became unreadable during the checkpoint",
            ));
        }
        let technical_pass = !items.is_empty()
            && items
                .iter()
                .all(|i| matches!(i.status, CheckStatus::Pass | CheckStatus::Warning));
        if checkpoint == "input" {
            record.baseline = if technical_pass {
                measured
                    .result
                    .get("baseline")
                    .cloned()
                    .unwrap_or(Value::Null)
            } else {
                Value::Null
            };
        }
        let session_evidence = match record.spec.session.as_deref() {
            Some(session) => {
                json!({"session":session,"latest_run":measured.run_id,"language":record.language})
            }
            None => Value::Null,
        };
        let check = AnalysisCheck {
            id: uuid::Uuid::new_v4().simple().to_string(),
            analysis: id.into(),
            checkpoint: checkpoint.into(),
            details,
            created_ms: now_ms(),
            checker_digest: record.spec.checker_digest.clone(),
            guide_digest: record.spec.guide_digest.clone(),
            run_id: measured.run_id,
            code_digest: measured.code_digest,
            result_digest: checks::hash(&serde_json::to_vec(&measured.result)?),
            items,
            facts: measured.result["facts"].clone(),
            files: after.unwrap_or_default(),
            session_evidence,
            technical_pass,
        };
        self.journal.save_analysis(&record, Some(&check))?;
        self.publish_analysis(record, Some(&check)).await
    }

    fn owned_analysis(&self, root: &str, id: &str) -> Result<AnalysisRecord> {
        let root = project(root)?;
        let record = self.journal.analysis(id)?.context("analysis not found in authoritative journal (project exports are historical files only)")?;
        if record.project != root {
            bail!("analysis belongs to another project");
        }
        Ok(record)
    }

    pub async fn get_analysis(&self, root: &str, id: &str) -> Result<Value> {
        let _guard = self.analysis_gate.lock().await;
        let record = self.owned_analysis(root, id)?;
        self.analysis_response(&record, None, None)
    }

    async fn publish_analysis(
        &self,
        mut record: AnalysisRecord,
        check: Option<&AnalysisCheck>,
    ) -> Result<Value> {
        let all_checks = self.journal.analysis_checks(&record.id)?;
        let journal = self.journal.clone();
        let exported = tokio::task::spawn_blocking(move || {
            let result = export(&journal, &mut record, &all_checks);
            (record, result)
        })
        .await?;
        let (record, result) = exported;
        // Persist successful individual publications even when a later export failed.
        self.journal.save_analysis(&record, None)?;
        self.analysis_response(&record, check, Some(result.err().map(|e| format!("{e:#}"))))
    }

    fn analysis_response(
        &self,
        record: &AnalysisRecord,
        fresh: Option<&AnalysisCheck>,
        export: Option<Option<String>>,
    ) -> Result<Value> {
        let checks = self.journal.analysis_checks(&record.id)?;
        let mut changes = Vec::new();
        if current_contract(record).is_err() {
            changes.push("guide or checker digest changed");
        }
        if let Some(last) = checks.last()
            && let Some(session) = record.spec.session.as_deref()
        {
            if self.journal.latest_session_run(session)?
                != last.session_evidence["latest_run"]
                    .as_str()
                    .map(str::to_owned)
            {
                changes.push("kernel ran additional code after checkpoint");
            }
            if !self
                .sessions
                .lock()
                .unwrap()
                .get(session)
                .is_some_and(|s| s.is_running())
            {
                changes.push("selected kernel ended or disconnected");
            }
        }
        let history: Vec<_> = checks.iter().rev().take(100).map(|c|json!({"id":c.id,"checkpoint":c.checkpoint,"created_ms":c.created_ms,"run_id":c.run_id,"technical_pass":c.technical_pass,"result_digest":c.result_digest})).collect();
        let mut manifest = serde_json::to_value(record)?;
        manifest.as_object_mut().unwrap().remove("baseline");
        manifest.as_object_mut().unwrap().remove("exports");
        manifest.as_object_mut().unwrap().remove("pending_exports");
        let mut latest = fresh
            .or_else(|| checks.last())
            .map(serde_json::to_value)
            .transpose()?;
        if let Some(check) = latest.as_mut()
            && serde_json::to_vec(check)?.len() > 512 * 1024
        {
            check["facts"] = json!({"bounded_summary":true,"full_evidence":"authoritative journal and checks/<id>.json"});
            if let Some(items) = check["items"].as_array_mut() {
                items.truncate(256);
                for item in items {
                    item["evidence"] = Value::Null;
                    if let Some(message) = item["message"].as_str() {
                        item["message"] = json!(message.chars().take(2048).collect::<String>());
                    }
                }
            }
        }
        let current = fresh.is_some_and(|c| c.technical_pass) && changes.is_empty();
        let files_reverified = fresh.is_some_and(|c| {
            !c.items.iter().any(|i| {
                matches!(
                    i.id.as_str(),
                    "file_evidence" | "file_freshness" | "input_freshness"
                )
            })
        });
        let result = json!({"analysis":manifest,"latest_check":latest,"history":history,"history_count":checks.len(),"validity":{"current":current,"state":if current{"checked_at_this_request"}else if fresh.is_some(){"failed_or_incomplete"}else{"historical_not_reverified"},"known_changes":changes,"files_reverified":files_reverified,"scientific_validity":"awaiting_human_review"},"export":match export {Some(None)=>json!({"saved":true}),Some(Some(error))=>json!({"saved":false,"error":error,"recovery":"authoritative record retained; next check retries exports without importing project files"}),None=>json!({"saved":null,"state":"not_reverified"})}});
        if serde_json::to_vec(&result)?.len() > checks::MAX_RESULT {
            bail!(
                "analysis response exceeds 1MiB; evidence remains in journal and managed check export"
            );
        }
        Ok(result)
    }
}

/// Open each directory component with O_NOFOLLOW; hold its descriptor through writes.
fn directory(path: &Path, create: bool) -> Result<File> {
    let mut dir = File::open("/")?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let name = std::ffi::CString::new(name.as_encoded_bytes())?;
        if create {
            let result = unsafe { libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o700) };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        let fd = unsafe {
            libc::openat(
                dir.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error())
                .context("managed output ancestor is linked or inaccessible");
        }
        dir = unsafe { File::from_raw_fd(fd) };
    }
    Ok(dir)
}

fn export(
    journal: &crate::journal::Journal,
    record: &mut AnalysisRecord,
    checks: &[AnalysisCheck],
) -> Result<()> {
    let root = Path::new(&record.project)
        .join(".sandhill/analysis")
        .join(&record.id);
    let mut protected: Vec<String> = record.input_files.iter().map(|f| f.path.clone()).collect();
    protected.extend(record.spec.documents.clone());
    for check in checks {
        protected.extend(check.files.iter().map(|f| f.path.clone()));
    }
    reject_managed_aliases(record, &protected, checks)?;
    let _ = directory(&root, true)?;
    let mut manifest = serde_json::to_value(&*record)?;
    manifest.as_object_mut().unwrap().remove("baseline");
    manifest.as_object_mut().unwrap().remove("exports");
    manifest.as_object_mut().unwrap().remove("pending_exports");
    publish(
        record,
        journal,
        &root,
        "manifest.json",
        &serde_json::to_vec_pretty(&manifest)?,
        false,
    )?;
    if record.language == "design" {
        let bytes = serde_json::to_vec_pretty(&record.spec.design)?;
        publish(record, journal, &root, "study-design.json", &bytes, false)?;
    }
    for check in checks {
        publish(
            record,
            journal,
            &root,
            &format!("checks/{}.json", check.id),
            &serde_json::to_vec_pretty(check)?,
            false,
        )?;
    }
    let mut report = format!(
        "# Sandhill analysis {}\n\nAuthoritative state: daemon journal. This export is historical, not a current-validity certificate.\n\nScientific interpretation, design, QC tradeoffs and annotation require independent human review.\n\nGuide: {} (not human-reviewed). Workflow: {:?}.\n",
        record.id, record.spec.guide_id, record.spec.workflow
    );
    for check in checks {
        report.push_str(&format!(
            "\n## {} — {}\n\nTime: {}. Technical requirements satisfied: {}. Run: {}.\n",
            check.checkpoint,
            check.id,
            check.created_ms,
            check.technical_pass,
            check
                .run_id
                .as_deref()
                .unwrap_or("none (no kernel execution)")
        ));
        for item in &check.items {
            report.push_str(&format!(
                "- {:?}: {} — {}\n",
                item.status,
                item.id,
                item.message.replace('\n', " ")
            ));
        }
    }
    publish(record, journal, &root, "report.md", report.as_bytes(), true)
}

fn publish(
    record: &mut AnalysisRecord,
    journal: &crate::journal::Journal,
    root: &Path,
    relative: &str,
    bytes: &[u8],
    mutable: bool,
) -> Result<()> {
    let relative_path = Path::new(relative);
    let parent = root.join(relative_path.parent().unwrap_or(Path::new("")));
    let dir = directory(&parent, true)?;
    let name = std::ffi::CString::new(
        relative_path
            .file_name()
            .context("export filename")?
            .as_encoded_bytes(),
    )?;
    let read_fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
        )
    };
    let digest = checks::hash(bytes);
    let mut previous = None;
    if read_fd >= 0 {
        let mut existing = unsafe { File::from_raw_fd(read_fd) };
        let meta = existing.metadata()?;
        if !meta.is_file() || meta.nlink() != 1 {
            bail!("linked managed export preserved: {relative}");
        }
        for input in &record.input_files {
            let source = std::fs::metadata(&input.path)?;
            if (source.dev(), source.ino()) == (meta.dev(), meta.ino()) {
                bail!("managed export aliases an input: {relative}");
            }
        }
        if meta.len() > 16 * 1024 * 1024 {
            bail!("oversized existing export preserved: {relative}");
        }
        let mut old = Vec::new();
        existing.read_to_end(&mut old)?;
        let old_digest = checks::hash(&old);
        if old_digest == digest {
            record.exports.insert(relative.into(), digest);
            record.pending_exports.remove(relative);
            journal.save_analysis(record, None)?;
            return Ok(());
        }
        if !mutable
            || (record.exports.get(relative) != Some(&old_digest)
                && record.pending_exports.get(relative) != Some(&old_digest))
        {
            bail!("existing or manually changed export preserved: {relative}");
        }
        // Promote only a digest authorized by the durable publication intent.
        record.exports.insert(relative.into(), old_digest.clone());
        previous = Some((existing, meta, old_digest));
    } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::NotFound {
        return Err(std::io::Error::last_os_error())
            .context("linked/inaccessible export preserved");
    }
    record
        .pending_exports
        .insert(relative.into(), digest.clone());
    journal.save_analysis(record, None)?;
    let temporary = format!(".{}.tmp", uuid::Uuid::new_v4().simple());
    let tmp = std::ffi::CString::new(temporary)?;
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            tmp.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut file = unsafe { File::from_raw_fd(fd) };
    let written = (|| -> Result<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        let status = if let Some((existing, meta, old_digest)) = previous.as_mut() {
            revalidate_report(&dir, &name, existing, meta, old_digest)?;
            // POSIX has no compare-content-and-rename operation. This final
            // descriptor/content check detects edits through this point, not
            // an adversarial write in the remaining rename syscall window.
            unsafe {
                libc::renameat(
                    dir.as_raw_fd(),
                    tmp.as_ptr(),
                    dir.as_raw_fd(),
                    name.as_ptr(),
                )
            }
        } else {
            unsafe {
                libc::linkat(
                    dir.as_raw_fd(),
                    tmp.as_ptr(),
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    0,
                )
            }
        };
        if status < 0 {
            return Err(std::io::Error::last_os_error())
                .context("atomic export publication failed");
        }
        unsafe { libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0) };
        dir.sync_all()?;
        Ok(())
    })();
    unsafe { libc::unlinkat(dir.as_raw_fd(), tmp.as_ptr(), 0) };
    written?;
    record.exports.insert(relative.into(), digest);
    record.pending_exports.remove(relative);
    journal.save_analysis(record, None)?;
    Ok(())
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::{blobs::BlobStore, journal::Journal};

    fn record(root: &Path) -> AnalysisRecord {
        serde_json::from_value(json!({
            "format_version":1,"id":"owned","project":root,"created_ms":0,"language":"design",
            "spec":{"workflow":"study_design","guide_id":"bio-study-design","guide_digest":"test",
                "checker_digest":"test","goal":"test","design":{},"parameters":{}},
            "input_files":[],"baseline":null,"exports":{}
        }))
        .unwrap()
    }

    #[test]
    fn report_revalidation_detects_in_place_edit_without_inode_change() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let path = root.path().join("report.md");
        std::fs::write(&path, b"original").unwrap();
        let mut opened = File::open(&path).unwrap();
        let expected = opened.metadata().unwrap();
        let digest = checks::hash(b"original");
        // Equal length and the same inode are not evidence of unchanged content.
        std::fs::write(&path, b"modified").unwrap();
        assert_eq!(opened.metadata().unwrap().ino(), expected.ino());
        let dir = directory(root.path(), false).unwrap();
        let name = std::ffi::CString::new("report.md").unwrap();
        assert!(revalidate_report(&dir, &name, &mut opened, &expected, &digest).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"modified");
    }

    #[test]
    fn journaled_publication_intent_recovers_post_rename_crash_without_trusting_edits() {
        let state = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let db = state.path().join("journal.sqlite");
        let mut record = record(root.path());
        {
            let journal = Journal::open(&db, BlobStore::new(state.path().join("blobs"))).unwrap();
            publish(
                &mut record,
                &journal,
                root.path(),
                "report.md",
                b"first",
                true,
            )
            .unwrap();
            // Exact durable/FS state after publication but before ownership commit.
            record
                .pending_exports
                .insert("report.md".into(), checks::hash(b"second"));
            journal.save_analysis(&record, None).unwrap();
            std::fs::write(root.path().join("report.md"), b"second").unwrap();
        }
        let journal = Journal::open(&db, BlobStore::new(state.path().join("blobs"))).unwrap();
        let mut recovered = journal.analysis("owned").unwrap().unwrap();
        publish(
            &mut recovered,
            &journal,
            root.path(),
            "report.md",
            b"third",
            true,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("report.md")).unwrap(),
            b"third"
        );
        assert!(
            journal
                .analysis("owned")
                .unwrap()
                .unwrap()
                .pending_exports
                .is_empty()
        );
        std::fs::write(root.path().join("report.md"), b"user edit").unwrap();
        assert!(
            publish(
                &mut recovered,
                &journal,
                root.path(),
                "report.md",
                b"fourth",
                true
            )
            .is_err()
        );
        assert_eq!(
            std::fs::read(root.path().join("report.md")).unwrap(),
            b"user edit"
        );
    }
}
