---
name: bio-study-design
description: Develop a falsifiable biological study question, evidence map, experimental and statistical design, validation plan, and explicit unresolved decisions before collecting or analyzing data.
license: GPL-3.0-or-later
metadata:
  author: Sandhill contributors (AI-assisted original text)
  revision: "2026-11-10.1"
  review-status: awaiting_human_domain_review
  reviewer: none
  review-date: none
---

# Biological study design

**Awaiting human domain review. Not a default recommended guide.** Huan or a designated domain reviewer must review this guide and each project's scientific choices. AI self-checks and multiple-model agreement are not independent domain review.

## Scope and prerequisites

Use for a complete early-stage project design, including computational reanalysis. Not an ethics approval, clinical recommendation, grant novelty certificate, power calculation service, or proof of causality. No kernel is needed for the `study_design` workflow. Obtain consent/access approvals and identify the responsible scientist before handling restricted data. Do not upload identifiable records or complete matrices to model prompts. Existing agents may send summaries to their model provider; this is not an offline guarantee.

## Questions before drafting

Ask for the biological question, species/system, relevant prior evidence, proposed intervention/exposure and comparator, experimental unit, sample/donor/litter/batch structure, pairing, primary outcome and meaningful effect, feasible resources, sources of bias, independent validation material, ethics/data constraints, and decisions still unknown. Look in project files first; never invent answers to make a form pass. Distinguish individual cells, technical replicates, biological replicates, and units randomized to treatment.

## Workflow

1. Write the question and operational primary outcome. Separate what was observed, cited, assumed, and hypothesized. Search primary sources; retain search date, URL/DOI/PMID, access status, the exact claim supported, and limitations. A plausible citation string is not verified evidence. If retrieval fails, label it unverified; do not claim novelty from an incomplete search.
2. State a falsifiable hypothesis and a competing explanation. Define what result would refute or materially weaken it. Specify target population, experimental unit, controls, groups, pairing, inclusion/exclusion rules, randomization and blinding where applicable, before seeing outcomes.
3. Map confounding and measurement: donor, sex, age, processing day, sequencing run, library chemistry, and tissue handling may be aligned with group. A group fully confounded with batch cannot be rescued by blindly integrating it. Define QC exclusions independent of the desired result.
4. Select an analysis appropriate to the experimental unit and outcome. State estimand, model, covariates, multiplicity family, missing-data handling and sensitivity analyses. Separate confirmatory tests from exploratory cluster markers. Cells from one donor are not independent donor replicates.
5. Justify sample size using defensible variance/effect estimates, precision or power targets, attrition, resource constraints and assumptions. If these are unavailable, mark sample size unresolved and propose a pilot; never invent power or a universal minimum donor count.
6. Plan independent validation, failure/stop rules, interpretation limits, reproducibility and a human decision meeting. Include negative or ambiguous outcomes. Do not promise all downstream questions can be answered by the planned assay.
7. Draft the complete project artifact using [references/study-design.md](references/study-design.md). Preserve open questions in `unresolved_questions`; meaningful unknowns remain unresolved even if structural fields are present.
8. Fetch this guide's current digest/checker digest with `get_bio_guide`. Create the analysis via `begin_analysis` with `workflow: "study_design"`, this guide ID, digests, goal, absolute source/document paths, and `design`. Call `check_analysis` at `design` with `details: {}`; the backend checks its authoritative declared design, without executing a kernel. Read the program report using `get_analysis` and correct actual omissions. Never write a pass report yourself.

## Checkpoints and stop conditions

The design checkpoint checks required structure, source identifiers, experimental unit, replication information, validation and stopping rules. It cannot verify source truth, adequate power, novelty, causal identification or biological feasibility. Unknown placeholders do not meet requirements. If the treatment comparison lacks independent biological replication or is inseparable from batch, report the unresolved design defect; restrict any reachable work to description/exploration. Stop confirmatory inference until a human approves a suitable design.

## Claims not permitted

No “scientifically validated” badge from a complete JSON file; no causal conclusion from association alone; no treatment/disease inference from single-donor PBMC3k; no ethics approval inferred from dataset availability; no fabricated sources, power, replication or verification. Do not install software, create environments, download data, or start an analysis merely because this guide was read. Ask/Plan can read guides/history, not create analysis records or execute new checks.

## Sources, revision and authorship

Original Sandhill guidance, revision **2026-11-10.1**, author **Sandhill contributors (AI-assisted)**. Human reviewer/date: **none; awaiting human domain review**. References: [ARRIVE 2.0](https://arriveguidelines.org/arrive-guidelines) (animal-study reporting/design considerations, not a universal human-study protocol); [Squair et al., 2021](https://doi.org/10.1038/s41467-021-25960-2) (replication and false discoveries in single-cell comparisons); [Agent Skills format](https://agentskills.io/specification) (packaging only). Sources do not endorse this guide. Project-specific citations must be independently checked.
