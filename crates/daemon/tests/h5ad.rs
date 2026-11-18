//! The read-only preview works over the daemon protocol without a kernel.
mod common;

use std::path::Path;

use sandhill_daemon::{Daemon, DaemonOptions, StateDir};
use sandhill_proto::client::{Client, ClientError};
use sandhill_proto::{Call, H5adColumnKind, H5adMatrix, H5adStorage, H5adSummary, SessionInfo};
use tokio::net::UnixListener;

async fn describe(client: &Client, project: &Path, name: &str) -> Result<H5adSummary, ClientError> {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/h5ad");
    let path = project.join(name);
    std::fs::copy(source.join(name), &path).unwrap();
    client
        .call(Call::DescribeH5ad {
            path: path.to_str().unwrap().to_owned(),
        })
        .await
}

fn assert_obs(summary: &H5adSummary, category_count: u64) {
    let obs = &summary.obs;
    assert_eq!(summary.n_obs, 61);
    assert_eq!(obs.index_name, "cell");
    assert_eq!(obs.index.len(), 50);
    assert_eq!(obs.index[49], "cell-49");
    assert_eq!(obs.column_count, 4);
    assert_eq!(
        obs.columns
            .iter()
            .map(|col| col.name.as_str())
            .collect::<Vec<_>>(),
        ["population", "score", "kept", "barcode"]
    );
    let category = &obs.columns[0];
    assert_eq!(
        category.kind,
        H5adColumnKind::Category {
            count: category_count,
            examples: (0..10).map(|i| format!("type-{i:02}")).collect(),
        }
    );
    assert_eq!(category.values.len(), 50);
    assert_eq!(&category.values[..3], ["type-63", "NA", "type-00"]);
    assert_eq!(category.values[49], "type-47");
    let score = &obs.columns[1];
    assert_eq!(
        score.kind,
        H5adColumnKind::Number {
            dtype: "float32".into()
        }
    );
    assert_eq!(
        score.values,
        (0..50).map(|i| format!("{i}.5")).collect::<Vec<_>>()
    );
    assert_eq!(obs.columns[2].kind, H5adColumnKind::Bool);
    assert_eq!(&obs.columns[2].values[..2], ["true", "false"]);
    assert_eq!(obs.columns[3].kind, H5adColumnKind::Text);
    assert_eq!(obs.columns[3].values[49], "béta-49");
    assert_eq!(summary.var.columns[0].name, "rank");
    assert_eq!(
        summary.var.columns[0].kind,
        H5adColumnKind::Number {
            dtype: "int32".into()
        }
    );
    assert_eq!(summary.var.index[0], "g0");
}

#[tokio::test]
async fn previews_anndata_formats_and_keeps_serving_after_errors() {
    let tmp = common::tempdir().unwrap();
    let project = tmp.path().join("AnnData files");
    std::fs::create_dir(&project).unwrap();
    let state = StateDir::new(tmp.path().join("state"));
    let daemon = Daemon::open_with(
        state.clone(),
        DaemonOptions {
            executable: env!("CARGO_BIN_EXE_sandhilld").into(),
            socket: state.socket_path(),
            started_by: String::new(),
        },
    )
    .await
    .unwrap();
    let listener = UnixListener::bind(state.socket_path()).unwrap();
    let server = tokio::spawn(daemon.serve(listener));
    let client = Client::connect_unix(&state.socket_path()).await.unwrap();

    let dense = describe(&client, &project, "dense.h5ad").await.unwrap();
    assert_obs(&dense, 60);
    assert_eq!(dense.n_vars, 4);
    assert_eq!(dense.var.columns[0].values, ["0", "1", "2", "3"]);
    assert_eq!(
        dense.x,
        Some(H5adMatrix {
            dtype: "float32".into(),
            storage: H5adStorage::Dense
        })
    );
    assert_eq!(
        dense.layers,
        vec![
            (
                "counts".into(),
                H5adMatrix {
                    dtype: "int16".into(),
                    storage: H5adStorage::Csc {
                        nnz: 2,
                        share: 2. / 244.
                    }
                }
            ),
            (
                "scaled".into(),
                H5adMatrix {
                    dtype: "float64".into(),
                    storage: H5adStorage::Dense
                }
            ),
        ]
    );
    assert_eq!(
        dense.obsm,
        [
            ("X_latent".into(), vec![61, 3]),
            ("X_umap".into(), vec![61, 2])
        ]
    );
    assert_eq!(dense.varm, [("loadings".into(), vec![4, 6])]);
    assert_eq!(dense.obsp, [("neighbors".into(), vec![61, 61])]);
    assert_eq!(dense.varp, [("pairs".into(), vec![4, 4])]);
    assert_eq!(dense.uns, ["params", "version"]);
    assert_eq!(dense.raw_n_vars, Some(5));

    let truncated = describe(&client, &project, "truncated.h5ad")
        .await
        .unwrap_err();
    assert!(truncated.to_string().contains(".h5ad"), "{truncated:#}");
    client.ping().await.unwrap();

    let old = describe(&client, &project, "compound.h5ad")
        .await
        .unwrap_err();
    assert!(
        old.to_string()
            .contains("This .h5ad was written by an older anndata that Sandhill can't read"),
        "{old:#}"
    );

    for name in ["csr.h5ad", "csc.h5ad", "legacy.h5ad", "windowed.h5ad"] {
        let summary = describe(&client, &project, name)
            .await
            .unwrap_or_else(|err| panic!("{name}: {err:#}"));
        assert_obs(&summary, 64);
        assert_eq!(summary.n_vars, 5);
        let (nnz, share) = (3, 3. / 305.);
        let storage = if name == "csc.h5ad" {
            H5adStorage::Csc { nnz, share }
        } else {
            H5adStorage::Csr { nnz, share }
        };
        assert_eq!(
            summary.x,
            Some(H5adMatrix {
                dtype: "float32".into(),
                storage
            })
        );
        assert_eq!(summary.raw_n_vars, None);
    }

    let empty = describe(&client, &project, "empty.h5ad").await.unwrap();
    assert_eq!((empty.n_obs, empty.n_vars), (0, 5));
    assert_eq!(empty.x, None);
    assert_eq!(empty.obs.column_count, 0);
    assert!(empty.obs.index.is_empty());
    assert_eq!(empty.var.index, ["0", "1", "2", "3", "4"]);
    assert!(
        client
            .call::<Vec<SessionInfo>>(Call::ListSessions)
            .await
            .unwrap()
            .is_empty()
    );
    server.abort();
}
