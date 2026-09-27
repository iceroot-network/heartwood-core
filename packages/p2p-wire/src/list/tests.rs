use super::*;

/// `n` items of one byte each (the item's index, modulo 256).
fn items(n: usize) -> Vec<Vec<u8>> {
    (0..n).map(|i| vec![i as u8]).collect()
}

#[test]
fn a_list_round_trips_with_a_big_endian_length_before_each_item() {
    let list = vec![vec![0xaa, 0xbb], vec![], vec![0xcc]];
    let bytes = encode(&list);
    assert_eq!(hex::encode(&bytes), "00000002aabb0000000000000001cc");
    assert_eq!(decode(&bytes, MAX_TRANSACTIONS), list);
    assert!(encode::<Vec<u8>>(&[]).is_empty());
}

#[test]
fn four_bytes_or_fewer_are_an_empty_list() {
    for length in 0..=4 {
        assert!(decode(&vec![0; length], MAX_BLOCKS).is_empty(), "{length}");
    }
    assert!(decode(&[0, 0, 0, 1], MAX_BLOCKS).is_empty());
}

#[test]
fn a_final_zero_length_item_is_dropped_and_a_middle_one_kept() {
    // A final item of length 0 is exactly the last four bytes: the loop stops before it.
    assert_eq!(decode(&encode(&[vec![1u8], vec![]]), 10), vec![vec![1u8]]);
    assert_eq!(
        decode(&encode(&[vec![1u8], vec![], vec![2]]), 10),
        vec![vec![1u8], vec![], vec![2]]
    );
    // Only the last one of several goes.
    assert_eq!(
        decode(&encode(&[vec![1u8], vec![], vec![]]), 10),
        vec![vec![1u8], vec![]]
    );
    // A zero-length item followed by 1 to 3 bytes is read, and the bytes are dropped.
    assert_eq!(decode(&[0, 0, 0, 0, 9], 10), vec![Vec::<u8>::new()]);
}

#[test]
fn one_to_three_trailing_bytes_are_dropped() {
    let list = items(3);
    for trailing in 1..=3 {
        let mut bytes = encode(&list);
        bytes.extend(std::iter::repeat_n(0xff, trailing));
        assert_eq!(decode(&bytes, 10), list, "{trailing} trailing bytes");
    }
    // Five trailing bytes are a length and a cut item.
    let mut bytes = encode(&list);
    bytes.extend([0, 0, 0, 9, 0xee]);
    let mut longer = list.clone();
    longer.push(vec![0xee]);
    assert_eq!(decode(&bytes, 10), longer);
}

#[test]
fn an_item_that_overruns_the_buffer_is_cut_and_ends_the_list() {
    assert_eq!(decode(&[0, 0, 0, 3, 1, 2], 10), vec![vec![1u8, 2]]);
    assert_eq!(
        decode(&[0xff, 0xff, 0xff, 0xff, 7], 10),
        vec![vec![7u8]],
        "a length of 2^32 - 1"
    );
    let mut bytes = encode(&items(2));
    bytes.extend([0, 0, 1, 0, 0xab, 0xcd]);
    assert_eq!(decode(&bytes, 10).last(), Some(&vec![0xab, 0xcd]));
}

#[test]
fn a_list_holds_at_most_one_item_more_than_its_cap() {
    for cap in [
        MAX_BLOCKS,
        MAX_BLOCK_TRANSACTIONS,
        MAX_TRANSACTIONS,
        MAX_EXCLUSIONS,
    ] {
        for n in [cap - 1, cap, cap + 1, cap + 2, cap + 10] {
            let list = items(n);
            let decoded = decode(&encode(&list), cap);
            assert_eq!(decoded.len(), n.min(cap + 1), "cap {cap}, {n} items");
            assert_eq!(decoded[..], list[..decoded.len()]);
        }
    }
    assert_eq!(decode(&encode(&items(3)), 0).len(), 1);
}

#[test]
fn exclude_lists_are_whole_ids_or_nothing() {
    let ids: Vec<[u8; 32]> = (0..3u8).map(|i| [i; 32]).collect();
    let bytes = ids.as_flattened();
    assert_eq!(decode_exclude(bytes), ids);
    assert!(decode_exclude(&[]).is_empty());
    for cut in [1, 31, 33, 95] {
        assert!(decode_exclude(&bytes[..cut]).is_empty(), "{cut} bytes");
    }
    let mut longer = bytes.to_vec();
    longer.push(0);
    assert!(decode_exclude(&longer).is_empty());
}

#[test]
fn an_exclude_list_holds_at_most_15001_ids() {
    for n in [MAX_EXCLUSIONS, MAX_EXCLUSIONS + 1, MAX_EXCLUSIONS + 2] {
        let bytes: Vec<u8> = (0..n).flat_map(|i| [(i % 251) as u8; 32]).collect();
        let ids = decode_exclude(&bytes);
        assert_eq!(ids.len(), n.min(MAX_EXCLUSIONS + 1), "{n} ids");
        assert_eq!(ids.last(), Some(&[((ids.len() - 1) % 251) as u8; 32]));
    }
}
