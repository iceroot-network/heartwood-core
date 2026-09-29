use super::*;

#[test]
fn ids_paths_and_names_agree() {
    for route in RouteId::ALL {
        assert_eq!(RouteId::from_id(route.id()), Some(route));
        assert_eq!(route.path(), format!("/{}", route.id().replace('.', "/")));
        assert!(route.id().ends_with(route.name()));
        assert_eq!(route.kind() == CodecKind::Json, route.max_bytes().is_none());
    }
    assert_eq!(RouteId::GetStatus.name(), "getStatus");
    assert_eq!(RouteId::from_id("/p2p.peer.getStatus"), None);
}

#[test]
fn the_reference_routes_keep_their_order_and_the_attestation_routes_follow() {
    let reference: Vec<&str> = (RouteId::ALL.into_iter())
        .filter(|route| route.is_reference())
        .map(RouteId::id)
        .collect();
    assert_eq!(
        reference,
        [
            "p2p.peer.getPeers",
            "p2p.peer.getCommonBlocks",
            "p2p.peer.getStatus",
            "p2p.blocks.getBlocks",
            "p2p.blocks.postBlock",
            "p2p.transactions.getUnconfirmedTransactions",
            "p2p.transactions.postTransactions",
            "p2p.internal.emitEvent",
            "p2p.internal.getCurrentRound",
            "p2p.internal.getSlotNumber",
            "p2p.internal.getNetworkState",
            "p2p.internal.syncBlockchain",
        ]
    );
    for (route, id) in [
        (RouteId::GetStatusToSign, "p2p.internal.getStatusToSign"),
        (
            RouteId::PostStatusSignatures,
            "p2p.internal.postStatusSignatures",
        ),
    ] {
        assert_eq!(RouteId::from_id(id), Some(route));
        assert!(!route.is_reference());
        assert!(route.is_internal());
        assert_eq!(route.kind(), CodecKind::Json);
        assert_eq!(route.name(), id.rsplit_once('.').unwrap().1);
    }
    assert!(RouteId::ALL[..12].iter().all(|route| route.is_reference()));
}

#[test]
fn every_public_route_has_two_codecs() {
    for route in RouteId::ALL {
        for direction in [Direction::Request, Direction::Response] {
            let codec = Codec::of(route, direction);
            assert_eq!(codec.is_some(), route.kind() == CodecKind::Protobuf);
            if let Some(codec) = codec {
                assert_eq!((codec.route(), codec.direction()), (route, direction));
            }
        }
    }
    for (index, codec) in Codec::ALL.into_iter().enumerate() {
        assert_eq!(codec as usize, index);
    }
    assert_eq!(Codec::GetStatusResponse.route(), RouteId::GetStatus);
    assert_eq!(Codec::GetStatusResponse.direction(), Direction::Response);
    assert_eq!(
        Codec::PostTransactionsRequest.route(),
        RouteId::PostTransactions
    );
    assert_eq!(Codec::of(RouteId::EmitEvent, Direction::Request), None);
    assert_eq!(Codec::select(0), Codec::GetPeersRequest);
    assert_eq!(Codec::select(13), Codec::PostTransactionsResponse);
    assert_eq!(Codec::select(14), Codec::GetPeersRequest);
    assert_eq!(Codec::select(255), Codec::ALL[255 % 14]);
}
