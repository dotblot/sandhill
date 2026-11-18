use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};
use hdf5_pure::{AttrValue, Dataset, Datatype, Error, Group, VariableLengthType};
use sandhill_proto::{H5adColumn, H5adColumnKind, H5adTable};

use super::{Attrs, NAME_BYTES, OLD_FILE, attr, clipped, dataset, encoding, group, matrix};

const ROWS: u64 = 50;
const COLUMNS: usize = 128;
const EXAMPLES: u64 = 10;
const CELL_BYTES: usize = 256;
const WINDOW_BYTES: u64 = 8 * 1024 * 1024;
const DATAFRAME: &[(&str, &str)] = &[("dataframe", "0.1.0"), ("dataframe", "0.2.0")];

pub(super) fn open(parent: &Group, name: &str) -> Result<Group> {
    match parent.group(name) {
        Ok(g) => Ok(g),
        Err(Error::NotAGroup(_))
            if matches!(parent.dataset(name)?.datatype()?, Datatype::Compound { .. }) =>
        {
            bail!(OLD_FILE)
        }
        Err(err) => Err(err.into()),
    }
}

pub(super) fn dimensions(group: &Group) -> Result<(u64, u64)> {
    let attrs = group.attrs()?;
    let (index, columns) = metadata(&attrs)?;
    let legacy = super::group(group, "__categories")?;
    let index = Column::open(group, index, legacy.as_ref())?;
    Ok((index.len, columns.len() as u64))
}

pub(super) fn read(group: &Group) -> Result<(H5adTable, u64)> {
    let attrs = group.attrs()?;
    let (index_name, columns) = metadata(&attrs)?;
    let legacy = super::group(group, "__categories")?;
    let index = Column::open(group, index_name, legacy.as_ref())?;
    let len = index.len;
    let rows = len.min(ROWS);
    let (_, index) = index.preview(rows)?;
    let preview = columns
        .iter()
        .take(COLUMNS)
        .map(|name| -> Result<_> {
            let col = Column::open(group, name, legacy.as_ref())
                .with_context(|| format!("Reading column {}", clipped(name.clone(), NAME_BYTES)))?;
            ensure!(col.len == len, "Column length doesn't match its index");
            let (kind, values) = col.preview(rows)?;
            Ok(H5adColumn {
                name: clipped(name.clone(), NAME_BYTES),
                kind,
                values,
            })
        })
        .collect::<Result<_>>()?;
    Ok((
        H5adTable {
            index_name: if index_name == "_index" {
                "Index".into()
            } else {
                clipped(index_name.to_owned(), NAME_BYTES)
            },
            index,
            columns: preview,
            column_count: columns.len() as u64,
        },
        len,
    ))
}

fn metadata(attrs: &Attrs) -> Result<(&str, &[String])> {
    if !attrs.contains_key("encoding-type") {
        bail!(OLD_FILE);
    }
    encoding(attrs, DATAFRAME)?;
    let index = attr(attrs, "_index").context("The AnnData table has no index")?;
    let columns = match attrs.get("column-order") {
        Some(
            AttrValue::StringArray(v)
            | AttrValue::AsciiStringArray(v)
            | AttrValue::VarLenStringArray(v)
            | AttrValue::VarLenAsciiStringArray(v)
            | AttrValue::AsciiStringArraySized { values: v, .. }
            | AttrValue::StringArraySized { values: v, .. },
        ) => v.as_slice(),
        Some(
            AttrValue::String(v)
            | AttrValue::AsciiString(v)
            | AttrValue::VarLenString(v)
            | AttrValue::VarLenAsciiString(v)
            | AttrValue::AsciiStringSized { value: v, .. }
            | AttrValue::StringSized { value: v, .. },
        ) => std::slice::from_ref(v),
        // Some writers use an empty numeric attribute for an empty column order.
        Some(AttrValue::F64Array(v)) if v.is_empty() => &[],
        Some(AttrValue::F32Array(v)) if v.is_empty() => &[],
        _ => bail!("The AnnData table has no valid column order"),
    };
    Ok((index, columns))
}

struct Column {
    len: u64,
    values: Values,
    categories: Option<Values>,
}

impl Column {
    fn open(parent: &Group, name: &str, legacy: Option<&Group>) -> Result<Self> {
        let (values, categories) = match dataset(parent, name) {
            Ok(Some(d)) => {
                let legacy = legacy.map(|g| dataset(g, name)).transpose()?.flatten();
                (
                    Values::dataset(d)?,
                    legacy.map(Values::dataset).transpose()?,
                )
            }
            Ok(None) => bail!("Missing AnnData column"),
            Err(Error::NotADataset(_)) => {
                let g = parent.group(name)?;
                let attrs = g.attrs()?;
                if attr(&attrs, "encoding-type") == Some("categorical") {
                    encoding(&attrs, &[("categorical", "0.2.0")])?;
                    (
                        Values::dataset(g.dataset("codes")?)?,
                        Some(Values::open(&g, "categories")?),
                    )
                } else {
                    (Values::nullable(&g, &attrs)?, None)
                }
            }
            Err(err) => return Err(err.into()),
        };
        Ok(Self {
            len: values.len,
            values,
            categories,
        })
    }

