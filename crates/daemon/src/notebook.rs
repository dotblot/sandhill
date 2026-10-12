//! Opening `.ipynb` files for the app without shipping their plots inline.
//!
//! A notebook keeps every output in the file, so a scanpy notebook is mostly
//! base64 PNGs. Sending that text to the app costs a copy of it in every
//! process on the way (daemon, app, web view) and leaves the web view
//! holding the base64 in its notebook model and again in `data:` URLs.
//! `read_notebook` moves those values into the blob store, the same way the
//! journal does for live outputs, and returns the notebook with blob
//! references in their place. The app loads plots by URL. `write_notebook`
//! takes the notebook back with its references and puts the values back as
//! it writes the file, so saving doesn't bring every plot through the app
//! either.

use std::fmt;
use std::io::Write;

use anyhow::{Context, Result, bail};
use sandhill_proto::{BLOB_KEY, FileText, FileWritten};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{self, SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::ser::PrettyFormatter;
use serde_json::{Map, Value, json};

use crate::blobs::{BlobStore, INLINE_LIMIT, is_binary_mime};
use crate::files;

pub fn read_notebook(path: &str, store: &BlobStore) -> Result<FileText> {
    let resolved = files::resolve(path)?;
    let meta = std::fs::metadata(&resolved)
        .with_context(|| format!("cannot open {}", resolved.display()))?;
    if meta.is_dir() {
        bail!("{} is a directory", resolved.display());
    }
    // Parsed as it is read, storing each large value as soon as it is
    // complete, so memory use doesn't grow with the notebook's size.
    let file = std::fs::File::open(&resolved)?;
    let reader = std::io::BufReader::with_capacity(64 * 1024, file);
    let mut de = serde_json::Deserializer::from_reader(reader);
    let notebook = Node {
        store,
        place: Place::Root,
    }
    .deserialize(&mut de)
    .and_then(|v| de.end().map(|()| v))
    .with_context(|| format!("{} is not a Jupyter notebook", resolved.display()))?;
    Ok(FileText {
        path: resolved.to_string_lossy().into_owned(),
        text: serde_json::to_string(&notebook)?,
        mtime_ms: files::mtime_ms(&meta),
    })
}

/// Writes a notebook the app sends with blob references in its outputs,
/// putting the stored values back as the file is written. The app lays the
/// notebook out as nbformat does (keys sorted, one-space indent, text split
/// into lines); the values put back follow the same layout.
pub fn write_notebook(
    path: &str,
    text: &str,
    expected_mtime_ms: Option<i64>,
    store: &BlobStore,
) -> Result<FileWritten> {
    let notebook: Value = serde_json::from_str(text).context("not a Jupyter notebook")?;
    files::write_with(path, expected_mtime_ms, |file| {
        let mut out = std::io::BufWriter::with_capacity(64 * 1024, file);
        let mut json =
            serde_json::Serializer::with_formatter(&mut out, PrettyFormatter::with_indent(b" "));
        WithBlobs {
            value: &notebook,
            place: Place::Root,
            store,
        }
        .serialize(&mut json)?;
        out.write_all(b"\n")?;
        out.flush()?;
        Ok(())
    })
}

/// Where a value sits in the notebook: only `cells[].outputs[].data.*`
/// values are candidates for the blob store.
#[derive(Clone, Copy)]
enum Place {
    Root,
    Cells,
    Cell,
    Outputs,
    Output,
    Data,
    MimeValue { binary: bool, split: bool },
    Other,
}

impl Place {
    fn field(self, key: &str) -> Place {
        match (self, key) {
            (Place::Root, "cells") => Place::Cells,
            (Place::Cell, "outputs") => Place::Outputs,
            (Place::Output, "data") => Place::Data,
            (Place::Data, mime) => Place::MimeValue {
                binary: is_binary_mime(mime),
                // As nbformat (and the app's serializer) split them.
                split: mime.starts_with("text/")
                    || mime == "application/javascript"
                    || mime == "image/svg+xml",
            },
            _ => Place::Other,
        }
    }

    fn element(self) -> Place {
        match self {
            Place::Cells => Place::Cell,
            Place::Outputs => Place::Output,
            _ => Place::Other,
        }
    }
}

struct Node<'a> {
    store: &'a BlobStore,
    place: Place,
}

impl<'de> DeserializeSeed<'de> for Node<'_> {
    type Value = Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        match self.place {
            Place::Other => Value::deserialize(deserializer),
            Place::MimeValue { binary, .. } => {
                let value = Value::deserialize(deserializer)?;
                externalize_value(self.store, binary, value).map_err(de::Error::custom)
            }
            _ => deserializer.deserialize_any(self),
        }
    }
}

