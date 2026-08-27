use super::*;

#[test]
fn strict_wire_enum_parsing_rejects_unknown_or_noncanonical_values() {
    assert_eq!("portfolio".parse(), Ok(MarginMode::Portfolio));
    assert_eq!("perp".parse(), Ok(InstrumentKind::Perp));
    assert!("Portfolio".parse::<MarginMode>().is_err());
    assert!("future".parse::<MarginMode>().is_err());
    assert!("PERP".parse::<InstrumentKind>().is_err());
    assert!("future".parse::<InstrumentKind>().is_err());
}

#[test]
fn perp_tif_encoding_matches_hypercall_directive_contract() {
    assert_eq!(PerpTimeInForce::Alo.encoded(), 1);
    assert_eq!(PerpTimeInForce::Gtc.encoded(), 2);
    assert_eq!(PerpTimeInForce::Ioc.encoded(), 3);
}
