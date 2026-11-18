"""Run with AnnData 0.13 for the modern files; run with 0.7.8 for legacy.h5ad."""
from importlib.metadata import version
from pathlib import Path

import anndata as ad
import h5py
import numpy as np
import pandas as pd
from scipy import sparse


HERE = Path(__file__).resolve().parent


def make():
    matrix = np.zeros((61, 5), dtype=np.float32)
    matrix[0, 1], matrix[2, 0], matrix[60, 4] = 1, 2, 3
    labels = [f"type-{i:02}" for i in range(64)]
    obs = pd.DataFrame(
        {
            "population": pd.Categorical([labels[63], None] + labels[:59], categories=labels),
            "score": np.arange(61, dtype=np.float32) + 0.5,
            "kept": np.arange(61) % 2 == 0,
            "barcode": [f"béta-{i:02}" for i in range(61)],
        },
        index=pd.Index([f"cell-{i:02}" for i in range(61)], name="cell"),
    )
    var = pd.DataFrame({"rank": np.arange(5, dtype=np.int32)}, index=[f"g{i}" for i in range(5)])
    if version("anndata").startswith("0.7."):
        ad.AnnData(sparse.csr_matrix(matrix), obs=obs, var=var).write(HERE / "legacy.h5ad")
        return

    dataset = ad.AnnData(matrix, obs=obs, var=var)
    dataset.raw = dataset.copy()
    dataset = dataset[:, :4].copy()
    dataset.obsm["X_umap"] = np.zeros((61, 2), dtype=np.float32)
    dataset.obsm["X_latent"] = pd.DataFrame(np.zeros((61, 3)), index=dataset.obs_names,
                                            columns=["a", "b", "c"])
    dataset.varm["loadings"] = np.zeros((4, 6))
    dataset.obsp["neighbors"] = sparse.eye(61, format="csr")
    dataset.varp["pairs"] = np.zeros((4, 4))
    dataset.uns["params"] = {"method": "test"}
    dataset.uns["version"] = 4
    dataset.layers["counts"] = sparse.csc_matrix(dataset.X.astype(np.int16))
    dataset.layers["scaled"] = dataset.X.astype(np.float64)
    dataset.write_h5ad(HERE / "dense.h5ad", compression="gzip")

    for format_, convert in [("csr", sparse.csr_matrix), ("csc", sparse.csc_matrix)]:
        dataset = ad.AnnData(convert(matrix), obs=obs, var=var)
        dataset.write_h5ad(HERE / f"{format_}.h5ad", compression="gzip")

    ad.AnnData(shape=(0, 5)).write_h5ad(HERE / "empty.h5ad")

    # Unreadable chunks prove that the preview never loads sparse data, reads
    # only the last indptr, and doesn't read table rows past the sample.
    dataset = ad.AnnData(sparse.csr_matrix(matrix), obs=obs, var=var)
    dataset.write_h5ad(HERE / "windowed.h5ad", compression="gzip")
    with h5py.File(HERE / "windowed.h5ad", "r+") as file:
        for path, chunk, corrupt in [("X/indptr", 16, 0), ("obs/score", 10, 50)]:
            values, attrs = file[path][:], dict(file[path].attrs)
            del file[path]
            part = file.create_dataset(path, data=values, chunks=(chunk,), compression="gzip")
            part.attrs.update(attrs)
            part.id.write_direct_chunk((corrupt,), b"not deflate", filter_mask=0)
        file["X/data"].id.write_direct_chunk((0,), b"not deflate", filter_mask=0)
    # An otherwise valid AnnData file with a pre-0.7 style compound table.
    dataset.write_h5ad(HERE / "compound.h5ad")
    with h5py.File(HERE / "compound.h5ad", "r+") as file:
        del file["obs"]
        file["obs"] = np.zeros(61, dtype=[("index", "S8"), ("kept", "?")])
    (HERE / "truncated.h5ad").write_bytes((HERE / "dense.h5ad").read_bytes()[:512])


if __name__ == "__main__":
    make()
