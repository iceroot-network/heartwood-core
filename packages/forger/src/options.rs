//! The forger component's options: its entry's `options` object in `app.json`, read as the
//! reference's schema reads it, with the reference's defaults.
//!
//! | Key | Value | Default |
//! |---|---|---|
//! | `hosts` | the relays, in order: `[{hostname, port}]`, each hostname an IP address literal and each port from 1 to 65535 (a number or its decimal text) | one relay at `127.0.0.1` on `CORE_P2P_PORT`, else 4002 |
//! | `aux` | 64 hex digits: the aux of every block signature | none: fresh random bytes for each signature |
//! | `delay` | milliseconds to wait before each attempt to forge, a debugging aid | 0 |
//! | `localAddress` | the IP address the forger's sockets are bound to | none |
//!
//! Other keys are allowed and ignored, as the reference's schema allows them. The reference's
//! default port is 4000 where the relay's is 4002; Heartwood's forger defaults to the relay's.

use std::net::IpAddr;
use std::time::Duration;

use heartwood_crypto::Aux;
use serde_json::{Map, Value};

use crate::errors::OptionsError;

/// The relay port when neither `hosts` nor `CORE_P2P_PORT` gives one: the relay's default.
pub const DEFAULT_PORT: u16 = 4002;

/// A relay the forger connects to: `ws://<address>:<port>/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RelayHost {
    /// The relay's address as configured, an IP literal as the reference's schema requires: the
    /// text that the log lines, the errors and `emitEvent`'s local host rule use.
    pub hostname: String,
    /// The address parsed from `hostname`, which the sessions connect to.
    pub address: IpAddr,
    /// The relay's peer-to-peer port.
    pub port: u16,
}

impl RelayHost {
    /// The relay at `hostname` and `port`; `None` when `hostname` is not an IP address.
    pub fn new(hostname: &str, port: u16) -> Option<RelayHost> {
        let address = hostname.parse().ok()?;
        let hostname = hostname.to_owned();
        Some(RelayHost {
            hostname,
            address,
            port,
        })
    }
}

/// The forger component's options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The relays, in the order host selection tries them.
    pub hosts: Vec<RelayHost>,
    /// The aux of every block signature; `None` draws fresh random bytes for each one.
    pub aux: Option<[u8; 32]>,
    /// The wait before each attempt to forge.
    pub delay: Duration,
    /// The address the forger's sockets are bound to.
    pub local_address: Option<IpAddr>,
}

impl Options {
    /// The options in `value`, the component's `options` object (`null` or absent: every
    /// default), with `env` giving the environment (`CORE_P2P_PORT`, for the default host).
    pub fn from_json(
        value: &Value,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Options, OptionsError> {
        let empty = Map::new();
        let object = match value {
            Value::Null => &empty,
            Value::Object(object) => object,
            _ => return Err(invalid("options", "must be an object")),
        };
        let hosts = match object.get("hosts") {
            None => Options::default_hosts(env)?,
            Some(Value::Array(hosts)) => (hosts.iter().enumerate())
                .map(|(index, host)| relay_host(index, host))
                .collect::<Result<_, _>>()?,
            Some(_) => return Err(invalid("hosts", "must be an array")),
        };
        let aux = match object.get("aux") {
            None => None,
            Some(Value::String(text)) => Some(aux(text)?),
            Some(_) => return Err(invalid("aux", "must be 64 hex digits")),
        };
        let delay = match object.get("delay") {
            None | Some(Value::Null) => Duration::ZERO,
            Some(value) => Duration::from_millis(
                whole_number(value)
                    .ok_or_else(|| invalid("delay", "must be whole milliseconds"))?,
            ),
        };
        let local_address = match object.get("localAddress") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(
                text.parse()
                    .map_err(|_| invalid("localAddress", "must be an IP address"))?,
            ),
            Some(_) => return Err(invalid("localAddress", "must be an IP address")),
        };
        Ok(Options {
            hosts,
            aux,
            delay,
            local_address,
        })
    }

    /// The default relay: `127.0.0.1` on `CORE_P2P_PORT` when it is set, else on
    /// [`DEFAULT_PORT`].
    pub fn default_hosts(
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Vec<RelayHost>, OptionsError> {
        let port = match env("CORE_P2P_PORT").filter(|text| !text.is_empty()) {
            None => DEFAULT_PORT,
            Some(text) => port(&Value::String(text))
                .ok_or_else(|| invalid("hosts[0].port", "CORE_P2P_PORT must be a port"))?,
        };
        Ok(vec![RelayHost {
            hostname: "127.0.0.1".into(),
            address: IpAddr::from([127, 0, 0, 1]),
            port,
        }])
    }

    /// The aux of a block signature: the configured bytes, or fresh random ones.
    pub fn aux(&self) -> Aux {
        self.aux.map_or(Aux::random(), Aux::configured)
    }
}

fn invalid(key: &str, reason: &str) -> OptionsError {
    OptionsError::Invalid {
        key: key.to_owned(),
        reason: reason.to_owned(),
    }
}

fn relay_host(index: usize, value: &Value) -> Result<RelayHost, OptionsError> {
    let key = |field: &str| format!("hosts[{index}].{field}");
    let Value::Object(host) = value else {
        return Err(invalid(&format!("hosts[{index}]"), "must be an object"));
    };
    let hostname = (host.get("hostname").and_then(Value::as_str))
        .and_then(|text| RelayHost::new(text, DEFAULT_PORT))
        .ok_or_else(|| invalid(&key("hostname"), "must be an IP address"))?;
    let port = (host.get("port"))
        .and_then(port)
        .ok_or_else(|| invalid(&key("port"), "must be a port from 1 to 65535"))?;
    Ok(RelayHost { port, ..hostname })
}

/// A port from 1 to 65535: a number, or its decimal text, as the reference's schema converts it.
fn port(value: &Value) -> Option<u16> {
    whole_number(value)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port >= 1)
}

