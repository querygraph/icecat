use grustcat::{GrustGraph, grust};
use icebug_core::ExecutionContext;
fn main() {
    let a: Vec<_> = std::env::args().collect();
    let text = std::fs::read_to_string(&a[1]).unwrap();
    let mut t = text.split_whitespace();
    let n: usize = t.next().unwrap().parse().unwrap();
    let m: usize = t.next().unwrap().parse().unwrap();
    let nodes = (0..n)
        .map(|i| grust::Node {
            id: i.to_string().into(),
            label: "Node".into(),
            props: grust::Props::new(),
        })
        .collect();
    let edges = (0..m)
        .map(|_| {
            let u = t.next().unwrap();
            let v = t.next().unwrap();
            let w = t.next().unwrap().parse().unwrap();
            grust::Edge::new(
                "EDGE",
                u,
                v,
                grust::Props::from([("weight".into(), grust::Value::Float(w))]),
            )
        })
        .collect();
    let g = GrustGraph::new(grust::Graph::new(nodes, edges), Some("weight")).unwrap();
    let ctx = ExecutionContext::default();
    eprintln!("READY pid={}", std::process::id());
    for _ in 0..1000 {
        let result = match a[2].as_str() {
            "wcc" => g.weakly_connected_components(&ctx),
            "scc" => g.strongly_connected_components(&ctx),
            "bfs" => g.bfs(0, &ctx),
            "dijkstra" => g.dijkstra(0, &ctx),
            _ => g.pagerank(&ctx).map(|r| r.scores),
        }
        .unwrap();
        std::hint::black_box(result);
    }
}
