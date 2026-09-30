//! The gate commands' flags: `--name value` pairs, repeatable ones and switches.

use std::collections::BTreeMap;

use crate::errors::{Error, Result};

/// The flags of one gate command, taken one by one; [`Flags::finish`] refuses what is left.
#[derive(Debug, Default)]
pub struct Flags {
    named: BTreeMap<String, Vec<String>>,
    switches: Vec<String>,
    rest: Vec<String>,
}

impl Flags {
    /// Split `args` into flags and positional arguments. `switches` names the flags that take
    /// no value.
    pub fn parse(args: &[String], switches: &[&str]) -> Result<Flags> {
        let mut flags = Flags::default();
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let Some(name) = arg.strip_prefix("--") else {
                flags.rest.push(arg.clone());
                continue;
            };
            if switches.contains(&name) {
                flags.switches.push(name.to_owned());
            } else {
                let value = it
                    .next()
                    .ok_or_else(|| Error::Usage(format!("--{name} needs a value")))?;
                flags
                    .named
                    .entry(name.to_owned())
                    .or_default()
                    .push(value.clone());
            }
        }
        Ok(flags)
    }

    /// The value of `--name`, if given (an error when given twice).
    pub fn one(&mut self, name: &str) -> Result<Option<String>> {
        let mut values = self.named.remove(name).unwrap_or_default();
        if values.len() > 1 {
            return Err(Error::Usage(format!("--{name} is given twice")));
        }
        Ok(values.pop())
    }

    /// The value of `--name`, which the command needs.
    pub fn required(&mut self, name: &str) -> Result<String> {
        self.one(name)?
            .ok_or_else(|| Error::Usage(format!("--{name} is required")))
    }

    /// Every value of `--name`, in order.
    pub fn many(&mut self, name: &str) -> Vec<String> {
        self.named.remove(name).unwrap_or_default()
    }

    /// Whether the switch `--name` is given.
    pub fn switch(&mut self, name: &str) -> bool {
        let before = self.switches.len();
        self.switches.retain(|s| s != name);
        self.switches.len() != before
    }

    /// The positional arguments, in order.
    pub fn positional(&mut self) -> Vec<String> {
        std::mem::take(&mut self.rest)
    }

    /// A number flag.
    pub fn number<T: std::str::FromStr>(&mut self, name: &str) -> Result<Option<T>> {
        self.one(name)?
            .map(|text| {
                text.parse()
                    .map_err(|_| Error::Usage(format!("--{name} needs a number, not {text:?}")))
            })
            .transpose()
    }

    /// Refuse the flags and arguments no one took.
    pub fn finish(self) -> Result<()> {
        if let Some(name) = self.named.keys().next() {
            return Err(Error::Usage(format!("unknown flag --{name}")));
        }
        if let Some(name) = self.switches.first() {
            return Err(Error::Usage(format!("unknown flag --{name}")));
        }
        if let Some(arg) = self.rest.first() {
            return Err(Error::Usage(format!("unexpected argument {arg:?}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn flags_are_taken_and_leftovers_refused() {
        let mut flags = Flags::parse(
            &args(&["--a", "1", "--b", "x", "--b", "y", "--sw", "pos"]),
            &["sw"],
        )
        .unwrap();
        assert_eq!(flags.number::<u32>("a").unwrap(), Some(1));
        assert_eq!(flags.many("b"), ["x", "y"]);
        assert!(flags.switch("sw"));
        assert!(!flags.switch("sw"));
        assert!(flags.finish().is_err());
    }

    #[test]
    fn a_missing_value_and_a_repeat_are_errors() {
        assert!(Flags::parse(&args(&["--a"]), &[]).is_err());
        let mut flags = Flags::parse(&args(&["--a", "1", "--a", "2"]), &[]).unwrap();
        assert!(flags.one("a").is_err());
        let mut flags = Flags::parse(&args(&[]), &[]).unwrap();
        assert!(flags.required("a").is_err());
        flags.finish().unwrap();
    }
}
