use icebug_core::{Edge, ExecutionContext, from_edges};
use icebug_io::{read_snapshot, write_snapshot};

#[test]
fn roundtrip_counts_weights_and_prepared_index() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("graph");
    let graph = from_edges(
        3,
        true,
        true,
        &[
            Edge {
                source: 0,
                target: 0,
                weight: 2.0,
            },
            Edge {
                source: 1,
                target: 2,
                weight: 3.0,
            },
        ],
    )
    .unwrap()
    .prepare_incoming(&ExecutionContext::default())
    .unwrap();
    write_snapshot(&graph, &path).unwrap();
    let loaded = read_snapshot(&path, 1 << 20).unwrap();
    assert_eq!(loaded.node_count(), 3);
    assert_eq!(loaded.edge_count(), 2);
    assert_eq!(loaded.self_loop_count(), 1);
    assert_eq!(loaded.outgoing().weights(), graph.outgoing().weights());
    assert_eq!(
        loaded.incoming().unwrap().targets(),
        graph.incoming().unwrap().targets()
    );
    assert!(write_snapshot(&graph, &path).is_err());
    assert!(read_snapshot(&path, 1).is_err());
    std::fs::write(path.join("out_arcs.arrow"), b"corrupt").unwrap();
    assert!(read_snapshot(&path, 1 << 20).is_err());
}
#[test]
fn rejects_missing_manifest_and_unknown_version() {
    let temp = tempfile::tempdir().unwrap();
    assert!(read_snapshot(temp.path(), 1 << 20).is_err());
    let path = temp.path().join("graph");
    write_snapshot(&from_edges(0, false, true, &[]).unwrap(), &path).unwrap();
    let loaded = read_snapshot(&path, 1 << 20).unwrap();
    assert!(loaded.is_weighted());
    assert_eq!(loaded.node_count(), 0);
    let manifest = path.join("manifest.json");
    let content = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("\"version\":1", "\"version\":99");
    std::fs::write(&manifest, content).unwrap();
    assert!(read_snapshot(&path, 1 << 20).is_err());
}
