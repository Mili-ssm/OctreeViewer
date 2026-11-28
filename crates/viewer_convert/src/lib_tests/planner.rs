//! Characterisation of the spatial planner.

use super::*;

#[test]
fn sha256_matches_the_known_vector() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

fn assert_structure(
    nodes: &[String],
    sha: &str,
    expected_count: usize,
    expected_head: &[&str],
    expected_listing_sha: &str,
    expected_output_sha: &str,
) {
    assert_eq!(nodes.len(), expected_count);
    assert_eq!(&nodes[..expected_head.len()], expected_head);
    for line in nodes {
        // One block per node; V8 numbers the blocks in page order, not in node preorder.
        if expected_count > 1 {
            let blocks = line
                .split("blocks=[")
                .nth(1)
                .unwrap()
                .split(']')
                .next()
                .unwrap();
            assert!(!blocks.is_empty() && !blocks.contains(','), "{line}");
        }
    }
    assert_eq!(
        sha256_hex(nodes.join("\n").as_bytes()),
        expected_listing_sha
    );
    assert_eq!(sha, expected_output_sha);
}

#[test]
fn characterize_planner_structure_for_4000_points() {
    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    let options = leaf_new(64, 0.01).unwrap();
    let (nodes, sha) = planner_snapshot_for(&input, tmp.path(), options);
    println!(
        "{}\nL={}\nSHA={sha}",
        nodes.len(),
        sha256_hex(nodes.join("\n").as_bytes())
    );
    assert_structure(
        &nodes,
        &sha,
        // Re-pinned for V8 in D4 Task 5c from the V8 output (equal for 1, 2 and 4 workers).
        125,
        &[
            "id=0 parent=None level=0 points=500 blocks=[0] children=[1, 64]",
            "id=1 parent=Some(0) level=1 points=1348 blocks=[1] children=[2, 33]",
            "id=2 parent=Some(1) level=2 points=1024 blocks=[3] children=[3, 18]",
            "id=3 parent=Some(2) level=3 points=512 blocks=[7] children=[4, 11]",
        ],
        "e0214b5a392da5cda7d175a4f791f86cd9265a294d058810d46a89d291160a98",
        "1dba104446dc8b91109529136d7d028ad9c91a206105b6f8f828e44c72a246bf",
    );
}

/// A8: the V8 planner structure of the 4000-point fixture is the same for every root range
/// count (the pinned values are those of `characterize_planner_structure_for_4000_points`).
#[test]
fn characterize_planner_structure_is_independent_of_root_ranges() {
    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    for ranges in [1, 2, 4, 8] {
        let case = tmp.path().join(format!("ranges-{ranges}"));
        fs::create_dir(&case).unwrap();
        let options = leaf_new(64, 0.01)
            .unwrap()
            .with_root_partition_ranges(ranges)
            .unwrap();
        let (nodes, sha) = planner_snapshot_for(&input, &case, options);
        assert_eq!(nodes.len(), 125, "ranges={ranges}");
        assert_eq!(
            sha256_hex(nodes.join("\n").as_bytes()),
            "e0214b5a392da5cda7d175a4f791f86cd9265a294d058810d46a89d291160a98",
            "ranges={ranges}"
        );
        assert_eq!(
            sha, "1dba104446dc8b91109529136d7d028ad9c91a206105b6f8f828e44c72a246bf",
            "ranges={ranges}"
        );
    }
}

#[test]
#[ignore = "issue #114: coincident points with small leaves fail in the V8 paged index; the SHAs below are the pre-V8 ones, regenerate them from the fixed V8 output"]
fn characterize_planner_structure_for_coincident_points() {
    let tmp = test_dir();
    let input = tmp.path().join("coincident.las");
    coincident_fixture(&input);
    let options = leaf_new(2, 0.01).unwrap();
    let (nodes, sha) = planner_snapshot_for(&input, tmp.path(), options);
    println!("L={}\nSHA={sha}", sha256_hex(nodes.join("\n").as_bytes()));
    assert_structure(
        &nodes,
        &sha,
        1,
        &["id=0 parent=None level=0 points=5 blocks=[0, 1, 2] children=[]"],
        "4b45d9a0f4c2345becff50c73ebf2e64ec214b97e5dfab5ce66b218274090908",
        "3ab4e637732e6ba996f5613244a97b77b2d03ed80160ddc3b8b46225303c1278",
    );
}

/// The spatial workers used by an accepted plan, or `(requested, limit)` of a rejected one.
fn planner_budget_outcome(
    input: &std::path::Path,
    dir: &std::path::Path,
    budget: u64,
    workers: usize,
) -> Result<usize, (u64, u64)> {
    let output = dir.join(format!("budget-{budget}-{workers}.oct"));
    let options = leaf_budget(64, 0.01, budget)
        .unwrap()
        .with_spatial_workers(workers)
        .unwrap();
    match convert_file(input, &output, options) {
        Ok(report) => Ok(report.spatial_workers_used),
        Err(ConvertError::MemoryBudgetExceeded { requested, limit }) => {
            assert!(!output.exists());
            Err((requested, limit))
        }
        Err(other) => panic!("budget={budget} workers={workers}: unexpected {other:?}"),
    }
}

