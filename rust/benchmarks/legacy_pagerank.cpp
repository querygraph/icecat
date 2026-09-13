// Baseline-only executable. Not linked into the Rust implementation.
#include <networkit/auxiliary/Parallelism.hpp>
#include <networkit/auxiliary/Vector2Arrow.hpp>
#include <networkit/centrality/PageRank.hpp>
#include <networkit/graph/GraphR.hpp>

#include <algorithm>
#include <chrono>
#include <iomanip>
#include <iostream>
#include <vector>

int main(int argc, char **argv) {
    using namespace NetworKit;
    const count n = argc > 1 ? std::stoull(argv[1]) : 100000;
    if (n < 3) {
        return 1;
    }
    Aux::setNumberOfThreads(1);
    std::vector<std::vector<node>> out(n), in(n);
    for (node u = 0; u < n; ++u) {
        if (u % 11 == 0) {
            continue;
        }
        for (node v : {(u + 1) % n, (u + 2) % n}) {
            out[u].push_back(v);
            in[v].push_back(u);
        }
    }
    std::vector<node> outTargets, inTargets;
    std::vector<count> outOffsets{0}, inOffsets{0};
    for (node u = 0; u < n; ++u) {
        std::sort(out[u].begin(), out[u].end());
        std::sort(in[u].begin(), in[u].end());
        outTargets.insert(outTargets.end(), out[u].begin(), out[u].end());
        inTargets.insert(inTargets.end(), in[u].begin(), in[u].end());
        outOffsets.push_back(outTargets.size());
        inOffsets.push_back(inTargets.size());
    }
    GraphR storage(n, true,
                   Aux::vectorToArrow<node, arrow::UInt64Array>(std::move(outTargets)),
                   Aux::vectorToArrow<count, arrow::UInt64Array>(std::move(outOffsets)),
                   Aux::vectorToArrow<node, arrow::UInt64Array>(std::move(inTargets)),
                   Aux::vectorToArrow<count, arrow::UInt64Array>(std::move(inOffsets)));
    const auto graph = storage.asGraph();
    PageRank rank(graph, 0.85, 1e-8, false, PageRank::DISTRIBUTE_SINKS);
    rank.norm = PageRank::Norm::L1_NORM;
    rank.maxIterations = 1000;
    const auto start = std::chrono::steady_clock::now();
    rank.run();
    const auto elapsed = std::chrono::duration<double, std::milli>(
        std::chrono::steady_clock::now() - start).count();
    double sum = 0, checksum = 0;
    for (node u = 0; u < n; ++u) {
        sum += rank.scores()[u];
        checksum += rank.scores()[u] * (u + 1);
    }
    std::cout << "nodes,edges,pagerank_ms,iterations,sum,checksum\n"
              << n << ',' << storage.numberOfEdges() << ',' << std::setprecision(15)
              << elapsed << ',' << rank.numberOfIterations() << ',' << sum << ','
              << checksum << '\n';
}
