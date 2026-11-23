//! Test-only characterization of whole-assignment edge-exclusion regret.
//!
//! This file is deliberately unwired from production. It reconstructs the raw padded assignment
//! problem from the public GNN result, computes the cheapest alternating cycle that removes each
//! selected real edge, and checks that result against an independent exhaustive enumerator. It
//! does not choose a safety threshold or change association behavior.

#![allow(clippy::indexing_slicing, clippy::panic)]

use oa_track::assoc::{
    assoc_plot_track, assoc_scratch_new, gnn, objective, Pair, MAX_COMPONENT, NONE,
};

const INF: f64 = f64::INFINITY;
const TOL: f64 = 1.0e-9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegretError {
    BadShape,
    UnsupportedSize,
    BudgetExhausted,
}

#[derive(Debug, PartialEq)]
struct RegretResult {
    by_row: Vec<Option<f64>>,
    work: usize,
}

#[derive(Debug)]
struct Problem {
    n_tracks: usize,
    n_plots: usize,
    gate: f64,
    matrix: Vec<f64>,
    matching: Vec<usize>,
    selected_rows: Vec<usize>,
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOL * (1.0 + a.abs().max(b.abs()))
}

fn padded_matrix(n_tracks: usize, n_plots: usize, pairs: &[Pair], gate: f64) -> Vec<f64> {
    let n = n_tracks + n_plots;
    let mut matrix = vec![INF; n * n];
    for pair in pairs {
        let i = pair.track as usize;
        let j = pair.plot as usize;
        if i < n_tracks && j < n_plots && pair.cost.is_finite() {
            let slot = &mut matrix[i * n + j];
            *slot = slot.min(pair.cost);
        }
    }
    let penalty = 0.5 * gate;
    for i in 0..n_tracks {
        matrix[i * n + n_plots + i] = penalty;
    }
    for j in 0..n_plots {
        matrix[(n_tracks + j) * n + j] = penalty;
    }
    for i in n_tracks..n {
        for j in n_plots..n {
            matrix[i * n + j] = 0.0;
        }
    }
    matrix
}

fn reconstruct_matching(
    n_tracks: usize,
    n_plots: usize,
    plot_track: &[u32],
) -> Result<(Vec<usize>, Vec<usize>), RegretError> {
    let n = n_tracks + n_plots;
    if plot_track.len() != n_plots {
        return Err(RegretError::BadShape);
    }
    let mut matching = vec![usize::MAX; n];
    let mut selected_rows = Vec::with_capacity(n_plots);
    for (j, &track) in plot_track.iter().enumerate() {
        if track == NONE {
            matching[n_tracks + j] = j;
            continue;
        }
        let i = track as usize;
        if i >= n_tracks || matching[i] != usize::MAX {
            return Err(RegretError::BadShape);
        }
        matching[i] = j;
        matching[n_tracks + j] = n_plots + i;
        selected_rows.push(i);
    }
    for (i, column) in matching.iter_mut().take(n_tracks).enumerate() {
        if *column == usize::MAX {
            *column = n_plots + i;
        }
    }
    let mut used = vec![false; n];
    for &column in &matching {
        if column >= n || used[column] {
            return Err(RegretError::BadShape);
        }
        used[column] = true;
    }
    Ok((matching, selected_rows))
}

fn production_problem(n_tracks: usize, n_plots: usize, pairs: &[Pair], gate: f64) -> Problem {
    let mut scratch = assoc_scratch_new(n_tracks, n_plots, pairs.len());
    let stats = gnn(&mut scratch, n_tracks, n_plots, pairs, gate);
    assert!(
        stats.is_ok(),
        "production GNN refused a small valid problem"
    );
    let (matching, selected_rows) =
        reconstruct_matching(n_tracks, n_plots, assoc_plot_track(&scratch))
            .unwrap_or_else(|error| panic!("invalid production matching: {error:?}"));
    Problem {
        n_tracks,
        n_plots,
        gate,
        matrix: padded_matrix(n_tracks, n_plots, pairs, gate),
        matching,
        selected_rows,
    }
}

fn matching_cost(matrix: &[f64], matching: &[usize]) -> Result<f64, RegretError> {
    let n = matching.len();
    if matrix.len() != n * n {
        return Err(RegretError::BadShape);
    }
    let mut sum = 0.0;
    for (row, &column) in matching.iter().enumerate() {
        if column >= n {
            return Err(RegretError::BadShape);
        }
        let cost = matrix[row * n + column];
        if !cost.is_finite() {
            return Err(RegretError::BadShape);
        }
        sum += cost;
    }
    Ok(sum)
}

