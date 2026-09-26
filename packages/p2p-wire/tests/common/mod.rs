//! Shared by the vector tests: the committed records of the wire vector class, and the codec a
//! `wire.decodeSelected` record selects.

use std::path::Path;

use heartwood_p2p_wire::Codec;
use serde_json::{Value, json};

/// The wire vector class.
pub const CLASS: &str = "V13-p2p-wire";

/// The records of the V13 file, without its meta record.
///
/// The file is read from `tests/vectors`, or from the directory in `WIRE_VECTORS` (a relative
/// path is taken from the workspace root), for example a local oracle run in
/// `target/vectors/local`. The class is committed beside the crate, so a missing file fails.
pub fn records() -> Vec<Value> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::var_os("WIRE_VECTORS").map_or_else(
        || manifest.join("tests/vectors"),
        |dir| manifest.join("../..").join(dir),
    );
    let file = dir.join(format!("{CLASS}.jsonl"));
    let text = std::fs::read_to_string(&file).unwrap_or_else(|error| {
        panic!(
            "{}: {error} (make vectors-install-wire installs it)",
            file.display()
        )
    });
    let mut lines = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap());
    let meta = lines.next().expect("the meta record");
    assert_eq!(
        (&meta["op"], &meta["class"]),
        (&json!("meta"), &json!(CLASS))
    );
    lines.collect()
}

/// A `wire.decodeSelected` record as the `wire.decode` record of the codec its first byte selects
/// (0 for an empty input), with the rest as the payload; any other record as it is.
#[allow(
    dead_code,
    reason = "the frame and message tests read the records only"
)]
pub fn selected_as_decode(record: &Value) -> Value {
    let mut decode = record.clone();
    if record["op"] != "wire.decodeSelected" {
        return decode;
    }
    let bytes = hex::decode(record["input"]["hex"].as_str().unwrap()).unwrap();
    let (first, payload) = bytes.split_first().map_or((0, &[][..]), |(b, p)| (*b, p));
    let codec = Codec::select(first);
    let (route, direction) = (json!(codec.route().name()), json!(codec.direction().name()));
    if let Some(output) = decode.get_mut("output").and_then(Value::as_object_mut) {
        assert_eq!(
            (output.remove("route"), output.remove("direction")),
            (Some(route.clone()), Some(direction.clone())),
            "{}: the selected codec",
            record["name"]
        );
    }
    decode["op"] = json!("wire.decode");
    decode["input"] =
        json!({ "route": route, "direction": direction, "hex": hex::encode(payload) });
    decode
}