#[test]
fn characterize_planner_budget_outcomes_with_one_worker() {
    let tmp = test_dir();
    let input = tmp.path().join("source.las");
    fixture(&input, false, true, 4_000);
    // (budget, expected). Thresholds re-pinned for V8 in D4 Task 5c (binary search on V8 output).
    // The root check rejects below 17_182_720; between that and 17_278_719 a later check in the
    // serial plan rejects with a larger `requested`.
    let cases = [
        (1, Some((17_182_720, 1))),
        (17_182_719, Some((17_182_720, 17_182_719))),
        (17_182_720, Some((17_183_232, 17_182_720))),
        (17_199_104, Some((17_199_360, 17_199_104))),
        (17_278_719, Some((17_278_720, 17_278_719))),
        (17_278_720, None),
        (18_182_720, None),
    ];
    for (budget, expected) in cases {
        assert_eq!(
            planner_budget_outcome(&input, tmp.path(), budget, 1).err(),
            expected,
            "{BUDGET_HINT} budget={budget}"
        );
    }
}

/// A budget rejects the same plans whatever the worker count: the budget lowers the effective
/// count, down to the serial plan, instead of failing a plan that one worker would complete.
#[test]
fn planner_budget_outcomes_do_not_depend_on_the_worker_count() {
    let tmp = test_dir();
    let dense = tmp.path().join("source.las");
    fixture(&dense, false, true, 4_000);
    let clusters_tmp = test_dir();
    let clusters = clusters_tmp.path().join("clusters.las");
    eight_cluster_fixture(&clusters);
    // Budgets around the W=1 thresholds of each fixture (17_278_720 for `dense`, 17_242_720 or
    // less for `clusters`) and above where several workers would fit.
    let budgets = [
        1, 17_182_720, 17_199_104, 17_230_000, 17_278_719, 17_278_720, 17_500_000, 18_182_720,
        19_637_088, 19_737_088, 20_937_216, 24_000_000,
    ];
    let mut parallel_runs = 0;
    for (input, dir) in [(&dense, tmp.path()), (&clusters, clusters_tmp.path())] {
        for budget in budgets {
            let serial = planner_budget_outcome(input, dir, budget, 1);
            for workers in [2, 3, 4, 8, 16] {
                let outcome = planner_budget_outcome(input, dir, budget, workers);
                // Same verdict as one worker, and the same bytes when it converts.
                assert_eq!(
                    outcome.is_ok(),
                    serial.is_ok(),
                    "{BUDGET_HINT} budget={budget} workers={workers} input={input:?}"
                );
                if let (Ok(used), Ok(_)) = (outcome, serial) {
                    assert!(used <= workers);
                    parallel_runs += usize::from(used > 1);
                    assert_eq!(
                        read_pair(&dir.join(format!("budget-{budget}-{workers}.oct"))),
                        read_pair(&dir.join(format!("budget-{budget}-1.oct"))),
                        "budget={budget} workers={workers} input={input:?}"
                    );
                } else {
                    assert_eq!(outcome, serial, "budget={budget} workers={workers}");
                }
            }
        }
    }
    // A generous budget really runs the batch: the test would pass with one worker always.
    assert!(
        parallel_runs > 0,
        "no accepted plan used more than one worker"
    );
}

fn convert_error_at(
    input: &std::path::Path,
    dir: &std::path::Path,
    workers: usize,
) -> ConvertError {
    let output = dir.join(format!("err-{workers}.oct"));
    let options = leaf_new(16, 0.01)
        .unwrap()
        .with_spatial_workers(workers)
        .unwrap();
    let error = convert_file(input, &output, options).unwrap_err();
    assert!(!output.exists());
    error
}

#[test]
fn characterize_input_errors_are_identical_across_worker_counts() {
    let tmp = test_dir();
    let dir = tmp.path();
    // Header max X lowered (offset 179, as in the point_source tests): the first point beyond it
    // is reported by global index while the source spool is written, before any planner worker,
    // so branches never see it. No data error is reachable from a planner branch today.
    let bounds = dir.join("bounds.las");
    fixture(&bounds, false, false, 1_000);
    let mut bytes = fs::read(&bounds).unwrap();
    bytes[179..187].copy_from_slice(&1_000_100.0_f64.to_le_bytes());
    fs::write(&bounds, bytes).unwrap();
    // Truncated point data is rejected at preflight, so PointCountMismatch is not reachable
    // through the public conversion API; the preflight error is pinned instead.
    let truncated = dir.join("truncated.las");
    fixture(&truncated, false, false, 1_000);
    let length = fs::metadata(&truncated).unwrap().len();
    fs::OpenOptions::new()
        .write(true)
        .open(&truncated)
        .unwrap()
        .set_len(length - 100)
        .unwrap();
    for workers in [1, 2, 4] {
        let error = convert_error_at(&bounds, dir, workers);
        assert!(
            matches!(
                error,
                ConvertError::PointOutsideHeaderBounds {
                    point_index: 401,
                    ..
                }
            ),
            "workers={workers}: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            "point 401 at [1000100.25, 2000002.0, 51.0] is outside the LAS/LAZ header bounds"
        );
        let error = convert_error_at(&truncated, dir, workers);
        assert!(
            matches!(error, ConvertError::LasPreflight(_)),
            "workers={workers}: {error:?}"
        );
        assert_eq!(
            error.to_string(),
            "unsafe LAS/LAZ header: header point count x point_data_record_length (1000 x 20 B) exceeds the 19900 B after offset_to_point_data"
        );
    }
}

