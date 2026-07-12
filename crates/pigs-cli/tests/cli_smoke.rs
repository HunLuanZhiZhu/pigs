//! pigs-cli is library-only; product smoke is on `pigs`.

#[test]
fn legacy_crate_is_library_only() {
    assert_eq!(env!("CARGO_PKG_NAME"), "pigs-cli");
}
