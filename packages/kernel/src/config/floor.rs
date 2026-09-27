//! The version floor: the node's own version against the `p2p.minimumVersions` of the milestones.
//!
//! The floor in force at the stored tip (height 1 at a first start) must admit the node's version,
//! or the start stops and names the height. A later milestone whose floor would refuse the node is
//! a warning, with its height: the node is cut off there, as the network intends.

use heartwood_crypto::{Milestones, Params};
use thiserror::Error;

/// A later milestone whose floor refuses the node's version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorWarning {
    /// The milestone's height.
    pub height: u32,
    /// Its minimum versions.
    pub ranges: Vec<String>,
}

/// The floor in force refuses the node's version.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error(
    "this node's version {version} is below the minimum versions {} in force at height {height}",
    ranges.join(", ")
)]
pub struct FloorError {
    /// The height whose milestone refuses the node.
    pub height: u32,
    /// The node's version.
    pub version: String,
    /// The milestone's minimum versions.
    pub ranges: Vec<String>,
}

/// Check the node's version `own_version` on the network named `network_name` against the floor
/// in force at `height` and the later ones; `gate` is the peers' version gate (whether a version
/// passes a milestone's minimum versions on the network).
pub fn check(
    milestones: &Milestones,
    own_version: &str,
    network_name: &str,
    height: u32,
    gate: &dyn Fn(&str, &Params, &str) -> bool,
) -> Result<Vec<FloorWarning>, FloorError> {
    let admits = |params: &Params| gate(own_version, params, network_name);
    let ranges = |params: &Params| {
        let listed = params
            .as_json()
            .get("p2p")
            .and_then(|p2p| p2p.get("minimumVersions"));
        serde_json::from_value(listed.cloned().unwrap_or_default()).unwrap_or_default()
    };
    let in_force = milestones.at(height);
    if !admits(in_force) {
        let (version, ranges) = (own_version.to_owned(), ranges(in_force));
        return Err(FloorError {
            height,
            version,
            ranges,
        });
    }
    let later = milestones
        .all()
        .iter()
        .filter(|params| params.height() > height);
    let refused = later.filter(|params| !admits(params));
    Ok(refused
        .map(|params| FloorWarning {
            height: params.height(),
            ranges: ranges(params),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use heartwood_crypto::Network;
    use serde_json::{Value, json};

    use super::*;

    /// The corpus's network and its milestone file with `later` appended.
    fn milestones(later: &[Value]) -> Milestones {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tools/difftest/corpus/txgen-one-node/crypto"
        );
        let read = |file: &str| std::fs::read_to_string(format!("{dir}/{file}")).unwrap();
        let network = Network::load(&read("network.json")).unwrap();
        let mut all: Vec<Value> = serde_json::from_str(&read("milestones.json")).unwrap();
        all.extend_from_slice(later);
        Milestones::load(&Value::Array(all).to_string(), &network).unwrap()
    }

    /// A gate of `>=a.b.c` ranges over plain versions.
    fn gate(version: &str, params: &Params, _network: &str) -> bool {
        let parse =
            |text: &str| -> Vec<u32> { text.split('.').map(|n| n.parse().unwrap()).collect() };
        let listed = &params.as_json()["p2p"]["minimumVersions"];
        let ranges = listed.as_array().unwrap();
        ranges.iter().any(|range| {
            let floor = range.as_str().unwrap().trim_start_matches(">=");
            parse(version) >= parse(floor)
        })
    }

    #[test]
    fn the_floor_in_force_refuses_and_a_later_one_warns() {
        let raised = |height: u32, floor: &str| json!({"height": height, "p2p": {"minimumVersions": [floor]}});
        let later = milestones(&[raised(100, ">=4.4.0"), raised(200, ">=5.0.0")]);
        // The corpus's floor is >=4.3.0 from height 1.
        assert_eq!(
            check(&later, "4.3.1", "devnet", 1, &gate),
            Ok(vec![
                FloorWarning {
                    height: 100,
                    ranges: vec![">=4.4.0".into()]
                },
                FloorWarning {
                    height: 200,
                    ranges: vec![">=5.0.0".into()]
                },
            ])
        );
        assert_eq!(
            check(&later, "4.4.0", "devnet", 99, &gate).unwrap().len(),
            1
        );
        let error = check(&later, "4.3.1", "devnet", 150, &gate).unwrap_err();
        assert_eq!(
            error,
            FloorError {
                height: 150,
                version: "4.3.1".into(),
                ranges: vec![">=4.4.0".into()]
            }
        );
        assert!(error.to_string().contains("150"), "{error}");
        assert_eq!(check(&later, "5.0.0", "devnet", 1, &gate), Ok(Vec::new()));
        assert!(check(&milestones(&[]), "4.2.9", "devnet", 1, &gate).is_err());
    }
}