impl<'de> Visitor<'de> for Node<'_> {
    type Value = Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a Jupyter notebook")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut out = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            let place = self.place.field(&key);
            let value = map.next_value_seed(Node {
                store: self.store,
                place,
            })?;
            out.insert(key, value);
        }
        Ok(Value::Object(out))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let place = self.place.element();
        let mut out = Vec::new();
        while let Some(value) = seq.next_element_seed(Node {
            store: self.store,
            place,
        })? {
            out.push(value);
        }
        Ok(Value::Array(out))
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }
}

/// Moves a binary or oversized output value to the store, as the journal
/// does for live outputs. nbformat keeps text as a list of lines; a large
/// one is stored whole.
fn externalize_value(store: &BlobStore, binary: bool, value: Value) -> std::io::Result<Value> {
    let text = match value {
        Value::String(s) if binary || s.len() > INLINE_LIMIT => s,
        Value::Array(lines)
            if lines.iter().all(Value::is_string)
                && lines
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::len)
                    .sum::<usize>()
                    > INLINE_LIMIT =>
        {
            lines.iter().filter_map(Value::as_str).collect()
        }
        other => return Ok(other),
    };
    let hash = store.put(text.as_bytes())?;
    Ok(json!({ BLOB_KEY: hash, "size": text.len() }))
}

/// A notebook value written with its blob references replaced by what they
/// refer to.
struct WithBlobs<'a> {
    value: &'a Value,
    place: Place,
    store: &'a BlobStore,
}

impl Serialize for WithBlobs<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let store = self.store;
        match (self.place, self.value) {
            (Place::Other, value) => value.serialize(serializer),
            (Place::MimeValue { split, .. }, Value::Object(map)) => {
                let Some(hash) = map.get(BLOB_KEY).and_then(Value::as_str) else {
                    return self.value.serialize(serializer);
                };
                let bytes = store.get(hash).map_err(|err| {
                    ser::Error::custom(format!(
                        "an output of this notebook ({hash}) is missing: {err}"
                    ))
                })?;
                let text = String::from_utf8(bytes).map_err(ser::Error::custom)?;
                if split {
                    serializer.collect_seq(split_lines(&text))
                } else {
                    serializer.serialize_str(&text)
                }
            }
            (place, Value::Object(map)) => {
                let mut out = serializer.serialize_map(Some(map.len()))?;
                for (key, value) in map {
                    let place = place.field(key);
                    out.serialize_entry(
                        key,
                        &WithBlobs {
                            value,
                            place,
                            store,
                        },
                    )?;
                }
                out.end()
            }
            (place, Value::Array(items)) => {
                let place = place.element();
                serializer.collect_seq(items.iter().map(|value| WithBlobs {
                    value,
                    place,
                    store,
                }))
            }
            (_, value) => value.serialize(serializer),
        }
    }
}