/// A whole number of at least 0: a JSON number without a fraction, or its decimal text.
fn whole_number(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64().or_else(|| {
            let float = number.as_f64()?;
            // A float with no fraction, below 2^53, is exactly that integer.
            (float >= 0.0 && float.fract() == 0.0 && float < 9_007_199_254_740_992.0)
                .then_some(float as u64)
        }),
        Value::String(text) if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) => {
            text.parse().ok()
        }
        _ => None,
    }
}

fn aux(text: &str) -> Result<[u8; 32], OptionsError> {
    let bytes = heartwood_crypto::utils::hex::decode(&text.to_ascii_lowercase())
        .map_err(|_| invalid("aux", "must be 64 hex digits"))?;
    <[u8; 32]>::try_from(bytes).map_err(|_| invalid("aux", "must be 64 hex digits"))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn every_option_has_the_reference_default() {
        let options = Options::from_json(&Value::Null, &no_env).unwrap();
        assert_eq!(
            options,
            Options {
                hosts: vec![RelayHost::new("127.0.0.1", 4002).unwrap()],
                aux: None,
                delay: Duration::ZERO,
                local_address: None,
            }
        );
        let env = |name: &str| (name == "CORE_P2P_PORT").then(|| "6002".to_owned());
        let options = Options::from_json(&json!({}), &env).unwrap();
        assert_eq!(options.hosts[0].port, 6002);
        let env = |name: &str| (name == "CORE_P2P_PORT").then(|| "port".to_owned());
        assert!(Options::from_json(&json!({}), &env).is_err());
    }

    #[test]
    fn hosts_aux_delay_and_the_local_address_are_read() {
        let aux = "AB".repeat(32);
        let value = json!({
            "hosts": [
                {"hostname": "127.0.0.11", "port": 6002},
                {"hostname": "::1", "port": "6003"},
            ],
            "aux": aux,
            "delay": 250,
            "localAddress": "127.0.0.111",
            "unknown": true,
        });
        let options = Options::from_json(&value, &no_env).unwrap();
        assert_eq!(
            options.hosts,
            [
                RelayHost::new("127.0.0.11", 6002).unwrap(),
                RelayHost::new("::1", 6003).unwrap(),
            ]
        );
        let spelled = json!({"hosts": [{"hostname": "0:0:0:0:0:0:0:1", "port": 6002}]});
        let spelled = Options::from_json(&spelled, &no_env).unwrap();
        assert_eq!(
            spelled.hosts[0].hostname, "0:0:0:0:0:0:0:1",
            "the text as configured"
        );
        assert_eq!(spelled.hosts[0].address, "::1".parse::<IpAddr>().unwrap());
        assert_eq!(options.aux, Some([0xab; 32]));
        assert_eq!(options.delay, Duration::from_millis(250));
        assert_eq!(options.local_address, Some("127.0.0.111".parse().unwrap()));
        assert_eq!(options.aux(), Aux::configured([0xab; 32]));
        let random = Options::from_json(&Value::Null, &no_env).unwrap();
        assert_eq!(random.aux(), Aux::random());
    }

    #[test]
    fn values_the_schema_refuses_are_refused() {
        for value in [
            json!([]),
            json!({"hosts": {}}),
            json!({"hosts": [{"hostname": "localhost", "port": 6002}]}),
            json!({"hosts": [{"hostname": "127.0.0.1", "port": 0}]}),
            json!({"hosts": [{"hostname": "127.0.0.1", "port": 65536}]}),
            json!({"hosts": [{"hostname": "127.0.0.1", "port": 6002.5}]}),
            json!({"hosts": [{"hostname": "127.0.0.1"}]}),
            json!({"aux": "ab"}),
            json!({"aux": "zz".repeat(32)}),
            json!({"aux": 1}),
        ] {
            assert!(
                Options::from_json(&value, &no_env).is_err(),
                "{value} was accepted"
            );
        }
    }

    /// The reference's schema accepts these: an IP address with a CIDR suffix (its `ip()` rule
    /// allows one), and any `delay` (it waits only for a positive number). It has no
    /// `localAddress`.
    #[test]
    fn values_the_schema_accepts_but_heartwood_refuses_are_refused() {
        for value in [
            json!({"hosts": [{"hostname": "127.0.0.1/8", "port": 6002}]}),
            json!({"hosts": [{"hostname": "::1/128", "port": 6002}]}),
            json!({"delay": -1}),
            json!({"delay": "soon"}),
            json!({"localAddress": "here"}),
        ] {
            assert!(
                Options::from_json(&value, &no_env).is_err(),
                "{value} was accepted"
            );
        }
    }
}
