use anyhow::{Context, Result, bail, ensure};
use hdf5_pure::{AttrValue, Datatype, Error, Group};
use sandhill_proto::{H5adMatrix, H5adStorage};

use super::{Attrs, OLD_FILE, attr, dataset, encoding, group, is_null, table};

pub(super) fn read(parent: &Group, name: &str, dims: [u64; 2]) -> Result<Option<H5adMatrix>> {
    match dataset(parent, name) {
        Ok(Some(d)) => {
            let attrs = d.attrs()?;
            if is_null(&attrs) {
                return Ok(None);
            }
            if attrs.contains_key("encoding-type") {
                encoding(&attrs, &[("array", "0.2.0")])?;
            }
            ensure!(
                d.shape()? == dims,
                "The matrix dimensions don't match obs and var"
            );
            Ok(Some(H5adMatrix {
                dtype: dtype(&d.datatype()?)?,
                storage: H5adStorage::Dense,
            }))
        }
        Ok(None) => Ok(None),
        Err(Error::NotADataset(_)) => {
            let g = parent.group(name)?;
            let (csr, shape) = sparse_shape(&g.attrs()?)?;
            ensure!(
                shape == dims,
                "The matrix dimensions don't match obs and var"
            );
            let data = g.dataset("data")?;
            // Only inspect the data header. The last pointer gives the stored
            // entry count without reading data or indices.
            let indptr = g.dataset("indptr")?;
            let end = dims[usize::from(!csr)];
            ensure!(
                indptr.shape()? == [end.checked_add(1).context("Invalid sparse matrix extent")?],
                "The sparse matrix has an invalid indptr length"
            );
            table::check_window(&indptr, 1)?;
            let nnz = indptr.read_u64_rows(end, 1)?[0];
            ensure!(
                data.shape()? == [nnz],
                "The sparse matrix data length doesn't match indptr"
            );
            let cells = u128::from(dims[0]) * u128::from(dims[1]);
            let share = if cells == 0 {
                0.0
            } else {
                nnz as f64 / cells as f64
            };
            let storage = if csr {
                H5adStorage::Csr { nnz, share }
            } else {
                H5adStorage::Csc { nnz, share }
            };
            Ok(Some(H5adMatrix {
                dtype: dtype(&data.datatype()?)?,
                storage,
            }))
        }
        Err(err) => Err(err.into()),
    }
}

pub(super) fn shape(parent: &Group, name: &str) -> Result<Vec<u64>> {
    match dataset(parent, name) {
        Ok(Some(d)) => Ok(d.shape()?),
        Ok(None) => bail!("Missing aligned array"),
        Err(Error::NotADataset(_)) => {
            let g = group(parent, name)?.context("Missing aligned array")?;
            let attrs = g.attrs()?;
            if attr(&attrs, "encoding-type") == Some("dataframe") {
                let (length, width) = table::dimensions(&g)?;
                Ok(vec![length, width])
            } else {
                Ok(sparse_shape(&attrs)?.1.to_vec())
            }
        }
        Err(err) => Err(err.into()),
    }
}

fn sparse_shape(attrs: &Attrs) -> Result<(bool, [u64; 2])> {
    let (csr, shape_attr) = match attr(attrs, "encoding-type") {
        Some(ty) => {
            encoding(attrs, &[("csr_matrix", "0.1.0"), ("csc_matrix", "0.1.0")])?;
            (ty == "csr_matrix", "shape")
        }
        None => match attr(attrs, "h5sparse_format") {
            Some("csr") => (true, "h5sparse_shape"),
            Some("csc") => (false, "h5sparse_shape"),
            _ => bail!(OLD_FILE),
        },
    };
    let shape = match attrs.get(shape_attr) {
        Some(AttrValue::I32Array(v)) if v.len() == 2 => {
            [u64::try_from(v[0])?, u64::try_from(v[1])?]
        }
        Some(AttrValue::I64Array(v)) if v.len() == 2 => {
            [u64::try_from(v[0])?, u64::try_from(v[1])?]
        }
        Some(AttrValue::U32Array(v)) if v.len() == 2 => [u64::from(v[0]), u64::from(v[1])],
        Some(AttrValue::U64Array(v)) if v.len() == 2 => [v[0], v[1]],
        _ => bail!("The sparse matrix has no valid shape"),
    };
    Ok((csr, shape))
}

pub(super) fn is_bool(datatype: &Datatype) -> bool {
    matches!(datatype, Datatype::Enumeration { size: 1, members, .. }
        if members.len() == 2 && members.iter().any(|m| m.name == "FALSE" && m.value == [0])
        && members.iter().any(|m| m.name == "TRUE" && m.value == [1]))
}

pub(super) fn dtype(datatype: &Datatype) -> Result<String> {
    let name = match datatype {
        Datatype::FixedPoint(n) => format!(
            "{}int{}",
            if n.layout().signed() { "" } else { "u" },
            n.size().get() * 8
        ),
        Datatype::FloatingPoint(n) => format!("float{}", n.size().get() * 8),
        Datatype::Complex(n) => format!("complex{}", n.size().get() * 8),
        Datatype::Compound { members, size }
            if members.len() == 2
                && members[0].name == "r"
                && members[1].name == "i"
                && matches!(members[0].datatype, Datatype::FloatingPoint(_))
                && members[0].datatype == members[1].datatype =>
        {
            format!("complex{}", size * 8)
        }
        t if is_bool(t) => "bool".to_owned(),
        _ => bail!("Unsupported AnnData numeric type"),
    };
    Ok(name)
}