/// Python's `str.splitlines(keepends=True)`, which nbformat uses to store
/// text as a list of lines. The app's `splitLines` does the same.
pub(crate) fn split_lines(text: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let end = match c {
            '\r' if matches!(chars.peek(), Some((_, '\n'))) => {
                chars.next();
                i + 2
            }
            '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}'
            | '\u{2029}' => i + c.len_utf8(),
            _ => continue,
        };
        lines.push(&text[start..end]);
        start = end;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plots_and_large_tables_become_blob_references() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs"));
        let table: Vec<String> = (0..20_000)
            .map(|i| format!("<tr><td>{i}</td></tr>\n"))
            .collect();
        let path = dir.path().join("a.ipynb");
        let notebook = json!({
            "cells": [{
                "cell_type": "code", "id": "c1", "metadata": {}, "source": ["plot()"],
                "execution_count": 1,
                "outputs": [
                    {"output_type": "display_data", "metadata": {},
                     "data": {"image/png": "iVBORw0KGgo=\n", "text/plain": ["<Figure>"]}},
                    {"output_type": "execute_result", "execution_count": 1, "metadata": {},
                     "data": {"text/html": table, "text/plain": ["df"]}},
                    {"output_type": "stream", "name": "stdout", "text": ["hi\n"]}
                ]
            }],
            "metadata": {"widgets": {"data": {"image/png": "not an output"}}},
            "nbformat": 4, "nbformat_minor": 5
        });
        std::fs::write(&path, notebook.to_string()).unwrap();

        let read = read_notebook(&path.to_string_lossy(), &store).unwrap();
        let out: Value = serde_json::from_str(&read.text).unwrap();
        let outputs = &out["cells"][0]["outputs"];
        let png = outputs[0]["data"]["image/png"][BLOB_KEY].as_str().unwrap();
        assert_eq!(store.get(png).unwrap(), b"iVBORw0KGgo=\n");
        assert_eq!(outputs[0]["data"]["text/plain"], json!(["<Figure>"]));
        let html = outputs[1]["data"]["text/html"][BLOB_KEY].as_str().unwrap();
        assert_eq!(
            String::from_utf8(store.get(html).unwrap()).unwrap(),
            table.concat()
        );
        assert_eq!(outputs[2]["text"], json!(["hi\n"]));
        assert_eq!(out["metadata"], notebook["metadata"]);
        assert_eq!(out["nbformat"], 4);
        assert!(read.text.len() < 1000, "{}", read.text.len());
    }

    /// The notebook laid out as the app's `serializeNotebook` does.
    fn nbformat_layout(value: &Value) -> String {
        let mut out = Vec::new();
        let mut json =
            serde_json::Serializer::with_formatter(&mut out, PrettyFormatter::with_indent(b" "));
        value.serialize(&mut json).unwrap();
        String::from_utf8(out).unwrap() + "\n"
    }

    /// Opens `path` as the app does and saves it back unchanged.
    fn open_and_save(path: &std::path::Path, store: &BlobStore) -> FileWritten {
        let path = path.to_string_lossy();
        let read = read_notebook(&path, store).unwrap();
        let text = nbformat_layout(&serde_json::from_str(&read.text).unwrap());
        write_notebook(&path, &text, Some(read.mtime_ms), store).unwrap()
    }

    #[test]
    fn saving_puts_the_plots_back_byte_for_byte() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs"));
        let original = include_str!("../../../tests/fixtures/jupyter-written.ipynb");
        let path = dir.path().join("a.ipynb");
        std::fs::write(&path, original).unwrap();
        open_and_save(&path, &store);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn saving_splits_a_large_table_into_lines_again() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs"));
        let table: Vec<String> = (0..20_000)
            .map(|i| format!("<tr><td>{i}</td></tr>\n"))
            .collect();
        let notebook = json!({
            "cells": [{
                "cell_type": "code", "execution_count": 1, "id": "c1", "metadata": {},
                "outputs": [{
                    "data": {"text/html": table, "text/plain": ["df"]},
                    "execution_count": 1, "metadata": {}, "output_type": "execute_result"
                }],
                "source": ["df"]
            }],
            "metadata": {}, "nbformat": 4, "nbformat_minor": 5
        });
        let original = nbformat_layout(&notebook);
        let path = dir.path().join("a.ipynb");
        std::fs::write(&path, &original).unwrap();
        open_and_save(&path, &store);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    #[test]
    fn a_missing_plot_leaves_the_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs"));
        let path = dir.path().join("a.ipynb");
        std::fs::write(&path, "old").unwrap();
        let text = json!({
            "cells": [{
                "cell_type": "code", "id": "c1", "metadata": {}, "source": [],
                "execution_count": 1,
                "outputs": [{"output_type": "display_data", "metadata": {},
                             "data": {"image/png": {BLOB_KEY: "0".repeat(64), "size": 3}}}]
            }],
            "metadata": {}, "nbformat": 4, "nbformat_minor": 5
        })
        .to_string();
        let err = write_notebook(&path.to_string_lossy(), &text, None, &store).unwrap_err();
        assert!(format!("{err:#}").contains("is missing"), "{err:#}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["a.ipynb"], "a temporary file was left");
    }

    #[test]
    fn a_reference_outside_outputs_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs"));
        let path = dir.path().join("a.ipynb");
        let reference = json!({BLOB_KEY: "0".repeat(64)});
        let notebook = json!({"cells": [], "metadata": {"x": {"data": {"image/png": reference}}}});
        let text = nbformat_layout(&notebook);
        write_notebook(&path.to_string_lossy(), &text, None, &store).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    #[test]
    fn lines_split_as_python_splitlines() {
        assert_eq!(split_lines("a\nb\r\nc\rd"), ["a\n", "b\r\n", "c\r", "d"]);
        assert_eq!(split_lines(""), Vec::<&str>::new());
        assert_eq!(split_lines("x\n"), ["x\n"]);
        assert_eq!(split_lines("\n\n"), ["\n", "\n"]);
        assert_eq!(split_lines("é\u{2028}ü"), ["é\u{2028}", "ü"]);
    }

    #[test]
    fn invalid_json_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::new(dir.path().join("blobs"));
        let path = dir.path().join("a.ipynb");
        std::fs::write(&path, "{\"cells\": [").unwrap();
        assert!(read_notebook(&path.to_string_lossy(), &store).is_err());
    }
}
