use super::*;

#[test]
fn amounts_read_and_print_as_bigint_does() {
    assert_eq!(Amount::parse(" 0x10\n"), Ok(Amount::new(16)));
    assert_eq!(Amount::parse(""), Ok(Amount::ZERO));
    assert_eq!(Amount::parse("-0").map(|a| a.to_string()), Ok("0".into()));
    assert_eq!(Amount::parse("1.5"), Err(DecodeError::AmountSyntax));
    assert_eq!(
        Amount::parse(&"9".repeat(40)),
        Err(DecodeError::AmountTooWide)
    );
    assert_eq!(Amount::from(u64::MAX).to_string(), "18446744073709551615");
    assert_eq!(Amount::new(-5).to_string(), "-5");
}
