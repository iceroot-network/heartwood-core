//! hapi's choice of an answer's encoding: `@hapi/accept` over the `Accept-Encoding` header and
//! hapi's encodings (`identity`, `gzip`, `deflate`, in its order of preference). hapi compresses a
//! compressible answer of 1 KiB or more with the encoding chosen, unless it is `identity`; the
//! relay compresses none, but Node writes the head of a compressed answer otherwise.

use std::cmp::Ordering;

use super::is_js_space;

/// hapi's encodings, in its order of preference.
const ENCODINGS: [&str; 3] = ["identity", "gzip", "deflate"];

/// Whether hapi compresses a compressible answer of 1 KiB or more to a request whose
/// `Accept-Encoding` is `header`: the encoding it chooses is `gzip` or `deflate`.
pub(super) fn compresses(header: Option<&str>) -> bool {
    let chosen = chosen(header.unwrap_or_default());
    chosen.is_some_and(|encoding| encoding != "identity")
}

/// The encoding `@hapi/accept` chooses from `header`: `identity` for none, and none when nothing
/// is acceptable. A header it refuses (a part with two parameters, an empty coding, a parameter
/// that is not a weight) is read by hapi as `identity`.
fn chosen(header: &str) -> Option<&'static str> {
    if header.is_empty() {
        return Some("identity");
    }
    // Spaces and tabs are removed first; each part is a coding and an optional weight.
    let header: String = header
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t'))
        .collect();
    let mut named = Vec::new();
    let mut selections = Vec::new();
    for (position, part) in header.split(',').enumerate() {
        if part.is_empty() {
            continue;
        }
        let mut params = part.split(';');
        let coding = params.next().unwrap_or_default().to_lowercase();
        let weight = params.next();
        if coding.is_empty() || params.next().is_some() {
            return Some("identity");
        }
        let coding = match coding.as_str() {
            "x-gzip" => "gzip".to_owned(),
            "x-compress" => "compress".to_owned(),
            _ => coding,
        };
        named.push(coding.clone());
        let mut q = 1.0;
        if let Some(weight) = weight {
            let mut pair = weight.split('=');
            let key = pair.next().unwrap_or_default();
            let value = pair.next().unwrap_or_default();
            if value.is_empty() || !matches!(key, "q" | "Q") {
                return Some("identity");
            }
            let score = parse_float(value);
            if score == 0.0 {
                continue;
            }
            if score.is_finite() && (0.001..=1.0).contains(&score) {
                q = score;
            }
        }
        let preference = ENCODINGS.iter().position(|encoding| *encoding == coding);
        selections.push((coding, position, q, preference));
    }
    // By weight, then hapi's preference (a coding it does not have last), then position.
    selections.sort_by(|a, b| {
        let preference = match (a.3, b.3) {
            (Some(a), Some(b)) => a.cmp(&b),
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (None, None) => Ordering::Equal,
        };
        let weight = b.2.partial_cmp(&a.2).unwrap_or(Ordering::Equal);
        weight.then(preference).then(a.1.cmp(&b.1))
    });
    let mut codings: Vec<String> = selections.into_iter().map(|s| s.0).collect();
    if !named.iter().any(|coding| coding == "identity") {
        codings.push("identity".to_owned());
    }
    let unnamed = |encoding: &&&str| !named.iter().any(|coding| coding == **encoding);
    for coding in codings {
        let found = match coding.as_str() {
            "*" => ENCODINGS.iter().find(unnamed),
            _ => ENCODINGS.iter().find(|encoding| **encoding == coding),
        };
        if let Some(encoding) = found {
            return Some(encoding);
        }
    }
    None
}

/// JavaScript's `parseFloat`: the longest decimal number (or `Infinity`) that starts `text`
/// after its white space, NaN for none.
fn parse_float(text: &str) -> f64 {
    let text = text.trim_start_matches(is_js_space);
    let bytes = text.as_bytes();
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let rest = text.get(sign..).unwrap_or_default();
    if rest.starts_with("Infinity") {
        return if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let digits = |from: usize| {
        let tail = bytes.get(from..).unwrap_or_default();
        tail.iter().take_while(|byte| byte.is_ascii_digit()).count()
    };
    let whole = digits(sign);
    let mut end = sign + whole;
    let fraction = match bytes.get(end) {
        Some(b'.') => digits(end + 1),
        _ => 0,
    };
    if whole + fraction == 0 {
        return f64::NAN;
    }
    if bytes.get(end) == Some(&b'.') {
        end += 1 + fraction;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let signed = usize::from(matches!(bytes.get(end + 1), Some(b'+' | b'-')));
        let exponent = digits(end + 1 + signed);
        if exponent > 0 {
            end += 1 + signed + exponent;
        }
    }
    text.get(..end)
        .and_then(|number| number.parse().ok())
        .unwrap_or(f64::NAN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoding_is_chosen_as_hapi_accept_chooses_it() {
        // `Accept.encoding(header, ['identity', 'gzip', 'deflate'])` of @hapi/accept 5.0.2 under
        // Node 18.20.8, and `identity` where it throws.
        let cases = [
            ("", Some("identity")),
            ("gzip", Some("gzip")),
            ("GZIP", Some("gzip")),
            ("x-gzip", Some("gzip")),
            ("deflate", Some("deflate")),
            ("deflate, gzip", Some("gzip")),
            ("gzip, deflate, br", Some("gzip")),
            ("gzip, identity", Some("identity")),
            ("*", Some("identity")),
            ("identity;q=0, *", Some("gzip")),
            ("br", Some("identity")),
            ("identity;q=0", None),
            ("gzip;q=0", Some("identity")),
            ("gzip;q=0.5, identity;q=0.4", Some("gzip")),
            ("gzip;q=0.5, deflate;q=0.6", Some("deflate")),
            ("gzip;q=2, deflate;q=0.9", Some("gzip")),
            ("gzip;q=0.0001, deflate;q=0.9", Some("gzip")),
            ("gzip;q=-0, deflate", Some("deflate")),
            ("gzip;q=0e5, deflate", Some("deflate")),
            ("gzip;q=.5x, deflate;q=0.4", Some("gzip")),
            ("gzip;q=x, deflate;q=0.4", Some("gzip")),
            ("gzip;q=\u{a0}0.3, deflate;q=0.4", Some("deflate")),
            ("gzip;q=Infinity, deflate;q=0.4", Some("gzip")),
            ("gzip;v=1", Some("identity")),
            ("gzip;q=1;x=2", Some("identity")),
            ("gzip;q=", Some("identity")),
            (";q=1", Some("identity")),
            (",,gzip,", Some("gzip")),
            ("g zip", Some("gzip")),
            ("de\tflate", Some("deflate")),
        ];
        for (header, encoding) in cases {
            assert_eq!(chosen(header), encoding, "{header:?}");
        }
        assert!(compresses(Some("gzip")) && !compresses(None) && !compresses(Some("identity;q=0")));
    }
}
