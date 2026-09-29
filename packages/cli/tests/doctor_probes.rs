//! The fixed probes: the parsers against `timedatectl show` of systemd 249, 252 and 255,
//! `chronyc -n tracking` of chrony 4.2 and 4.5 (written from their documented formats),
//! `systemctl is-active` and `ufw.conf`; and the runner: an absolute path only, `LANG=C` alone,
//! standard input on the null device, output cut at 64 KiB, a hanging program killed at 2 s and
//! reported as unknown, a missing program unknown.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use heartwood_cli::doctor::seams::{Probe, ProbeError, Probes};
use heartwood_cli::doctor::sources::ParseError;
use heartwood_cli::doctor::sources::probes::{
    HostProbes, LeapStatus, NtpState, ServiceState, parse_chrony_tracking, parse_is_active,
    parse_timedatectl, parse_ufw_conf, run_program,
};
use heartwood_cli::doctor::view::{Reason, Unknown};

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doctor/probes");
    fs::read(path.join(name)).unwrap()
}

/// The first of `paths` that exists.
fn program(paths: &[&'static str]) -> &'static Path {
    let found = paths
        .iter()
        .map(|path| Path::new(*path))
        .find(|path| path.exists());
    found.unwrap_or_else(|| panic!("none of {paths:?} is installed"))
}

const LIMITS: (Duration, usize) = (Probe::TIMEOUT, Probe::MAX_OUTPUT);

#[test]
fn timedatectl_of_systemd_249_252_and_255() {
    let state = |synchronized, ntp| NtpState {
        synchronized: Some(synchronized),
        ntp: Some(ntp),
    };
    assert_eq!(
        parse_timedatectl(&fixture("timedatectl-255")),
        Ok(state(true, true))
    );
    assert_eq!(
        parse_timedatectl(&fixture("timedatectl-252")),
        Ok(state(false, true))
    );
    assert_eq!(
        parse_timedatectl(&fixture("timedatectl-249")),
        Ok(state(false, false))
    );
    let reordered = parse_timedatectl(b"NTPSynchronized=yes\nNTP=no\n");
    assert_eq!(reordered, Ok(state(true, false)));
    let one = parse_timedatectl(b"NTPSynchronized=yes\n").unwrap();
    assert_eq!((one.synchronized, one.ntp), (Some(true), None));
    let odd = parse_timedatectl(b"NTP=maybe\nNTPSynchronized=yes\n").unwrap();
    assert_eq!((odd.synchronized, odd.ntp), (Some(true), None));
    for text in [
        &b""[..],
        b"NTP=maybe\n",
        b"Failed to connect to bus\n",
        &[0xff, 0xfe],
    ] {
        assert!(parse_timedatectl(text).is_err(), "{text:?}");
    }
}

#[test]
fn chronyc_tracking_of_chrony_4_2_and_4_5() {
    let tracking = parse_chrony_tracking(&fixture("chronyc-4.2")).unwrap();
    assert_eq!(
        (tracking.offset_us, tracking.leap),
        (-7, LeapStatus::Normal)
    );
    let tracking = parse_chrony_tracking(&fixture("chronyc-4.5")).unwrap();
    assert_eq!(
        (tracking.offset_us, tracking.leap),
        (234_568, LeapStatus::Pending)
    );
    let tracking = parse_chrony_tracking(&fixture("chronyc-unsynchronised")).unwrap();
    assert_eq!(
        (tracking.offset_us, tracking.leap),
        (0, LeapStatus::NotSynchronised)
    );
    let text = String::from_utf8(fixture("chronyc-4.2")).unwrap();
    let deleting = text.replace(
        "Leap status     : Normal",
        "Leap status     : Delete second",
    );
    assert_eq!(
        parse_chrony_tracking(deleting.as_bytes()).unwrap().leap,
        LeapStatus::Pending
    );
    let refused = [
        text.replace("Leap status     : Normal", "Leap status     : Unknown"),
        text.replace("0.000006523 seconds slow", "nan seconds slow"),
        text.replace("0.000006523 seconds slow", "inf seconds fast"),
        text.replace("seconds slow", "seconds sideways"),
        text.replace("0.000006523 seconds slow of NTP time", "0.000006523"),
        text.replace("System time", "System clock"),
        String::from("506 Cannot talk to daemon\n"),
        String::new(),
    ];
    for text in refused {
        let parsed = parse_chrony_tracking(text.as_bytes());
        assert_eq!(parsed, Err(ParseError::Malformed("chronyc")), "{text}");
    }
}

#[test]
fn systemctl_is_active_gives_one_state_per_firewall_service() {
    let captured = parse_is_active(&fixture("systemctl-is-active")).unwrap();
    let states = (captured.ufw, captured.nftables, captured.firewalld);
    assert_eq!(
        states,
        (
            ServiceState::Active,
            ServiceState::Inactive,
            ServiceState::Inactive
        )
    );
    let none = parse_is_active(&fixture("systemctl-none")).unwrap();
    assert_eq!(none.ufw, ServiceState::Inactive);
    let mixed = parse_is_active(b"failed\nactivating\ndeactivating\n").unwrap();
    let states = (mixed.ufw, mixed.nftables, mixed.firewalld);
    assert_eq!(
        states,
        (
            ServiceState::Failed,
            ServiceState::Activating,
            ServiceState::Other
        )
    );
    for text in [
        &b"active\ninactive\n"[..],
        b"",
        b"active\nactive\nactive\nactive\n",
    ] {
        let parsed = parse_is_active(text);
        assert_eq!(parsed, Err(ParseError::Malformed("systemctl")), "{text:?}");
    }
}

#[test]
fn ufw_conf_is_read_as_the_shell_reads_it() {
    assert!(parse_ufw_conf(&fixture("ufw.conf")).unwrap().enabled);
    let cases = [
        (&b"ENABLED=no\n"[..], false),
        (b"ENABLED=\"yes\"\n", true),
        (b"  ENABLED='YES'  \n", true),
        (b"#ENABLED=yes\n", false),
        (b"ENABLED=yes\nENABLED=no\n", false),
        (b"LOGLEVEL=low\n", false),
        (b"\xff\xfe\nENABLED=yes\n", true),
    ];
    for (text, enabled) in cases {
        assert_eq!(parse_ufw_conf(text).unwrap().enabled, enabled, "{text:?}");
    }
}

#[test]
fn a_probe_that_hangs_is_killed_at_two_seconds_and_reads_as_unknown() {
    let sleep = program(&["/usr/bin/sleep", "/bin/sleep"]);
    let started = Instant::now();
    let result = run_program(sleep, &["30.25"], LIMITS);
    let took = started.elapsed();
    assert_eq!(result, Err(ProbeError::TimedOut));
    assert!(
        took >= Probe::TIMEOUT && took < Duration::from_secs(4),
        "{took:?}"
    );
    let unknown = Unknown::from_probe(&ProbeError::TimedOut);
    assert_eq!(unknown.reason, Reason::TimedOut);
    let children = fs::read_dir("/proc").unwrap().filter_map(|entry| {
        let stat = fs::read_to_string(entry.ok()?.path().join("stat")).ok()?;
        let (_, rest) = stat.rsplit_once(')')?;
        let parent: u32 = rest.split_whitespace().nth(1)?.parse().ok()?;
        (parent == std::process::id() && stat.contains("(sleep)")).then_some(stat)
    });
    assert_eq!(
        children.count(),
        0,
        "the probe is neither running nor left unreaped"
    );
}

#[test]
fn a_probes_output_is_cut_at_64_kib_while_the_program_runs_to_its_end() {
    let head = program(&["/usr/bin/head", "/bin/head"]);
    let output = run_program(head, &["-c", "100000", "/dev/zero"], LIMITS).unwrap();
    assert_eq!(
        (output.status, output.stdout.len()),
        (Some(0), Probe::MAX_OUTPUT)
    );
    let output = run_program(head, &["-c", "4194304", "/dev/zero"], LIMITS).unwrap();
    assert_eq!(
        (output.status, output.stdout.len()),
        (Some(0), Probe::MAX_OUTPUT)
    );
    let output = run_program(head, &["-c", "100", "/dev/zero"], LIMITS).unwrap();
    assert_eq!(output.stdout, [0; 100]);
}

#[test]
fn a_probe_runs_with_lang_c_alone_no_input_and_no_error_output() {
    let env = program(&["/usr/bin/env", "/bin/env"]);
    let output = run_program(env, &[], LIMITS).unwrap();
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "LANG=C\n");
    let cat = program(&["/usr/bin/cat", "/bin/cat"]);
    let started = Instant::now();
    let output = run_program(cat, &[], LIMITS).unwrap();
    assert!(output.stdout.is_empty() && started.elapsed() < Duration::from_secs(1));
    let sh = program(&["/usr/bin/sh", "/bin/sh"]);
    let script = "echo out; echo err >&2; pwd; exit 3";
    let output = run_program(sh, &["-c", script], LIMITS).unwrap();
    assert_eq!(
        (output.status, output.stdout.as_slice()),
        (Some(3), &b"out\n/\n"[..])
    );
}

#[test]
fn a_missing_program_reads_as_unknown() {
    let missing = run_program(Path::new("/nonexistent/timedatectl"), &[], LIMITS);
    assert_eq!(missing, Err(ProbeError::Missing));
    assert_eq!(
        Unknown::from_probe(&ProbeError::Missing).reason,
        Reason::ProbeMissing
    );
}

#[test]
fn the_host_probes_run_only_their_fixed_programs() {
    for probe in Probe::ALL {
        assert!(probe.programs().iter().all(|path| path.starts_with('/')));
        let installed = probe.programs().iter().any(|path| Path::new(path).exists());
        let result = HostProbes.run(probe);
        match installed {
            false => assert_eq!(result, Err(ProbeError::Missing), "{probe:?}"),
            true => assert!(
                !matches!(result, Err(ProbeError::Missing)),
                "{probe:?}: {result:?}"
            ),
        }
    }
}