fn charge(work: &mut usize, limit: usize) -> Result<(), RegretError> {
    if *work == limit {
        return Err(RegretError::BudgetExhausted);
    }
    *work += 1;
    Ok(())
}

/// Returns the increase in the padded objective when each selected row's current edge is
/// forbidden. The alternating graph has one vertex per matched row/column pair. An arc `r -> k`
/// moves row `r` to the column currently owned by row `k`; a cycle is another perfect matching.
fn alternating_cycle_regret(
    matrix: &[f64],
    matching: &[usize],
    selected_rows: &[usize],
    limit: usize,
) -> Result<RegretResult, RegretError> {
    let n = matching.len();
    if n == 0 || n > MAX_COMPONENT || matrix.len() != n * n {
        return Err(if n > MAX_COMPONENT {
            RegretError::UnsupportedSize
        } else {
            RegretError::BadShape
        });
    }
    let mut used = vec![false; n];
    for &column in matching {
        if column >= n || used[column] {
            return Err(RegretError::BadShape);
        }
        used[column] = true;
    }

    let mut work = 0usize;
    let mut arc = vec![INF; n * n];
    for row in 0..n {
        let own = matrix[row * n + matching[row]];
        if !own.is_finite() {
            return Err(RegretError::BadShape);
        }
        for target in 0..n {
            charge(&mut work, limit)?;
            let cost = matrix[row * n + matching[target]];
            if row != target && cost.is_finite() {
                arc[row * n + target] = cost - own;
            }
        }
    }

    let mut dist = arc.clone();
    for i in 0..n {
        dist[i * n + i] = 0.0;
    }
    for mid in 0..n {
        for from in 0..n {
            for to in 0..n {
                charge(&mut work, limit)?;
                let through = dist[from * n + mid] + dist[mid * n + to];
                if through < dist[from * n + to] {
                    dist[from * n + to] = through;
                }
            }
        }
    }
    for i in 0..n {
        charge(&mut work, limit)?;
        if dist[i * n + i] < -TOL {
            return Err(RegretError::BadShape);
        }
    }

    let mut by_row = vec![None; n];
    for &row in selected_rows {
        if row >= n {
            return Err(RegretError::BadShape);
        }
        let mut best = INF;
        for next in 0..n {
            if next == row {
                continue;
            }
            charge(&mut work, limit)?;
            best = best.min(arc[row * n + next] + dist[next * n + row]);
        }
        if best.is_finite() {
            // A genuinely optimal matching has no negative alternating cycle. Tolerate only
            // floating-point dust here; a material negative value invalidates the premise.
            if best < -TOL {
                return Err(RegretError::BadShape);
            }
            by_row[row] = Some(best.max(0.0));
        }
    }
    Ok(RegretResult { by_row, work })
}

fn exhaustive_best(matrix: &[f64], forbidden: Option<(usize, usize)>) -> Option<f64> {
    let n = (matrix.len() as f64).sqrt() as usize;
    if n * n != matrix.len() {
        return None;
    }
    fn visit(
        matrix: &[f64],
        n: usize,
        forbidden: Option<(usize, usize)>,
        row: usize,
        used: &mut [bool],
        sum: f64,
        best: &mut f64,
    ) {
        if row == n {
            *best = best.min(sum);
            return;
        }
        for column in 0..n {
            if used[column] || forbidden == Some((row, column)) {
                continue;
            }
            let cost = matrix[row * n + column];
            if !cost.is_finite() {
                continue;
            }
            used[column] = true;
            visit(matrix, n, forbidden, row + 1, used, sum + cost, best);
            used[column] = false;
        }
    }
    let mut used = vec![false; n];
    let mut best = INF;
    visit(matrix, n, forbidden, 0, &mut used, 0.0, &mut best);
    best.is_finite().then_some(best)
}

