use std::path::Path;

use crate::kernelspecs::Search;
use sandhill_proto::{InstallTarget, InstallTool};

/// Import spelling, PyPI name, and conda name. R needs no translation.
const PYTHON_NAMES: &[(&str, &str, &str)] = &[
    ("sklearn", "scikit-learn", "scikit-learn"),
    ("cv2", "opencv-python", "opencv"),
    ("PIL", "pillow", "pillow"),
    ("yaml", "pyyaml", "pyyaml"),
    ("bs4", "beautifulsoup4", "beautifulsoup4"),
    ("skimage", "scikit-image", "scikit-image"),
    ("Bio", "biopython", "biopython"),
    ("umap", "umap-learn", "umap-learn"),
    ("igraph", "igraph", "python-igraph"),
    ("leidenalg", "leidenalg", "leidenalg"),
    ("louvain", "louvain", "louvain"),
    ("community", "python-louvain", "python-louvain"),
    ("dateutil", "python-dateutil", "python-dateutil"),
    ("attr", "attrs", "attrs"),
    ("Crypto", "pycryptodome", "pycryptodome"),
    ("docx", "python-docx", "python-docx"),
    ("pptx", "python-pptx", "python-pptx"),
    ("magic", "python-magic", "python-magic"),
    ("tables", "tables", "pytables"),
    ("torch", "torch", "pytorch"),
    ("harmonypy", "harmonypy", "harmonypy"),
    ("scvi", "scvi-tools", "scvi-tools"),
    ("decoupler", "decoupler", "decoupler-py"),
    ("gseapy", "gseapy", "gseapy"),
    ("anndata", "anndata", "anndata"),
    ("scanpy", "scanpy", "scanpy"),
    ("muon", "muon", "muon"),
    ("mudata", "mudata", "mudata"),
    ("squidpy", "squidpy", "squidpy"),
    ("pysam", "pysam", "pysam"),
    ("HTSeq", "HTSeq", "htseq"),
];

pub(crate) fn import_name<'a>(import: &'a str, target: &InstallTarget, search: &Search) -> &'a str {
    let InstallTarget::Python { prefix, .. } = target else {
        return import;
    };
    let conda = Path::new(prefix).join("conda-meta").is_dir()
        && super::install::conda(search, Path::new(prefix)).is_some();
    PYTHON_NAMES
        .iter()
        .find(|&&(name, _, _)| name == import)
        .map_or(import, |&(_, pypi, forge)| if conda { forge } else { pypi })
}

pub(super) fn module_distribution(module: &str) -> &str {
    PYTHON_NAMES
        .iter()
        .find(|&&(name, _, _)| name == module)
        .map_or(module, |&(_, pypi, _)| pypi)
}

/// Keep the same package when a plan is re-run with another source.
pub(super) fn distribution(name: &str, tool: InstallTool) -> &str {
    PYTHON_NAMES
        .iter()
        .find(|&&(_, pypi, conda)| {
            pypi.eq_ignore_ascii_case(name) || conda.eq_ignore_ascii_case(name)
        })
        .map_or(name, |&(_, pypi, conda)| {
            if tool == InstallTool::Conda {
                conda
            } else {
                pypi
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_follow_the_install_source_in_both_directions() {
        for (pypi, conda) in [
            ("opencv-python", "opencv"),
            ("igraph", "python-igraph"),
            ("tables", "pytables"),
            ("torch", "pytorch"),
            ("decoupler", "decoupler-py"),
        ] {
            assert_eq!(distribution(pypi, InstallTool::Conda), conda);
            assert_eq!(distribution(conda, InstallTool::Pip), pypi);
        }
        let python = InstallTarget::Python {
            prefix: "/nonexistent".into(),
            python: "/nonexistent/bin/python".into(),
        };
        let r = InstallTarget::R {
            r: String::new(),
            library: String::new(),
        };
        let search = Search::from_env();
        assert_eq!(import_name("sklearn", &python, &search), "scikit-learn");
        assert_eq!(import_name("Bio", &python, &search), "biopython");
        assert_eq!(import_name("community", &python, &search), "python-louvain");
        assert_eq!(import_name("cv2", &python, &search), "opencv-python");
        assert_eq!(import_name("cv2", &r, &search), "cv2");
        assert_eq!(
            import_name("any_other_name", &python, &search),
            "any_other_name"
        );
    }
}
