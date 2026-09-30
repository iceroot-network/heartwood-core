//! The colon aliases: `relay:start` is `relay start`, for every command the CLI builds.

use std::ffi::OsString;

use heartwood_cli::argv::rewrite;

fn words(line: &str) -> Vec<OsString> {
    line.split_whitespace().map(OsString::from).collect()
}

#[test]
fn every_colon_spelling_of_a_built_command_is_split() {
    let commands = [
        "relay:run",
        "relay:start",
        "relay:stop",
        "relay:restart",
        "relay:status",
        "relay:log",
        "forger:run",
        "forger:start",
        "forger:stop",
        "forger:restart",
        "forger:status",
        "forger:log",
        "network:generate",
        "snapshot:rollback",
        "pool:clear",
        "state:clear",
        "devnet:init",
        "devnet:start",
        "devnet:stop",
        "devnet:status",
        "devnet:logs",
        "devnet:stop-node",
        "devnet:start-node",
        "devnet:restart-node",
        "devnet:reset",
        "devnet:wait",
        "devnet:env",
    ];
    for command in commands {
        let spaced = command.replace(':', " ");
        assert_eq!(
            rewrite(words(&format!(
                "heartwood {command} --token node1 --network devnet"
            ))),
            words(&format!(
                "heartwood {spaced} --token node1 --network devnet"
            )),
            "{command}"
        );
    }
}

#[test]
fn other_arguments_are_left_as_they_are() {
    for line in [
        "heartwood relay start --token a:b",
        "heartwood relay:share",
        "heartwood core:start",
        "heartwood relay:",
        "heartwood :start",
        "heartwood relay:start:now",
        "heartwood --network devnet relay:start",
        "heartwood supervise --role relay",
        "heartwood",
    ] {
        assert_eq!(rewrite(words(line)), words(line), "{line}");
    }
    assert_eq!(rewrite(Vec::new()), Vec::<OsString>::new());
}

#[test]
fn an_argument_that_is_not_utf8_is_kept() {
    use std::os::unix::ffi::OsStringExt;
    let odd = OsString::from_vec(vec![0x72, 0x3a, 0xff]);
    let args = vec!["heartwood".into(), odd.clone(), "relay:start".into()];
    assert_eq!(rewrite(args.clone()), args);
}

#[test]
fn no_daemon_is_daemon_false_up_to_the_end_of_the_flags() {
    let spelled = rewrite(words("heartwood relay:start --network devnet --no-daemon"));
    assert_eq!(
        spelled,
        words("heartwood relay start --network devnet --daemon=false")
    );
    let child = "supervise --role relay -- heartwood relay run --no-daemon";
    assert_eq!(
        rewrite(words(&format!("heartwood {child}"))),
        words(&format!("heartwood {child}"))
    );
}
