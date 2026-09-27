use std::net::IpAddr;

use super::{is_valid_peer, process};

#[test]
fn addresses_are_processed_as_ipaddr_js_writes_them() {
    for (text, processed) in [
        ("1.2.3.4", Some("1.2.3.4")),
        ("0x7f.1", Some("127.0.0.1")),
        ("010.0.0.1", Some("8.0.0.1")),
        ("3232235777", Some("192.168.1.1")),
        ("::ffff:1.2.3.4", Some("1.2.3.4")),
        ("::1.2.3.4", Some("1.2.3.4")),
        ("::ffff:102:304", Some("1.2.3.4")),
        ("1:0:2:0:0:0:0:3", Some("1::2:0:0:0:0:3")),
        ("0:0:0:0:0:0:0:0", Some("::")),
        ("2001:DB8::1", Some("2001:db8::1")),
        ("fe80::1%eth0", Some("fe80::1%eth0")),
        ("1:0:0:0:0:0:0:0%x", Some("1::0%x")),
        ("1::1.2.3.4", None),
        ("08.1.1.1", None),
        ("256.1.1.1", None),
        ("1.2.3", Some("1.2.0.3")),
        ("1.2.3.4.5", None),
        ("1.65536", Some("1.1.0.0")),
        ("1.16777216", None),
        ("1.65535", Some("1.0.255.255")),
        ("1:2:3", None),
        ("", None),
        ("fe80::1%", None),
    ] {
        assert_eq!(process(text).as_deref(), processed, "{text:?}");
    }
}

#[test]
fn only_public_addresses_that_are_not_ours_are_valid() {
    let own: Vec<IpAddr> = ["198.51.100.9", "2001:db9::5", "::1.2.3.4"]
        .iter()
        .map(|a| a.parse().unwrap())
        .collect();
    for (text, private, valid, processed) in [
        ("198.51.100.1", false, false, "198.51.100.1"),
        ("8.8.8.8", false, true, "8.8.8.8"),
        ("::ffff:8.8.8.8", false, true, "8.8.8.8"),
        ("10.0.0.1", false, false, "10.0.0.1"),
        ("10.0.0.1", true, true, "10.0.0.1"),
        ("169.254.1.1", true, true, "169.254.1.1"),
        ("fd00::1", true, true, "fd00::1"),
        ("127.0.0.1", true, false, "127.0.0.1"),
        ("100.64.0.1", true, false, "100.64.0.1"),
        ("0.1.2.3", true, false, "0.1.2.3"),
        ("2001:db9::5", false, false, "2001:db9::5"),
        ("2001:db9:0:0:0:0:0:6", false, true, "2001:db9::6"),
        ("::102:304", false, true, "::102:304"),
        ("nonsense", true, false, "nonsense"),
    ] {
        let mut ip = text.to_owned();
        assert_eq!(is_valid_peer(&mut ip, private, &own), valid, "{text}");
        assert_eq!(ip, processed, "{text}");
    }
}
