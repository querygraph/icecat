/*
 * RangeMinimumQuery.cpp
 *
 *  Created on: 06.05.2026
 *      Author: Ian Chen (ianchen3@illinois.edu)
 */

#include <bit>
#include <cstddef>
#include <networkit/Globals.hpp>
#include <networkit/auxiliary/RangeMinimumQuery.hpp>

namespace Aux {

using NetworKit::omp_index;

RangeMinimumQuery::RangeMinimumQuery(const std::vector<int64_t> &data)
    : data(&data), n(data.size()), logn(0) {
    if (n == 0)
        return;

    logn = std::bit_width(n);
    st.resize(n * logn);
    auto idx = [&](std::size_t i, std::size_t j) { return j * n + i; };

#pragma omp parallel for
    for (omp_index i = 0; i < static_cast<omp_index>(n); ++i)
        st[idx(i, 0)] = i;

    for (std::size_t j = 1; j < logn; ++j) {
        std::size_t range_len = 1ULL << (j - 1);
        index bound = n - (1ULL << j) + 1;

#pragma omp parallel for
        for (omp_index i = 0; i < static_cast<omp_index>(bound); ++i) {
            std::size_t left_idx = st[idx(i, j - 1)];
            std::size_t right_idx = st[idx(i + range_len, j - 1)];

            if (data[left_idx] <= data[right_idx]) {
                st[idx(i, j)] = left_idx;
            } else {
                st[idx(i, j)] = right_idx;
            }
        }
    }
}

index RangeMinimumQuery::Query(index leftRange, index rightRange) {
    if (leftRange >= rightRange)
        return NetworKit::none;

    auto idx = [&](std::size_t i, std::size_t j) { return j * n + i; };
    index j = std::bit_width(rightRange - leftRange) - 1;
    index range_len = 1ULL << j;

    index left_idx = st[idx(leftRange, j)];
    index right_idx = st[idx(rightRange - range_len, j)];

    if ((*data)[left_idx] <= (*data)[right_idx])
        return left_idx;
    return right_idx;
}

} // namespace Aux