    fn preview(self, rows: u64) -> Result<(H5adColumnKind, Vec<String>)> {
        let Some(categories) = self.categories else {
            let strings = self.values.read(0, rows)?;
            return Ok((self.values.kind, strings));
        };
        let count = categories.len;
        let examples = categories.read(0, count.min(EXAMPLES))?;
        if rows == 0 {
            return Ok((H5adColumnKind::Category { count, examples }, Vec::new()));
        }
        check_window(&self.values.data, rows)?;
        let codes = self.values.data.read_i64_rows(0, rows)?;
        let mut extra = HashMap::new();
        let mut values = Vec::with_capacity(codes.len());
        for code in codes {
            if code == -1 {
                values.push("NA".to_owned());
                continue;
            }
            let code = u64::try_from(code)?;
            ensure!(
                code < count,
                "AnnData categorical code is outside its categories"
            );
            let label = if let Some(label) = examples.get(code as usize) {
                label
            } else if let Some(label) = extra.get(&code) {
                label
            } else {
                let mut label = categories.read(code, 1)?;
                extra
                    .entry(code)
                    .or_insert(label.pop().context("Missing AnnData category label")?)
            };
            values.push(label.clone());
        }
        Ok((H5adColumnKind::Category { count, examples }, values))
    }
}

struct Values {
    data: Dataset,
    mask: Option<Dataset>,
    datatype: Datatype,
    kind: H5adColumnKind,
    len: u64,
}

impl Values {
    fn open(parent: &Group, name: &str) -> Result<Self> {
        match dataset(parent, name) {
            Ok(Some(d)) => Self::dataset(d),
            Ok(None) => bail!("Missing AnnData values"),
            Err(Error::NotADataset(_)) => {
                let g = group(parent, name)?.context("Missing AnnData values")?;
                let attrs = g.attrs()?;
                Self::nullable(&g, &attrs)
            }
            Err(err) => Err(err.into()),
        }
    }

    fn dataset(data: Dataset) -> Result<Self> {
        let attrs = data.attrs()?;
        if attrs.contains_key("encoding-type") {
            encoding(&attrs, &[("array", "0.2.0"), ("string-array", "0.2.0")])?;
        }
        let len = vector_len(&data)?;
        let datatype = data.datatype()?;
        let kind = match &datatype {
            Datatype::String { .. } | Datatype::VariableLength(VariableLengthType::String(_)) => {
                H5adColumnKind::Text
            }
            d if matrix::is_bool(d) => H5adColumnKind::Bool,
            d => H5adColumnKind::Number {
                dtype: matrix::dtype(d)?,
            },
        };
        Ok(Self {
            data,
            mask: None,
            datatype,
            kind,
            len,
        })
    }

    fn nullable(group: &Group, attrs: &Attrs) -> Result<Self> {
        encoding(
            attrs,
            &[
                ("nullable-integer", "0.1.0"),
                ("nullable-boolean", "0.1.0"),
                ("nullable-string-array", "0.1.0"),
            ],
        )?;
        let mut col = Self::dataset(group.dataset("values")?)?;
        let mask = group.dataset("mask")?;
        ensure!(
            vector_len(&mask)? == col.len,
            "AnnData nullable column and mask differ in length"
        );
        col.mask = Some(mask);
        Ok(col)
    }

    fn read(&self, start: u64, count: u64) -> Result<Vec<String>> {
        if count == 0 {
            return Ok(Vec::new());
        }
        let data = &self.data;
        check_window(data, count)?;
        let mut strings = match &self.kind {
            H5adColumnKind::Text => data
                .read_string_rows(start, count)?
                .into_iter()
                .map(|s| clipped(s, CELL_BYTES))
                .collect(),
            H5adColumnKind::Bool => data
                .read_u8_rows(start, count)?
                .into_iter()
                .map(|v| (v != 0).to_string())
                .collect(),
            _ => match &self.datatype {
                Datatype::FixedPoint(n) if n.layout().signed() => {
                    stringify(data.read_i64_rows(start, count)?)
                }
                Datatype::FixedPoint(_) => stringify(data.read_u64_rows(start, count)?),
                Datatype::FloatingPoint(n) if n.size().get() <= 4 => {
                    stringify(data.read_f32_rows(start, count)?)
                }
                Datatype::FloatingPoint(_) => stringify(data.read_f64_rows(start, count)?),
                _ => bail!("Unsupported AnnData column type"),
            },
        };
        ensure!(
            strings.len() as u64 == count,
            "The AnnData column preview has the wrong length"
        );
        if let Some(mask) = &self.mask {
            check_window(mask, count)?;
            for (value, missing) in strings.iter_mut().zip(mask.read_u8_rows(start, count)?) {
                if missing != 0 {
                    *value = "NA".to_owned();
                }
            }
        }
        Ok(strings)
    }
}

fn stringify<T: ToString>(values: Vec<T>) -> Vec<String> {
    values.into_iter().map(|v| v.to_string()).collect()
}

fn vector_len(data: &Dataset) -> Result<u64> {
    match data.shape()?.as_slice() {
        [len] => Ok(*len),
        _ => bail!("An AnnData table column should be one-dimensional"),
    }
}

pub(super) fn check_window(data: &Dataset, count: u64) -> Result<()> {
    let width = data.element_size()?;
    let chunk = data.chunk_shape()?.unwrap_or_default();
    let chunk = chunk.first().copied().unwrap_or(0);
    ensure!(
        width.saturating_mul(count.max(chunk)) <= WINDOW_BYTES,
        "An AnnData preview column or chunk is too wide to read safely"
    );
    Ok(())
}
