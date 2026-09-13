use arrow_array::{Array, Float64Array, UInt64Array};
use grustcat::{
    GrustcatGraph,
    grust::{Edge, Graph, Node, Props, Value},
};
use grustcat_cypher::{CypherParameters, PreparedQuery, query};
use icebug_core::ExecutionContext;

fn graph() -> GrustcatGraph {
    let nodes = (0..5)
        .map(|i| Node {
            id: i.to_string().into(),
            label: "Node".into(),
            props: Props::new(),
        })
        .collect();
    let edges = [
        (0, 1, 0.),
        (1, 0, 0.),
        (0, 2, 2.),
        (1, 2, 2.),
        (2, 3, 3.),
        (0, 3, 5.),
    ]
    .into_iter()
    .map(|(u, v, w)| {
        Edge::new(
            "EDGE",
            u.to_string(),
            v.to_string(),
            Props::from([("weight".into(), Value::Float(w))]),
        )
    })
    .collect();
    GrustcatGraph::new(Graph::new(nodes, edges), Some("weight")).unwrap()
}
const FULL: &str = "CALL grustcat.fullPaths($source) YIELD nodeIds,costs UNWIND range(0,size(nodeIds)-1) AS i RETURN count(*) AS entries,sum(nodeIds[i]) AS nodes,sum(costs[i]) AS costs";

#[test]
fn algorithms_and_parameter_rebinding_match_direct_kernels() {
    let graph = graph();
    let ctx = ExecutionContext::default();
    for (name, field) in [
        ("bfs", "distance"),
        ("dijkstra", "distance"),
        ("wcc", "component_id"),
        ("scc", "component_id"),
        ("pagerank", "score"),
    ] {
        let arg = if matches!(name, "bfs" | "dijkstra") {
            "$source"
        } else {
            ""
        };
        let text = format!(
            "CALL grustcat.{name}({arg}) YIELD node_id AS n,{field} AS v RETURN v AS value,n AS id"
        );
        let plan = PreparedQuery::parse(&text).unwrap();
        for source in [0, 2, 4] {
            let params = CypherParameters::from([("source".into(), Value::Int(source))]);
            let actual = plan.execute(&graph, &params, &ctx).unwrap();
            let expected = match name {
                "bfs" => graph.bfs(source as usize, &ctx).unwrap(),
                "dijkstra" => graph.dijkstra(source as usize, &ctx).unwrap(),
                "wcc" => graph.weakly_connected_components(&ctx).unwrap(),
                "scc" => graph.strongly_connected_components(&ctx).unwrap(),
                _ => graph.pagerank(&ctx).unwrap().scores,
            };
            assert_eq!(actual.rows.column(0), expected.column(1));
            assert_eq!(actual.rows.column(1), expected.column(0));
            assert_eq!(actual.rows.schema().field(0).name(), "value");
            assert_eq!(actual.rows.schema().field(1).name(), "id");
        }
    }
}

#[test]
fn full_path_aggregates_match_grust_reference_cypher() {
    let graph = graph();
    let ctx = ExecutionContext::default();
    for source in [0, 2, 4] {
        let mut paths = Vec::new();
        let expected = graph
            .dijkstra_paths(source, &ctx, &mut |node_ids, costs| {
                paths.push(serde_json::json!({"nodeIds":node_ids,"costs":costs}))
            })
            .unwrap();
        let params = CypherParameters::from([("source".into(), Value::Int(source as i64))]);
        let actual = query(&graph, FULL, &params, &ctx).unwrap();
        assert_eq!(actual.distances.unwrap(), expected);
        assert_eq!(actual.reachable as usize, paths.len());
        let reference_text = "UNWIND $paths AS p WITH p.nodeIds AS nodeIds,p.costs AS costs UNWIND range(0,size(nodeIds)-1) AS i RETURN count(*) AS entries,sum(nodeIds[i]) AS nodes,sum(costs[i]) AS costs";
        let reference = grust_cypher::read::run_read_query(
            &Graph::default(),
            reference_text,
            &CypherParameters::from([(
                "paths".into(),
                Value::Json(serde_json::Value::Array(paths)),
            )]),
        )
        .unwrap();
        for i in 0..2 {
            let value = actual
                .rows
                .column(i)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(0);
            assert_eq!(reference.rows[0][i], Value::Int(value as i64));
        }
        let value = actual
            .rows
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert_eq!(reference.rows[0][2], Value::Float(value));
    }
}

