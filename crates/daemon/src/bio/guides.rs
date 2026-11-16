//! Read-only, bounded discovery of original embedded bioinformatics guides.

use std::sync::OnceLock;

use anyhow::{Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const REVISION: &str = "2026-11-10.1";
const REVIEW_STATUS: &str = "awaiting_human_domain_review";
const MAX_RESOURCE_BYTES: usize = 128 * 1024;
const MAX_GUIDE_BYTES: usize = 512 * 1024;

struct Resource {
    name: &'static str,
    content: &'static str,
}

struct Guide {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    checker_language: Option<&'static str>,
    resources: &'static [Resource],
    digest: OnceLock<String>,
}

static GUIDES: [Guide; 4] = [
    Guide {
        id: "bio-study-design",
        name: "Biological study design",
        description: "Falsifiable questions, evidence, experimental units, replication, analysis, validation and stopping rules.",
        checker_language: Some("design"),
        resources: &[
            Resource {
                name: "SKILL.md",
                content: include_str!("../../assets/bio/bio-study-design/SKILL.md"),
            },
            Resource {
                name: "references/study-design.md",
                content: include_str!(
                    "../../assets/bio/bio-study-design/references/study-design.md"
                ),
            },
            Resource {
                name: "references/checkpoints.md",
                content: include_str!(
                    "../../assets/bio/bio-results-review/references/checkpoints.md"
                ),
            },
        ],
        digest: OnceLock::new(),
    },
    Guide {
        id: "scrna-scanpy",
        name: "Python scRNA-seq with omicverse first",
        description: "Exploratory raw-count AnnData workflow prioritizing omicverse 2.3.2, with Scanpy for explicit gaps.",
        checker_language: Some("python"),
        resources: &[
            Resource {
                name: "SKILL.md",
                content: include_str!("../../assets/bio/scrna-scanpy/SKILL.md"),
            },
            Resource {
                name: "references/workflow.md",
                content: include_str!("../../assets/bio/scrna-scanpy/references/workflow.md"),
            },
            Resource {
                name: "references/pbmc-markers.md",
                content: include_str!(
                    "../../assets/bio/bio-results-review/references/pbmc-markers.md"
                ),
            },
            Resource {
                name: "references/checkpoints.md",
                content: include_str!(
                    "../../assets/bio/bio-results-review/references/checkpoints.md"
                ),
            },
        ],
        digest: OnceLock::new(),
    },
    Guide {
        id: "scrna-seurat",
        name: "R scRNA-seq with Seurat",
        description: "Exploratory raw-count Seurat workflow with explicit assay/layers, QC, reproducibility and provisional annotations.",
        checker_language: Some("r"),
        resources: &[
            Resource {
                name: "SKILL.md",
                content: include_str!("../../assets/bio/scrna-seurat/SKILL.md"),
            },
            Resource {
                name: "references/workflow.md",
                content: include_str!("../../assets/bio/scrna-seurat/references/workflow.md"),
            },
            Resource {
                name: "references/pbmc-markers.md",
                content: include_str!(
                    "../../assets/bio/bio-results-review/references/pbmc-markers.md"
                ),
            },
            Resource {
                name: "references/checkpoints.md",
                content: include_str!(
                    "../../assets/bio/bio-results-review/references/checkpoints.md"
                ),
            },
        ],
        digest: OnceLock::new(),
    },
    Guide {
        id: "bio-results-review",
        name: "Biological results review",
        description: "Separate execution, technical checks, statistical applicability and human biological interpretation.",
        checker_language: None,
        resources: &[
            Resource {
                name: "SKILL.md",
                content: include_str!("../../assets/bio/bio-results-review/SKILL.md"),
            },
            Resource {
                name: "references/pbmc-markers.md",
                content: include_str!(
                    "../../assets/bio/bio-results-review/references/pbmc-markers.md"
                ),
            },
            Resource {
                name: "references/checkpoints.md",
                content: include_str!(
                    "../../assets/bio/bio-results-review/references/checkpoints.md"
                ),
            },
        ],
        digest: OnceLock::new(),
    },
];

fn find(id: &str) -> Result<&'static Guide> {
    GUIDES
        .iter()
        .find(|guide| guide.id == id)
        .ok_or_else(|| anyhow::anyhow!("unknown bio guide ID: {id}"))
}

