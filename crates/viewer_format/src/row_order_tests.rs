//! Hybrid row order (ADR 0041/0042): the Morton key and permutation.

#[test]
fn morton_key_covers_all_96_bits_and_ties_break_by_source_row() {
    assert_eq!(super::morton_key([u32::MAX; 3]), (1u128 << 96) - 1);
    assert_eq!(super::morton_key([1, 0, 0]), 0b100);
    assert_eq!(super::morton_key([0, 0, 1]), 1);
    assert_eq!(super::morton_key([1 << 31, 0, 0]), 1u128 << 95);
    assert_eq!(super::morton_key([0, 0, 1 << 31]), 1u128 << 93);
    let coordinates = [[9, 9, 9], [0, 0, 0], [9, 9, 9], [0, 0, 1 << 31]];
    assert_eq!(super::morton_permutation(&coordinates), vec![1, 0, 2, 3]);
}

#[test]
fn morton_key_matches_the_bit_loop_reference() {
    fn reference(coordinate: [u32; 3]) -> u128 {
        let mut key = 0u128;
        for bit in 0..32 {
            for (axis, value) in coordinate.iter().enumerate() {
                key |= u128::from((value >> bit) & 1) << (3 * bit + (2 - axis));
            }
        }
        key
    }
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 32) as u32
    };
    for _ in 0..10_000 {
        let coordinate = [next(), next(), next()];
        assert_eq!(
            super::morton_key(coordinate),
            reference(coordinate),
            "{coordinate:?}"
        );
    }
    for coordinate in [
        [0; 3],
        [u32::MAX; 3],
        [1 << 20, 1 << 21, 1 << 31],
        [0x1f_ffff, 0, 0x7ff << 21],
    ] {
        assert_eq!(
            super::morton_key(coordinate),
            reference(coordinate),
            "{coordinate:?}"
        );
    }
}

#[test]
fn morton_permutation_equals_sorting_by_key_then_row() {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = |modulus: u32| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) as u32) % modulus
    };
    // Narrow ranges give many duplicate keys; the wide case spans the high key bits.
    for (count, modulus, offset) in [
        (0, 1, 0),
        (1, 1, 0),
        (500, 1, 7),
        (5000, 16, 1 << 20),
        (5000, u32::MAX, 0),
    ] {
        let coordinates: Vec<[u32; 3]> = (0..count)
            .map(|_| {
                [
                    offset + next(modulus),
                    offset + next(modulus),
                    offset + next(modulus),
                ]
            })
            .collect();
        let mut expected: Vec<u32> = (0..count as u32).collect();
        expected.sort_by_key(|&row| (super::morton_key(coordinates[row as usize]), row));
        assert_eq!(
            super::morton_permutation(&coordinates),
            expected,
            "count {count} modulus {modulus}"
        );
    }
}
