"""Compare prebuilt serial PageRank fixtures; report medians, check summary agreement."""
import argparse
import csv
import io
import math
import statistics
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--legacy", required=True)
    parser.add_argument("--rust", required=True)
    parser.add_argument("--nodes", type=int, nargs="+", default=[100000, 1000000])
    parser.add_argument("--repeats", type=int, default=5)
    args = parser.parse_args()
    if args.repeats < 1 or any(n < 3 for n in args.nodes):
        parser.error("repeats must be positive and node counts at least three")
    print("| Nodes | C++ median ms | Rust median ms | Rust / C++ | Iterations |")
    print("| --- | --- | --- | --- | --- |")
    for n in args.nodes:
        samples = [[], []]
        # Alternate executables to reduce ordering effects. Each process constructs its input.
        for _ in range(args.repeats):
            for bucket, executable in zip(samples, [args.legacy, args.rust]):
                output = subprocess.check_output([executable, str(n)], text=True)
                bucket.append(next(csv.DictReader(io.StringIO(output))))
        reference = samples[0][0]
        for row in samples[0] + samples[1]:
            for key in ("nodes", "edges", "iterations"):
                if row[key] != reference[key]:
                    raise RuntimeError(f"{key} differs: {row[key]} vs {reference[key]}")
            for key in ("sum", "checksum"):
                if not math.isclose(float(row[key]), float(reference[key]), rel_tol=1e-11):
                    raise RuntimeError(f"{key} differs from reference")
        cpp, rust = [statistics.median(float(s["pagerank_ms"]) for s in group)
                     for group in samples]
        print(f"| {n} | {cpp:.3f} | {rust:.3f} | {rust / cpp:.3f} | {reference['iterations']} |")


if __name__ == "__main__":
    main()