fn assert_matches_oracle(problem: &Problem, expected: Option<&[f64]>) {
    let base = matching_cost(&problem.matrix, &problem.matching)
        .unwrap_or_else(|error| panic!("invalid reconstructed matching: {error:?}"));
    let n = problem.n_tracks + problem.n_plots;
    let mut canonical_pairs = Vec::new();
    for track in 0..problem.n_tracks {
        for plot in 0..problem.n_plots {
            let cost = problem.matrix[track * n + plot];
            if cost.is_finite() {
                canonical_pairs.push(pair(track as u32, plot as u32, cost));
            }
        }
    }
    let mut plot_track = vec![NONE; problem.n_plots];
    for track in 0..problem.n_tracks {
        let plot = problem.matching[track];
        if plot < problem.n_plots {
            plot_track[plot] = track as u32;
        }
    }
    let public_objective = objective(
        &canonical_pairs,
        problem.n_tracks,
        &plot_track,
        problem.gate,
    );
    assert!(
        close(base, public_objective),
        "padded objective {base} != public objective {public_objective}"
    );
    let oracle_base = exhaustive_best(&problem.matrix, None)
        .unwrap_or_else(|| panic!("padded problem has no perfect matching"));
    assert!(
        close(base, oracle_base),
        "production raw objective {base} != exhaustive optimum {oracle_base}"
    );
    let result = alternating_cycle_regret(
        &problem.matrix,
        &problem.matching,
        &problem.selected_rows,
        usize::MAX,
    )
    .unwrap_or_else(|error| panic!("regret computation failed: {error:?}"));
    for (position, &row) in problem.selected_rows.iter().enumerate() {
        let alternate = exhaustive_best(&problem.matrix, Some((row, problem.matching[row])))
            .unwrap_or_else(|| panic!("edge exclusion removed every perfect matching"));
        let oracle = alternate - oracle_base;
        let actual = result.by_row[row]
            .unwrap_or_else(|| panic!("selected row {row} has no computed regret"));
        assert!(
            close(actual, oracle),
            "row {row}: alternating-cycle regret {actual} != exhaustive regret {oracle}"
        );
        if let Some(values) = expected {
            assert!(
                close(actual, values[position]),
                "row {row}: regret {actual} != expected {}",
                values[position]
            );
        }
    }
}

fn pair(track: u32, plot: u32, cost: f64) -> Pair {
    Pair { track, plot, cost }
}

#[test]
fn hand_cases_cover_ties_unmatched_disconnected_and_rerouting() {
    let gate = 10.0;
    let cases: Vec<(Problem, Vec<f64>)> = vec![
        (
            production_problem(
                2,
                2,
                &[
                    pair(0, 0, 1.0),
                    pair(0, 1, 1.0),
                    pair(1, 0, 1.0),
                    pair(1, 1, 1.0),
                ],
                gate,
            ),
            vec![0.0, 0.0],
        ),
        (
            production_problem(
                2,
                2,
                &[pair(0, 0, 1.0), pair(1, 1, 1.0), pair(1, 0, 1.1)],
                gate,
            ),
            vec![9.0, 9.0],
        ),
        (
            production_problem(1, 1, &[pair(0, 0, 4.9)], gate),
            vec![5.1],
        ),
        (
            production_problem(2, 1, &[pair(0, 0, 6.0)], gate),
            vec![4.0],
        ),
        (
            production_problem(2, 2, &[pair(0, 0, 1.0), pair(1, 1, 2.0)], gate),
            vec![9.0, 8.0],
        ),
        (
            production_problem(
                2,
                2,
                &[
                    pair(0, 0, 1.0),
                    pair(1, 1, 1.0),
                    pair(0, 1, f64::NAN),
                    pair(1, 0, f64::NAN),
                ],
                gate,
            ),
            vec![9.0, 9.0],
        ),
        (
            production_problem(
                2,
                2,
                &[pair(0, 0, 1.0), pair(1, 0, 3.0), pair(0, 1, 2.0)],
                16.0,
            ),
            vec![12.0, 12.0],
        ),
        (
            production_problem(
                3,
                3,
                &[
                    pair(0, 0, 2.0),
                    pair(0, 1, 2.0),
                    pair(0, 2, 2.0),
                    pair(1, 0, 2.0),
                    pair(1, 1, 2.0),
                    pair(1, 2, 2.0),
                    pair(2, 0, 2.0),
                    pair(2, 1, 2.0),
                    pair(2, 2, 2.0),
                ],
                gate,
            ),
            vec![0.0, 0.0, 0.0],
        ),
    ];
    for (problem, expected) in &cases {
        assert_matches_oracle(problem, Some(expected));
    }
}

