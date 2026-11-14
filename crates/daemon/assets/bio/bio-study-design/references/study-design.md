# Project design artifact and tool contract

Revision 2026-11-10.1. Author: Sandhill contributors (AI-assisted). **Awaiting human domain review; reviewer/date: none.** This is original guidance, not an approved protocol. Sources: ARRIVE 2.0, https://arriveguidelines.org/arrive-guidelines; Squair et al., https://doi.org/10.1038/s41467-021-25960-2.

## Complete project record

Keep a visible narrative in a project document. Record question, motivation, known evidence and source verification, falsifiable hypothesis and alternatives, experimental unit, population and recruitment, intervention/control, grouping/pairing, randomization/blinding, confounders, outcome and meaningful effect, analysis, biological versus technical replication, sample-size assumptions, exclusions/missingness, multiplicity, validation, stopping rules, reproducibility, ethics and unresolved decisions. The backend's required keys are a structural minimum, not a complete scientific evaluation. Use additional keys for the extra narrative; do not silently omit a limitation because there is no required field for it.

The following is a complete **illustrative declared design for a descriptive tutorial**, not a new biological experiment, a verified literature review or evidence that this analysis ran. Its deliberate limitations cannot be repaired by checking the file:

```json
{
  "research_question": "Which broad PBMC populations can be provisionally described in the existing PBMC3k tutorial dataset?",
  "evidence": "The official Seurat PBMC3k tutorial describes 2700 PBMC cells; this is a tutorial reference, not independent validation of our annotations.",
  "hypothesis": "Several broad immune lineages will show concordant multi-marker expression. If coherent lineage evidence is absent, retain unknown rather than force labels.",
  "experimental_unit": "One donor; cells are nested observations, not donor replicates.",
  "groups": "Descriptive unsupervised clusters only; no intervention or disease comparison.",
  "pairing": "Not applicable: no paired experimental comparison is proposed.",
  "confounders": "Single-donor sampling, capture efficiency, ambient RNA, doublets and technical batch limit interpretation; donor and batch variation cannot be estimated here.",
  "primary_outcome": "A provisional broad-lineage annotation table with expression summaries, conflicts, sources and uncertainty for every cluster.",
  "analysis_method": "Recorded QC, log-normalization, variable-feature selection, sparse-aware PCA, neighbor graph, exploratory clustering/UMAP and adjusted cluster marker tests; no treatment inference.",
  "sample_size_rationale": "Existing fixed tutorial sample; no prospective power claim or population-level effect estimate is made.",
  "validation": "Human domain review of multi-marker evidence and conflicts; independent donor/protein validation is unavailable and remains unresolved.",
  "stopping_rules": "Stop the raw-count workflow if provenance is unknown or counts are altered; stop annotation claims when marker evidence conflicts; stop all treatment/disease inference because there is no comparison design.",
  "sources": [{"url": "https://satijalab.org/seurat/articles/pbmc3k_tutorial", "claim": "Tutorial input description and example PBMC markers", "verification": "Official page accessed 2026-11-10; does not verify this project's outputs"}],
  "unresolved_questions": ["Independent biological validation is not available", "A human domain reviewer must approve QC and provisional annotations"],
  "biological_replicates": "One healthy donor; no independent donor replication for group inference.",
  "randomization_blinding": "No randomized intervention; reviewer should inspect evidence without copying tutorial cluster-number labels.",
  "ethics_access": "Use only already-authorized public tutorial files; public availability does not establish permission for unrelated restricted datasets.",
  "reproducibility": "Visible code, input hashes, software versions, seeds, parameters, object files and checkpoint history are required."
}
```

For a real intervention project, replace the scientific content with actual project facts, not copied tutorial answers. If sample size, replication, pairing or source support is unknown, preserve that in unresolved questions and do not call the design adequate. The structural checker may reject unknown required fields; that is a useful unresolved result, not a reason to fabricate values.

## Ready-to-use invocation pattern

The notation below uses tool-result variables in agent pseudocode, not literal JSON placeholders. `project` is the current project's actual absolute root, `documents` contains existing project document paths, and `design` is the reviewed draft object above or the actual project object. Obtain these from project context rather than asking for information already available.

```text
catalog = get_bio_guide({})
guide = get_bio_guide({"id":"bio-study-design"})
spec = {
  "workflow":"study_design",
  "guide_id":"bio-study-design",
  "guide_digest":guide.digest,
  "checker_digest":guide.checker_digest,
  "goal":design.research_question,
  "source_files":[],
  "documents":documents,
  "design":design,
  "parameters":{}
}
record = begin_analysis({"project":project,"spec":spec}).analysis
check_analysis({"project":project,"analysis":record.id,"checkpoint":"design","details":{}})
get_analysis({"project":project,"analysis":record.id})
```

The response contains `{analysis:{id,...},latest_check,...}`; bind its nested `analysis` record as shown, then pass `record.id`. Do not guess an ID or use an export path as one. `source_files` is empty only for a design with no local source files; otherwise list actual absolute source paths. Sources in `design.sources` need URL, DOI or PMID identifiers and a claim/access note. A structural pass is not citation verification or design sufficiency. Changes require a new check; historical exports cannot be imported as authoritative pass evidence. No kernel, environment creation or download is part of this workflow.
