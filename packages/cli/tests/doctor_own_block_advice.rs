use heartwood_cli::doctor::docs::docs;

#[test]
fn own_block_advice_states_only_observed_evidence() {
    let doc = docs()
        .unwrap()
        .checks
        .iter()
        .find(|doc| doc.id == "forger.one-key-one-forger")
        .unwrap();
    let text = doc.advice["own-block"].clone();
    assert!(!text.contains("restarted inside a slot"));
    assert!(text.contains("points at no other forger"));
}
