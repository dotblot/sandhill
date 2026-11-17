# Connectors: databases and services every agent can query

Status: **waiting for Huan's approval** (written 2026-11-16). Nothing is
built yet. Follows `docs/design/done/skills-plan.md`.

## What Huan asked for
- Connectors like Claude Science's (2026-11-16), after skills; "all of
  them if possible" (screenshot: 20 Featured connectors).
- Per device, with Copy to… (Huan's answer for skills, kept here).

A **connector** is an MCP server: a program or web address that gives an
agent tools such as "search PubMed" or "look up a variant in gnomAD".

## What exists to connect to
Checked on 2026-11-16 by calling each address.

Claude Science's 20 Featured connectors are built into its app, as its
skills are; they are not published and are not copied. The same databases
are reachable through these, all needing no account:

| Connector | What it reaches | Kind | Licence of the server |
|---|---|---|---|
| **BioMCP** (GenomOncology) | MyGene, UniProt, GO, Reactome, OLS, MyVariant (dbSNP), ClinVar, gnomAD, GWAS Catalog, GTEx, Human Protein Atlas, STRING, InterPro, Complex Portal, ClinGen, CIViC, Open Targets, cBioPortal, ChEMBL, MyChem, OpenFDA, ClinicalTrials.gov, PubMed/PubTator/Europe PMC, NIH RePORTER, KEGG, WikiPathways, Monarch/HPO | a program on the device, v0.9.3, one file per platform | MIT |
| **BioContextAI Knowledgebase** | Ensembl, InterPro, PRIDE, Reactome, STRING, AlphaFold DB, Human Protein Atlas, PanglaoDB, bioRxiv/medRxiv, Europe PMC, Antibody Registry, OLS | a Python program run with `uvx`, 0.2.1 | Apache-2.0 |
| **Open Targets** | Open Targets Platform | web address of the Open Targets team | (service) |
| **PubMed** | PubMed | web address hosted by Anthropic | (service) |
| **bioRxiv** | bioRxiv, medRxiv | web address hosted by Anthropic | (service) |
| **Clinical Trials** | ClinicalTrials.gov | web address hosted by Anthropic | (service) |
| **ChEMBL** | ChEMBL | web address hosted by Anthropic | (service) |

The last four are the ones Anthropic lists for Claude Code in its
Apache-2.0 `bio-research` plugin; they answered without a sign-in.

How Claude Science's Featured list is covered by these:

| Claude Science connector | Covered |
|---|---|
| Genes & Ontologies, Variants, Clinical Genomics, Expression, Protein Annotation, Cancer Models, Drug Regulatory | yes (BioMCP; Open Targets) |
| Human Genetics | GWAS Catalog yes; FinnGen and BioBank Japan no |
| Genomes | Ensembl lookups yes (BioContextAI); VEP no |
| Structures & Interactions | AlphaFold, Complex Portal, PDB ids yes; EMDB, IntAct no |
| Omics Archives | PRIDE yes; GEO, ArrayExpress, MGnify, MetaboLights no |
| Chemistry | ChEMBL, MyChem yes; PubChem, ChEBI, Rhea, BindingDB no |
| Literature Graph | Europe PMC, PubMed yes; OpenAlex, arXiv no |
| Research Resources | Antibody Registry, NIH RePORTER yes; Grants.gov no |
| Regulation (JASPAR, UniBind), RNA (Rfam), BioMart, CellGuide, ZINC, Ketcher | no |

Services that need the user's own account (they answered "401
unauthorized"): Consensus, Synapse, Wiley Scholar Gateway, Owkin,
BioRender, Benchling (a company's own address). They need a sign-in in
the browser (OAuth), which is phase 2 below.

## Approach: one doorway through Sandhill's own tool server
Every agent already reaches Sandhill's tool server (`sandhilld mcp`).
Connectors go through it too, the way skills do, so one install works for
all seven agents and nothing is written into the agents' own settings.

- The daemon keeps the connections: it calls web addresses itself and
  starts a program connector on first use (one per device, shared by every
  conversation, stopped after 10 minutes unused).
- Agents get three tools instead of every connector's own tools, because
  the five web connectors alone describe 31 tools in about 175 KB, which
  would fill every conversation's context:
  - `list_connectors`: the connectors that are on, each with its tools'
    names and one-line descriptions (read-only).
  - `query_connector {connector, tool, arguments}`: runs a tool that its
    server marks read-only, on a connector from Sandhill's list. Read-only,
    so agents run it without asking, as Claude Science does for Featured
    connectors.
  - `call_connector {connector, tool, arguments}`: any other tool, or any
    tool of a connector the user added. Not read-only, so each agent's own
    approval card asks first. Refused in Plan mode.
  - A tool's full input description comes back with `list_connectors
    {connector}` for one connector.
- One instruction line for agents: "Databases and services the user
  connected in Sandhill are listed by list_connectors; prefer them to
  scraping websites."
- Calls time out after 60 s; replies over 1 MB are cut with a note.

Rejected: passing connectors to each agent in its own way (Claude Code's
`--mcp-config`, Codex's `-c mcp_servers`, `agy mcp add`, ACP sessions,
pi's `mcp.json`, Posit's defaults). Seven ways, some writing into the
agents' own settings, OpenScience ignores ACP servers, and every tool's
description would land in every conversation.

## What gets built

### Phase 1: connectors without a sign-in
**Store**, per device, in the state folder: `connectors/index.json` (name,
kind, address or command, on/off, added when, where from) and
`connectors/secrets.json` (0600; header values the user typed, such as an
API key). Secrets are never shown again in full, never sent to agents and
never copied by Copy to….

**Sandhill's list** (`crates/daemon/assets/connectors/catalog.json`), all
on once added, none added until the user presses Add:
- BioMCP: Add downloads `biomcp-<platform>.tar.gz` v0.9.3 from its GitHub
  release into `connectors/bin/`, checks the published SHA-256, and runs
  `biomcp mcp`.
- BioContextAI Knowledgebase: runs `uvx biocontext_kb==0.2.1` with
  `UV_PYTHON=3.12` and `MCP_ENVIRONMENT=DEVELOPMENT`. Needs `uv` on the
  device; without it the row says so and links to uv's install page.
- Open Targets, PubMed, bioRxiv, Clinical Trials, ChEMBL: their web
  addresses.

**Add Connector** menu: Web Address… (a URL, an optional header name and
value) and Command… (a command line and optional environment lines). A
card shows what will run or be called and, for a command, that it runs as
the user on this device; Add tests the connection and lists the tools
before saving, so a wrong address fails on the card.

**Welcome → Connectors** page, beside Skills, one device at a time:
- Added: name, what it reaches or its address, a switch, ⋯ menu (Tools,
  Test, Copy to…, Remove). Tools shows the tool list. A connector that
  fails shows the error in red on its row.
- Suggested: Sandhill's list not yet added, each with Add.
- Same look as the Skills page (switch, card, menus).

**Daemon calls**: `ListConnectors`, `AddConnector {spec}` (tests, then
saves), `SetConnectorEnabled`, `RemoveConnector`, `TestConnector`,
`ExportConnector` (for Copy to…, without secrets), `ConnectorTools
{name?}` and `CallConnector {name, tool, arguments, read_only}` (for the
tool server). Types in `crates/proto/src/connectors.rs`; added to
`docs/protocol.md`.

**MCP client in the daemon**: Streamable HTTP (JSON and event-stream
replies, `Mcp-Session-Id`) and stdio, protocol versions 2025-06-18 back to
2024-11-05, `initialize`, `tools/list`, `tools/call`. No new language or
runtime.

### Phase 2: sign-in connectors
For web addresses that answer 401 with MCP's OAuth discovery: the card's
Sign In opens the browser on the Mac, the Mac app catches the redirect on
a loopback address and hands the code to the device's daemon, which keeps
and refreshes the token (in `secrets.json`). Then Consensus, Synapse,
Wiley Scholar Gateway, Owkin and BioRender join Sandhill's list, and
Benchling takes the company's own address. Built only after phase 1 is
in use, as its own step.

## Not building
- Anything taken out of Claude Science.app.
- Connectors Sandhill would write itself for the uncovered databases
  (VEP, GEO, PubChem, JASPAR and the rest). Huan can ask later, as for
  skills.
- Per-project connectors; automatic updates of BioMCP or BioContextAI.

## Order of work (phase 1)
1. Daemon: MCP client, store, list, calls, the three tools, Restricted and
   Plan rules. Tests.
2. App: the Connectors page and cards.
3. Check on screen; then BioMCP and Open Targets queried from a real
   conversation.

## Tests
In `crates/daemon/tests/connectors.rs` (no kernel needed):
- A stand-in stdio server and a stand-in HTTP server (JSON and
  event-stream replies, a session id): added, listed, called, turned
  off, removed.
- `query_connector` refuses a tool not marked read-only and a connector
  the user added; `call_connector` refused in Plan mode.
- A server that hangs: the call ends at the timeout; a huge reply is cut.
- Secrets: kept 0600, never in `ListConnectors`, `ExportConnector` or
  any tool reply.
- BioMCP download: a bad checksum is refused (a local file stands in for
  GitHub).
- Copy to…: export from one state folder, add into another.
In the app, on screen in light and dark.

## Risks
- **The data's own terms.** The servers are open source, but each
  database has terms (KEGG is free only for academic use, some ClinVar or
  OpenFDA data have conditions). Each Suggested row links to its source's
  terms page; the user is responsible, as in Claude Science.
- **Anthropic-hosted addresses.** Anthropic publishes the four addresses
  for Claude Code without a sign-in, but Sandhill would also let Codex,
  Antigravity and the others use them. See the question below.
- **This plan assumes agents call `list_connectors` when a question needs
  a database.** The instruction line and the tool descriptions say so; if
  an agent ignores them, the user can name the connector in the message.
- **A command connector runs a program as the user.** The card says so
  and nothing runs before Add.
- **Rollback:** deleting `connectors/` on a device returns it to today;
  stopping the daemon stops every connector program.

## For Huan to decide
1. Offer Anthropic's four hosted addresses (PubMed, bioRxiv, Clinical
   Trials, ChEMBL) in Sandhill's list, for every agent, or leave them out
   (BioMCP and BioContextAI already reach PubMed, ClinicalTrials.gov,
   ChEMBL and bioRxiv, through the databases' own public services)?
2. Phase 1 now and sign-in connectors later, as planned?
