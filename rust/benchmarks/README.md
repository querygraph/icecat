# PageRank reference fixture

Both executables construct the same directed unweighted simple graph: for every
vertex `u` not divisible by 11, add edges to `(u+1) % n` and `(u+2) % n`.
Vertices divisible by 11 are dangling. Node IDs and CSR neighbors are ordered.
Use `n >= 3`. No external dataset, download, or random seed is required.

Both kernels use one thread, uniform personalization, damping 0.85, L1 tolerance
1e-8, a 1000-iteration cap and probability-mode dangling redistribution. Kernel
timings exclude graph/index construction and printing. Score mass and a weighted
checksum are sanity checks, not a substitute for the independent correctness tests.

Build Rust from the repository root:

```sh
cargo build --release --locked --manifest-path rust/Cargo.toml -p icebug-algorithms --example benchmark
```

Build the existing C++ library with CMake Release and `NETWORKIT_NATIVE=OFF`, then
compile `legacy_pagerank.cpp` against that library, Arrow and OpenMP. The local
macOS/Homebrew command, with the CMake build at `rust/legacy-build`, is:

```sh
c++ -std=c++20 -O3 -Iinclude -Iextlibs/tlx -I/opt/homebrew/include \
  -I/opt/homebrew/opt/libomp/include -Xpreprocessor -fopenmp \
  rust/benchmarks/legacy_pagerank.cpp -Lrust/legacy-build -lnetworkit \
  -L/opt/homebrew/lib -larrow -L/opt/homebrew/opt/libomp/lib -lomp \
  -Wl,-rpath,/opt/homebrew/lib -Wl,-rpath,/opt/homebrew/opt/libomp/lib \
  -Wl,-rpath,"$PWD/rust/legacy-build" -o rust/legacy-build/legacy_pagerank
python3 rust/benchmarks/compare.py --legacy rust/legacy-build/legacy_pagerank \
  --rust rust/target/release/examples/benchmark
```

These are initial synthetic measurements, not a representative performance gate.
The Rust tracked-memory number is not RSS, and the reference retains different
construction temporaries. Do not compare them as whole-process memory measurements.