fn validate_bounds(guide: &Guide) -> Result<()> {
    let mut total = 0;
    for resource in guide.resources {
        if resource.content.len() > MAX_RESOURCE_BYTES {
            bail!("embedded bio guide resource exceeds size bound");
        }
        total += resource.content.len();
    }
    if total > MAX_GUIDE_BYTES {
        bail!("embedded bio guide exceeds size bound");
    }
    Ok(())
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value);
}

fn content_digest(guide: &Guide) -> &str {
    guide.digest.get_or_init(|| {
        let mut hasher = Sha256::new();
        hasher.update(b"sandhill-bio-guide-v1\0");
        hash_field(&mut hasher, guide.id.as_bytes());
        // Fixed order is part of the digest contract. References are covered,
        // not just SKILL.md; names are framed too, preventing concatenation aliases.
        for resource in guide.resources {
            hash_field(&mut hasher, resource.name.as_bytes());
            hash_field(&mut hasher, resource.content.as_bytes());
        }
        format!("sha256:{}", sandhill_proto::hex(&hasher.finalize()))
    })
}

fn metadata(guide: &Guide) -> Value {
    json!({
        "id": guide.id,
        "name": guide.name,
        "description": guide.description,
        "version": REVISION,
        "author": "Sandhill contributors (AI-assisted original text)",
        "review_status": REVIEW_STATUS,
        "reviewer": null,
        "review_date": null,
        "recommended": false,
        "digest": content_digest(guide),
        "digest_algorithm": "sha256-length-framed-resources-v1",
        "checker_digest": guide.checker_language.map(super::checks::digest),
        "checker_language": guide.checker_language,
        "resources": guide.resources.iter().map(|resource| resource.name).collect::<Vec<_>>(),
    })
}

/// Guide IDs and titles without loading or hashing their resources.
pub fn titles() -> impl Iterator<Item = (&'static str, &'static str)> {
    GUIDES.iter().map(|guide| (guide.id, guide.name))
}

pub(crate) fn skills() -> impl Iterator<Item = sandhill_proto::BuiltInSkill> {
    GUIDES.iter().map(|guide| sandhill_proto::BuiltInSkill {
        id: guide.id.into(),
        name: guide.name.into(),
        description: guide.description.into(),
    })
}

/// Catalog, main skill, or one exactly allowlisted relative resource.
/// No filesystem access, kernel execution, data download or user configuration.
pub fn get(id: Option<&str>, resource: Option<&str>) -> Result<Value> {
    let Some(id) = id else {
        if resource.is_some() {
            bail!("a resource requires an explicit bio guide ID");
        }
        let mut guides = Vec::with_capacity(GUIDES.len());
        for guide in &GUIDES {
            validate_bounds(guide)?;
            guides.push(metadata(guide));
        }
        return Ok(json!({"guides": guides, "recommended_guide": null}));
    };
    let guide = find(id)?;
    validate_bounds(guide)?;
    let name = resource.unwrap_or("SKILL.md");
    let Some(resource) = guide
        .resources
        .iter()
        .find(|resource| resource.name == name)
    else {
        bail!("resource is not allowlisted for bio guide {id}: {name}");
    };
    let mut value = metadata(guide);
    value["resource"] = json!(resource.name);
    value["content"] = json!(resource.content);
    Ok(value)
}

/// Digest of the guide ID and every embedded resource name and content byte.
pub fn digest(id: &str) -> Result<String> {
    let guide = find(id)?;
    validate_bounds(guide)?;
    Ok(content_digest(guide).to_owned())
}
