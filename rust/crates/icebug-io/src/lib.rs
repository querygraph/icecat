//! Versioned Arrow persistence.
//! A snapshot is a directory of separate IPC sections and a checksummed manifest.
//! A directory is readable only after manifest publication. Existing targets are never reused.
use arrow_array::{Array, Float64Array, RecordBatch, UInt64Array};
use arrow_ipc::{reader::FileReader, writer::FileWriter};
use icebug_core::{CsrAdjacency, Error, Graph, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    directed: bool,
    nodes: usize,
    edges: usize,
    loops: usize,
    files: BTreeMap<String, String>,
}
fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidGraph(message.into())
}
fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn write_batch(
    dir: &Path,
    name: &str,
    batch: &RecordBatch,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    let path = dir.join(name);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let mut writer = FileWriter::try_new(file, &batch.schema())?;
    writer.write(batch)?;
    writer.finish()?;
    writer.into_inner()?.sync_all()?;
    files.insert(name.into(), digest(&path)?);
    Ok(())
}
fn write_csr(
    dir: &Path,
    prefix: &str,
    csr: &CsrAdjacency,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    let offsets =
        RecordBatch::try_from_iter(vec![("offset", Arc::new(csr.offsets().clone()) as _)])?;
    write_batch(dir, &format!("{prefix}_offsets.arrow"), &offsets, files)?;
    let mut cols = vec![(
        "target",
        Arc::new(csr.targets().clone()) as arrow_array::ArrayRef,
    )];
    if let Some(weights) = csr.weights() {
        cols.push(("weight", Arc::new(weights.clone())));
    }
    write_batch(
        dir,
        &format!("{prefix}_arcs.arrow"),
        &RecordBatch::try_from_iter(cols)?,
        files,
    )
}
/// Write a new snapshot directory. A failed write may leave an incomplete directory,
/// which readers reject; the final manifest is published without overwriting an existing file.
pub fn write_snapshot(graph: &Graph, directory: impl AsRef<Path>) -> Result<()> {
    let dir = directory.as_ref();
    fs::create_dir(dir)?;
    let mut files = BTreeMap::new();
    write_csr(dir, "out", graph.outgoing(), &mut files)?;
    if graph.is_directed()
        && let Ok(incoming) = graph.incoming()
    {
        write_csr(dir, "in", incoming, &mut files)?;
    }
    if let Some(nodes) = graph.node_properties() {
        write_batch(dir, "nodes.arrow", nodes, &mut files)?;
    }
    if let Some(edges) = graph.edge_properties() {
        write_batch(dir, "edges.arrow", edges, &mut files)?;
    }
    if let Some(ids) = graph.slot_edge_ids() {
        write_batch(
            dir,
            "slot_edges.arrow",
            &RecordBatch::try_from_iter(vec![("edge_id", Arc::new(ids.clone()) as _)])?,
            &mut files,
        )?;
    }
    let manifest = Manifest {
        version: 1,
        directed: graph.is_directed(),
        nodes: graph.node_count(),
        edges: graph.edge_count(),
        loops: graph.self_loop_count(),
        files,
    };
    let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut temporary, &manifest).map_err(|e| invalid(e.to_string()))?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(dir.join("manifest.json"))
        .map_err(|e| Error::Io(e.error))?;
    Ok(())
}
fn read_batch(path: &Path) -> Result<RecordBatch> {
    let mut reader = FileReader::try_new(File::open(path)?, None)?;
    let batch = reader
        .next()
        .ok_or_else(|| invalid("missing record batch"))??;
    if reader.next().is_some() {
        return Err(invalid("snapshot section must contain exactly one batch"));
    }
    Ok(batch)
}
fn u64_column(batch: &RecordBatch, name: &str) -> Result<UInt64Array> {
    batch
        .column_by_name(name)
        .and_then(|a| a.as_any().downcast_ref::<UInt64Array>())
        .cloned()
        .ok_or_else(|| invalid(format!("missing UInt64 {name} column")))
}
fn read_csr(dir: &Path, prefix: &str, n: usize) -> Result<CsrAdjacency> {
    let offsets = read_batch(&dir.join(format!("{prefix}_offsets.arrow")))?;
    let arcs = read_batch(&dir.join(format!("{prefix}_arcs.arrow")))?;
    let weights = arcs
        .column_by_name("weight")
        .map(|a| {
            a.as_any()
                .downcast_ref::<Float64Array>()
                .cloned()
                .ok_or_else(|| invalid("weight must be Float64"))
        })
        .transpose()?;
    CsrAdjacency::try_new(
        n,
        u64_column(&arcs, "target")?,
        u64_column(&offsets, "offset")?,
        weights,
    )
}
/// Read and validate a version-1 snapshot, limiting the total encoded section bytes.
/// This is an encoded-size guard, not a complete decoded-memory/RSS admission limit.
pub fn read_snapshot(directory: impl AsRef<Path>, max_encoded_bytes: u64) -> Result<Graph> {
    let dir = directory.as_ref();
    let path = dir.join("manifest.json");
    if fs::metadata(&path)?.len() > 65536 {
        return Err(invalid("manifest too large"));
    }
    let manifest: Manifest =
        serde_json::from_reader(File::open(path)?).map_err(|e| invalid(e.to_string()))?;
    if manifest.version != 1 {
        return Err(invalid("unsupported snapshot version"));
    }
    let allowed = [
        "out_offsets.arrow",
        "out_arcs.arrow",
        "in_offsets.arrow",
        "in_arcs.arrow",
        "nodes.arrow",
        "edges.arrow",
        "slot_edges.arrow",
    ];
    if !manifest.files.contains_key("out_offsets.arrow")
        || !manifest.files.contains_key("out_arcs.arrow")
    {
        return Err(invalid("outgoing sections missing"));
    }
    let has_in = manifest.files.contains_key("in_offsets.arrow");
    if has_in != manifest.files.contains_key("in_arcs.arrow") || (!manifest.directed && has_in) {
        return Err(invalid("invalid incoming sections"));
    }
    let mut bytes = 0u64;
    for (name, checksum) in &manifest.files {
        if !allowed.contains(&name.as_str()) {
            return Err(invalid("unknown snapshot section"));
        }
        let section = dir.join(name);
        if fs::symlink_metadata(&section)?.file_type().is_symlink() {
            return Err(invalid("snapshot sections cannot be symlinks"));
        }
        bytes = bytes
            .checked_add(fs::metadata(&section)?.len())
            .ok_or(Error::Overflow)?;
        if bytes > max_encoded_bytes {
            return Err(invalid("snapshot exceeds encoded size limit"));
        }
        if digest(&section)? != *checksum {
            return Err(invalid(format!("checksum mismatch: {name}")));
        }
    }
    let graph = Graph::try_new(
        manifest.directed,
        read_csr(dir, "out", manifest.nodes)?,
        if has_in {
            Some(read_csr(dir, "in", manifest.nodes)?)
        } else {
            None
        },
    )?;
    if graph.edge_count() != manifest.edges || graph.self_loop_count() != manifest.loops {
        return Err(invalid("manifest counts differ from topology"));
    }
    let nodes = manifest
        .files
        .contains_key("nodes.arrow")
        .then(|| read_batch(&dir.join("nodes.arrow")))
        .transpose()?;
    let edges = manifest
        .files
        .contains_key("edges.arrow")
        .then(|| read_batch(&dir.join("edges.arrow")))
        .transpose()?;
    let ids = manifest
        .files
        .contains_key("slot_edges.arrow")
        .then(|| read_batch(&dir.join("slot_edges.arrow")).and_then(|b| u64_column(&b, "edge_id")))
        .transpose()?;
    graph.with_properties(nodes, edges, ids)
}
