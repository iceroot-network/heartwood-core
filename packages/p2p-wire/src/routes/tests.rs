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