#[test]
fn seeded_small_graphs_match_independent_exhaustive_enumeration() {
    let mut state = 0x7ac3_61d9_0b25_ef47u64;
    let mut next_u32 = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 32) as u32
    };
    for case in 0..240 {
        let n_tracks = 1 + next_u32() as usize % 3;
        let n_plots = 1 + next_u32() as usize % 3;
        let mut pairs = Vec::new();
        for track in 0..n_tracks {
            for plot in 0..n_plots {
                if next_u32() % 4 != 0 {
                    let cost = 0.25 * f64::from(1 + next_u32() % 35);
                    pairs.push(pair(track as u32, plot as u32, cost));
                    if next_u32() % 5 == 0 {
                        pairs.push(pair(track as u32, plot as u32, cost + 0.75));
                    }
                }
            }
        }
        // Invalid entries must not influence either production or the reconstructed problem.
        pairs.push(pair(n_tracks as u32, 0, 0.0));
        pairs.push(pair(0, n_plots as u32, 0.0));
        pairs.push(pair(0, 0, f64::NAN));
        let problem = production_problem(n_tracks, n_plots, &pairs, 10.0);
        assert_matches_oracle(&problem, None);
        assert!(
            problem.matrix.len() <= 36,
            "case {case} escaped the exhaustive bound"
        );
    }
}

#[test]
fn input_order_and_nonminimal_duplicates_do_not_change_regret() {
    let base = [
        pair(0, 0, 1.0),
        pair(0, 1, 4.0),
        pair(1, 0, 3.0),
        pair(1, 1, 2.0),
        pair(2, 1, 5.0),
        pair(2, 2, 1.5),
    ];
    let reordered = [
        pair(2, 2, 7.0),
        pair(1, 1, 2.0),
        pair(0, 1, 8.0),
        pair(2, 1, 5.0),
        pair(0, 0, 1.0),
        pair(2, 2, 1.5),
        pair(1, 0, 3.0),
        pair(0, 1, 4.0),
    ];
    let a = production_problem(3, 3, &base, 10.0);
    let b = production_problem(3, 3, &reordered, 10.0);
    assert_eq!(a.matching, b.matching);
    let ar = alternating_cycle_regret(&a.matrix, &a.matching, &a.selected_rows, usize::MAX)
        .unwrap_or_else(|error| panic!("base regret failed: {error:?}"));
    let br = alternating_cycle_regret(&b.matrix, &b.matching, &b.selected_rows, usize::MAX)
        .unwrap_or_else(|error| panic!("reordered regret failed: {error:?}"));
    assert_eq!(ar.by_row, br.by_row);
    assert_matches_oracle(&a, None);
    assert_matches_oracle(&b, None);
}

#[test]
fn budget_is_exact_and_failure_releases_no_partial_result() {
    let problem = production_problem(
        3,
        3,
        &[
            pair(0, 0, 1.0),
            pair(0, 1, 3.0),
            pair(1, 0, 2.0),
            pair(1, 1, 1.0),
            pair(1, 2, 4.0),
            pair(2, 1, 2.5),
            pair(2, 2, 1.0),
        ],
        10.0,
    );
    let full = alternating_cycle_regret(
        &problem.matrix,
        &problem.matching,
        &problem.selected_rows,
        usize::MAX,
    )
    .unwrap_or_else(|error| panic!("unbounded regret failed: {error:?}"));
    let exact = alternating_cycle_regret(
        &problem.matrix,
        &problem.matching,
        &problem.selected_rows,
        full.work,
    )
    .unwrap_or_else(|error| panic!("exact budget failed: {error:?}"));
    assert_eq!(exact.work, full.work);
    assert_eq!(exact.by_row, full.by_row);
    assert_eq!(
        alternating_cycle_regret(
            &problem.matrix,
            &problem.matching,
            &problem.selected_rows,
            full.work - 1,
        ),
        Err(RegretError::BudgetExhausted)
    );
}

#[test]
fn oversized_component_is_explicitly_unsupported() {
    let n = MAX_COMPONENT + 1;
    let matrix = vec![0.0; n * n];
    let matching: Vec<usize> = (0..n).collect();
    assert_eq!(
        alternating_cycle_regret(&matrix, &matching, &[0], usize::MAX),
        Err(RegretError::UnsupportedSize)
    );
}