#[test]
fn rejects_unsupported_or_unbound_query_parts() {
    let rejected = [
        "CALL gds.wcc() YIELD node_id RETURN node_id",
        "CALL grustcat.wcc(1) YIELD node_id RETURN node_id",
        "CALL grustcat.wcc() YIELD nonexistent RETURN nonexistent",
        "CALL grustcat.wcc() YIELD node_id RETURN absent",
        "CALL grustcat.wcc() YIELD node_id WHERE node_id > 1 RETURN node_id",
        "CALL grustcat.wcc() YIELD node_id RETURN node_id LIMIT 1",
        "CALL grustcat.wcc() YIELD node_id RETURN DISTINCT node_id",
        "CALL grustcat.wcc() YIELD node_id RETURN node_id AS x,node_id AS x",
        "CALL grustcat.wcc() YIELD node_id RETURN node_id UNION RETURN 1 AS node_id",
        "CALL grustcat.dijkstra(0+1) YIELD distance RETURN distance",
        "MATCH (n) RETURN n",
        "CREATE (n)",
    ];
    for text in rejected {
        assert!(PreparedQuery::parse(text).is_err(), "{text}");
    }
    for text in [
        FULL.replace("range(0,", "range(1,"),
        FULL.replace("nodeIds[i]", "nodeIds[0]"),
        FULL.replace("count(*)", "count(DISTINCT i)"),
        FULL.replace("size(nodeIds)", "size(costs)"),
    ] {
        assert!(PreparedQuery::parse(&text).is_err(), "{text}");
    }
}

#[test]
fn aliases_comments_cancellation_and_errors() {
    let graph = graph();
    let ctx = ExecutionContext::default();
    let text = "/* query */ CALL grustcat.fullPaths(0) YIELD costs AS c,nodeIds AS n UNWIND range(0,size(n)-1) AS j RETURN sum(c[j]) AS total, count(*) AS entries,sum(n[j]) AS nodes";
    let result = query(&graph, text, &CypherParameters::new(), &ctx).unwrap();
    assert_eq!(result.rows.num_columns(), 3);
    assert!(!result.rows.column(0).is_null(0));
    for value in [Value::Int(-1), Value::Int(5), Value::Float(0.), Value::Null] {
        assert!(
            query(
                &graph,
                FULL,
                &CypherParameters::from([("source".into(), value)]),
                &ctx
            )
            .is_err()
        );
    }
    assert!(query(&graph, FULL, &CypherParameters::new(), &ctx).is_err());
    ctx.cancel();
    assert!(query(&graph, text, &CypherParameters::new(), &ctx).is_err());
}

#[test]
fn unused_aggregates_do_not_introduce_overflow_errors() {
    let nodes = (0..3)
        .map(|i| Node {
            id: i.to_string().into(),
            label: "Node".into(),
            props: Props::new(),
        })
        .collect();
    let edges = (1..3)
        .map(|i| {
            Edge::new(
                "EDGE",
                "0",
                i.to_string(),
                Props::from([("weight".into(), Value::Float(1e308))]),
            )
        })
        .collect();
    let g = GrustcatGraph::new(Graph::new(nodes, edges), Some("weight")).unwrap();
    let ctx = ExecutionContext::default();
    let count = "CALL grustcat.fullPaths(0) YIELD nodeIds,costs UNWIND range(0,size(nodeIds)-1) AS i RETURN count(*) AS entries";
    let result = query(&g, count, &CypherParameters::new(), &ctx).unwrap();
    assert_eq!(
        result
            .rows
            .column(0)
            .as_any()
            .downcast_ref::<UInt64Array>()
            .unwrap()
            .value(0),
        5
    );
    assert!(
        query(
            &g,
            FULL,
            &CypherParameters::from([("source".into(), Value::Int(0))]),
            &ctx
        )
        .is_err()
    );
}
