import type { JSX } from "solid-js";
import { render } from "solid-js/web";
import { describe, expect, it } from "vitest";
import registry from "../../../docs/design/ui/icons/registry.json";
import { FileIcon, Icon, iconKind, LangIcon, ProviderIcon, V32_ICONS } from "./icons";
import { ICONS32 } from "./icons32";

function mount(el: () => JSX.Element) {
  const root = document.createElement("div");
  render(el, root);
  return root;
}

/** An SVG body as the DOM writes it back, so markup compares equal however it was written. */
function svgHtml(body: string) {
  const el = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  el.innerHTML = body;
  return el.innerHTML;
}

describe("V32.2 icons", () => {
  it("is the build of the design's registry", () => {
    expect(ICONS32.resolve).toEqual(registry.resolve);
    for (const id of Object.keys(ICONS32.bio)) expect(registry.icons).toHaveProperty(`file.${id}`);
    for (const id of Object.keys(ICONS32.folders)) expect(registry.icons).toHaveProperty(`folder.${id}`);
  });

  it("tells bioinformatics files apart, inside compression too", () => {
    const cases: [string, string][] = [
      ["reads_R1.fastq.gz", "fastq"],
      ["ref.fa", "fasta"],
      ["genome.fasta.bgz", "fasta"],
      ["sample.bam", "alignment"],
      ["calls.vcf.gz", "variants"],
      ["genes.gtf", "annotation"],
      ["peaks.bed", "intervals"],
      ["matrix.mtx.gz", "sparse-matrix"],
      ["pbmc.h5ad", "anndata"],
      ["seu.h5Seurat", "hdf5"],
      ["obj.rds", "r-object"],
      ["counts.tsv", "table"],
      ["Snakefile", "snakemake"],
      ["main.nf", "nextflow"],
      ["analysis.qmd", "quarto"],
      ["report.Rmd", "rmarkdown"],
      ["CLAUDE.md", "agent-notes"],
      ["README.md", "readme"],
      ["renv.lock", "lock"],
      ["backup.tar.gz", "archive"],
      ["data.gz", "archive"],
      ["notes", "file"],
      [".gitignore", "git"],
    ];
    for (const [name, kind] of cases) expect([name, iconKind(name)]).toEqual([name, kind]);
  });

  it("gives folders a kind by name only", () => {
    expect(iconKind("data", true)).toBe("data");
    expect(iconKind("figures", true)).toBe("figures");
    expect(iconKind(".sandhill", true)).toBe("sandhill");
    expect(iconKind("anything", true)).toBe("other");
  });

  it("draws file icons from the set", () => {
    const root = mount(() => <FileIcon name="x.fastq.gz" />);
    expect(root.querySelector("svg.fi")!.innerHTML).toBe(svgHtml(ICONS32.bio.fastq));
    const dir = mount(() => <FileIcon name="data" dir />);
    expect(dir.querySelector("svg.fi")!.innerHTML).toContain("var(--tv-cut)");
  });

  it("maps every V32.2 line icon id to a registry system icon", () => {
    for (const id of Object.keys(V32_ICONS)) {
      expect((registry.icons as Record<string, { kind: string }>)[id]?.kind).toBe("system");
    }
    const a = mount(() => <Icon name="run" />).innerHTML;
    const b = mount(() => <Icon name="play" />).innerHTML;
    expect(a).toBe(b);
  });

  it("uses Sandhill's chips for languages and agents", () => {
    expect(mount(() => <LangIcon language="python3" />).querySelector("svg.lang")!.innerHTML).toBe(svgHtml(ICONS32.lang.python));
    expect(mount(() => <LangIcon language="bash" />).querySelector("svg.lang")!.innerHTML).toBe(svgHtml(ICONS32.lang.shell));
    expect(mount(() => <LangIcon language="julia" />).textContent).toBe("ju");
    expect(mount(() => <ProviderIcon kind="claude" />).querySelector("svg.prov")!.innerHTML).toBe(svgHtml(ICONS32.agents.claude.body));
    expect(mount(() => <ProviderIcon kind="omp" />).querySelector("svg.prov")!.innerHTML).toBe(svgHtml(ICONS32.agents.omp.body));
    expect(mount(() => <ProviderIcon kind="other" />).querySelector("svg.prov")!.innerHTML).toBe(svgHtml(ICONS32.agents.generic.body));
  });
});
