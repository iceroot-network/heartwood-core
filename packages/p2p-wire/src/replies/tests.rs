use super::*;
use crate::codecs::Amount;
use crate::messages::blocks::BlockHeader;
use crate::messages::peer::Common;

fn peer(ip: Option<&str>, port: Option<u32>) -> Peer {
    Peer {
        ip: ip.map(str::to_owned),
        port,
        ..Peer::default()
    }
}

#[test]
fn peer_addresses_follow_the_schema_formats() {
    for ip in [
        "1.2.3.4",
        "001.002.003.004",
        "255.255.255.255",
        "::",
        "::1",
        "1::",
        "fe80::1%eth0",
        "  ::1  ",
        "::ffff:1.2.3.4",
        "1:2:3:4:5:6:7:8",
        "1:2:3:4:5:6:7::",
        "::2:3:4:5:6:7:8",
        "1:2:3:4:5:6:1.2.3.4",
        "1:2:3:4:5::1.2.3.4",
        "ABCD::EF01",
        "::1%a\n",
    ] {
        assert!(check_peers(&[peer(Some(ip), Some(4002))]).is_ok(), "{ip:?}");
    }
    for ip in [
        "",
        "1.2.3",
        "256.1.1.1",
        "1.2.3.4 ",
        "1.2.3.4.5",
        ":::",
        "1::2::3",
        "1:2:3:4:5:6:7:8:9",
        "1:2:3:4:5:6:7:8::",
        "1:2:3:4:5:6::1.2.3.4",
        "::01.2.3.4",
        "12345::",
        "::1%",
        "::1%\n",
        "::1%a\nb",
        "1.2.3.4::",
    ] {
        assert_eq!(
            check_peers(&[peer(Some(ip), Some(4002))]),
            Err(ReplyRefused::Peer),
            "{ip:?}"
        );
    }
}

#[test]
fn peer_entries_read_their_defaults() {
    // A missing port is the message's default, 0; a missing address is the empty text.
    let checked = check_peers(&[peer(Some("1.2.3.4"), None)]).unwrap();
    assert_eq!(
        checked,
        [PeerAddress {
            ip: "1.2.3.4".into(),
            port: 0
        }]
    );
    assert_eq!(check_peers(&[peer(None, Some(1))]), Err(ReplyRefused::Peer));
    assert_eq!(
        check_peers(&[peer(Some("1.2.3.4"), Some(65_536))]),
        Err(ReplyRefused::Peer)
    );
    let many = vec![peer(Some("1.2.3.4"), Some(1)); MAX_PEERS];
    assert_eq!(check_peers(&many).unwrap().len(), MAX_PEERS);
    let too_many = vec![peer(Some("x"), Some(1)); MAX_PEERS + 1];
    assert_eq!(check_peers(&too_many), Err(ReplyRefused::Root));
}

#[test]
fn common_blocks_are_null_or_a_block() {
    assert_eq!(
        check_common_blocks(&GetCommonBlocksResponse::default()),
        Ok(None)
    );
    let reply = |height: Option<u32>, id: Option<&str>| GetCommonBlocksResponse {
        common: Some(Common {
            height,
            id: id.map(str::to_owned),
            ..Common::default()
        }),
        ..GetCommonBlocksResponse::default()
    };
    let id = "AB".repeat(32);
    assert_eq!(
        check_common_blocks(&reply(Some(7), Some(&id))),
        Ok(Some(CommonBlock {
            height: 7,
            id: id.clone()
        }))
    );
    for refused in [
        reply(Some(0), Some(&id)),
        reply(None, Some(&id)),
        reply(Some(7), None),
        reply(Some(7), Some(&id[1..])),
        reply(Some(1), Some("")),
    ] {
        assert_eq!(check_common_blocks(&refused), Err(ReplyRefused::Common));
    }
}

fn listed(height: u32, key: &str) -> ListedBlock {
    ListedBlock {
        header: BlockHeader {
            id: Some("ab".repeat(32)),
            timestamp: Some(8),
            previous_block: Some("cd".repeat(32)),
            height: Some(height),
            generator_public_key: Some(key.to_owned()),
            block_signature: Some("ef".repeat(64)),
            ..BlockHeader::default()
        },
        reward: Amount::new(200_000_000),
        ..ListedBlock::default()
    }
}

#[test]
fn listed_blocks_pass_with_lowercased_keys() {
    let key = format!("02{}", "AB".repeat(32));
    let mut blocks = vec![listed(2, &key), listed(3, &key)];
    check_blocks(&mut blocks).unwrap();
    for block in &blocks {
        assert_eq!(
            block.header.generator_public_key.as_deref(),
            Some(key.to_ascii_lowercase().as_str())
        );
    }
    let mut blocks = vec![listed(2, &key); MAX_BLOCKS + 1];
    assert_eq!(check_blocks(&mut blocks), Err(ReplyRefused::Root));
}

#[test]
fn a_listed_block_that_breaks_the_header_schema_is_refused() {
    let key = format!("02{}", "AB".repeat(32));
    let mut first_passes = vec![listed(2, &key), listed(0, &key)];
    assert_eq!(check_blocks(&mut first_passes), Err(ReplyRefused::Header));
    // The first block was changed before the second was refused.
    assert_eq!(
        first_passes[0].header.generator_public_key.as_deref(),
        Some(key.to_ascii_lowercase().as_str())
    );
    let mut negative = vec![listed(2, &key)];
    negative[0].total_fee = Amount::new(-1);
    assert_eq!(check_blocks(&mut negative), Err(ReplyRefused::Header));
    let mut wide = vec![listed(2, &key)];
    wide[0].total_amount = Amount::new(i128::from(i64::MAX) + 1);
    assert_eq!(check_blocks(&mut wide), Err(ReplyRefused::Header));
    // At height 1 the totals are unbounded and the previous block may read as 0.
    let mut genesis = vec![listed(1, &key)];
    genesis[0].total_amount = Amount::new(i128::from(i64::MAX) + 1);
    genesis[0].header.previous_block = Some(String::new());
    assert_eq!(check_blocks(&mut genesis), Ok(()));
}

#[test]
fn post_block_replies_need_a_height() {
    let reply = |status: Option<bool>, height: Option<u32>| PostBlockResponse {
        status,
        height,
        ..PostBlockResponse::default()
    };
    assert_eq!(check_post_block(&reply(Some(true), Some(9))), Ok(Some(9)));
    assert_eq!(check_post_block(&reply(None, Some(1))), Ok(Some(1)));
    for refused in [reply(Some(true), None), reply(Some(false), Some(0))] {
        assert_eq!(check_post_block(&refused), Err(ReplyRefused::PostBlock));
    }
}

#[test]
fn unconfirmed_replies_are_capped() {
    let reply = |count: usize| UnconfirmedReply {
        transactions: vec![vec![0xff]; count],
        ..UnconfirmedReply::default()
    };
    assert_eq!(check_unconfirmed(&reply(0), 150), Ok(()));
    assert_eq!(check_unconfirmed(&reply(150), 150), Ok(()));
    assert_eq!(
        check_unconfirmed(&reply(151), 150),
        Err(ReplyRefused::Unconfirmed)
    );
    assert_eq!(check_post_transactions(&["ab".into()]), Ok(()));
}

#[test]
fn the_selector_order_is_fixed() {
    assert_eq!(ROUTES[0], RouteId::GetPeers);
    assert_eq!(ROUTES[5], RouteId::PostTransactions);
}