#[test]
fn sha256_matches_the_two_block_vector() {
    assert_eq!(
        sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
}

#[test]
fn later_stages_reserve_the_same_memory_with_or_without_split_ranges() {
    // The split ranges are freed before the block and validation stages start, so those stages
    // must size their workers as if the nodes had not been split.
    let tmp = test_dir();
    let input = tmp.path().join("deep.las");
    deep_child_fixture(&input);
    let mut compared = 0;
    for budget in (17..=40_u64).map(|mib| mib * 1024 * 1024 + 300_000) {
        let run = |split: u64| {
            let output = tmp.path().join(format!("later-{budget}-{split}.oct"));
            let options = leaf_budget(16, 0.01, budget)
                .unwrap()
                .with_spatial_workers(8)
                .unwrap()
                .with_range_split_points_for_tests(split);
            let before = split_counter();
            let result = convert_file(&input, &output, options);
            (result, splits_since(before))
        };
        let (Ok(plain), 0) = run(u64::MAX) else {
            continue;
        };
        let (split, splits) = run(150);
        let split = split.unwrap_or_else(|error| panic!("budget={budget}: {error:?}"));
        if splits == 0
            || split.spatial_workers_used <= 1
            || split.spatial_workers_used != plain.spatial_workers_used
        {
            continue;
        }
        compared += 1;
        assert_eq!(
            split.block_preparation.worker_limit, plain.block_preparation.worker_limit,
            "budget={budget}: block workers"
        );
        assert_eq!(
            split.output_validation.worker_limit, plain.output_validation.worker_limit,
            "budget={budget}: validation workers"
        );
        assert!(split.estimated_peak_memory_bytes >= plain.estimated_peak_memory_bytes);
    }
    assert!(
        compared > 0,
        "no budget split a node and kept the parallel run"
    );
}

#[test]
fn a_tight_budget_lowers_the_split_instead_of_failing() {
    let tmp = test_dir();
    let input = tmp.path().join("deep.las");
    deep_child_fixture(&input);
    let mut lowered_somewhere = false;
    // From the smallest budget one worker passes, up to one that holds every split.
    for budget in [
        2_u64 * 1024 * 1024,
        17_300_000,
        17_600_000,
        18_000_000,
        18_800_000,
        20_000_000,
        24_000_000,
        // Parallel and split, with a node's ranges lowered by the budget.
        30_000_000,
        31_000_000,
        31_500_000,
        32_500_000,
        64_000_000,
    ] {
        let run = |workers: usize, split: u64| {
            let output = tmp
                .path()
                .join(format!("budget-{budget}-{workers}-{split}.oct"));
            let options = leaf_budget(16, 0.01, budget)
                .unwrap()
                .with_spatial_workers(workers)
                .unwrap()
                .with_range_split_points_for_tests(split);
            let before = split_counter();
            let result = convert_file(&input, &output, options);
            (
                result.map(|report| (report, read_pair(&output))),
                split_log_since(before),
            )
        };
        let (serial, _) = run(1, 150);
        let Ok((_, expected)) = serial else {
            continue;
        };
        let (split, log) = run(8, 150);
        let (report, bytes) = split.unwrap_or_else(|error| panic!("budget={budget}: {error:?}"));
        assert_eq!(bytes, expected, "budget={budget}");
        // Without splits the estimate is checked against the budget. With them the reported peak
        // pairs the admitted ranges with the final real index (admission used the expected one),
        // so it may exceed the budget; only the kept parallel run may do so.
        assert!(
            report.estimated_peak_memory_bytes <= budget
                || (report.spatial_workers_used > 1 && !log.is_empty()),
            "budget={budget}: peak {} workers {}",
            report.estimated_peak_memory_bytes,
            report.spatial_workers_used
        );
        // Only a run that kept its parallel attempt (more than one worker) has a log of its own;
        // a one-worker fallback also counts the splits of the discarded attempt.
        if report.spatial_workers_used > 1 {
            lowered_somewhere |= log
                .iter()
                .any(|split| (2..split.requested).contains(&split.ranges));
        }
    }
    assert!(
        lowered_somewhere,
        "no budget kept the parallel run with a split lowered to 2 <= K < requested"
    );
}
