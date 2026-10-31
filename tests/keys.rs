//! Key storage, run against keyring-core's in-memory sample store so that no
//! test reads or writes the real system keychain.

use std::sync::Arc;

use keyring_core::api::CredentialStoreApi;
use keyring_core::sample::Store;

use hodis::keys::{ApiKey, Keychain, KeyedProvider};

const SERPER: KeyedProvider = KeyedProvider::Serper;

fn keychain() -> anyhow::Result<(Keychain, Arc<Store>)> {
    let store = Store::new()?;
    Ok((Keychain::new(store.clone()), store))
}

#[test]
fn a_stored_key_reads_back() -> anyhow::Result<()> {
    let (keychain, _store) = keychain()?;
    let key = ApiKey::new("0123abcd")?;

    keychain.set(SERPER, &key)?;

    assert_eq!(keychain.get(SERPER)?, Some(key));
    Ok(())
}

#[test]
fn storing_a_key_replaces_the_one_before() -> anyhow::Result<()> {
    let (keychain, _store) = keychain()?;
    keychain.set(SERPER, &ApiKey::new("old")?)?;

    keychain.set(SERPER, &ApiKey::new("new")?)?;

    assert_eq!(keychain.get(SERPER)?, Some(ApiKey::new("new")?));
    Ok(())
}

#[test]
fn a_provider_without_a_stored_key_has_none() -> anyhow::Result<()> {
    let (keychain, _store) = keychain()?;
    assert_eq!(keychain.get(SERPER)?, None);
    Ok(())
}

#[test]
fn deleting_reports_whether_there_was_a_key() -> anyhow::Result<()> {
    let (keychain, _store) = keychain()?;
    keychain.set(SERPER, &ApiKey::new("0123abcd")?)?;

    assert!(keychain.delete(SERPER)?);
    assert_eq!(keychain.get(SERPER)?, None);
    assert!(!keychain.delete(SERPER)?);
    Ok(())
}

#[test]
fn keys_are_filed_under_hodis_and_the_provider() -> anyhow::Result<()> {
    let (keychain, store) = keychain()?;
    keychain.set(SERPER, &ApiKey::new("0123abcd")?)?;

    let entry = store.build("hodis", "serper", None)?;

    assert_eq!(entry.get_password()?, "0123abcd");
    Ok(())
}

#[test]
fn a_malformed_stored_key_is_an_error() -> anyhow::Result<()> {
    let (keychain, store) = keychain()?;
    store
        .build("hodis", "serper", None)?
        .set_password("two words")?;

    let error = keychain
        .get(SERPER)
        .expect_err("a key with a space is refused");

    assert!(error.to_string().contains("is invalid"), "{error:#}");
    Ok(())
}

#[test]
fn whitespace_around_a_key_is_dropped() -> anyhow::Result<()> {
    assert_eq!(ApiKey::new("  0123abcd\n")?.expose(), "0123abcd");
    Ok(())
}

#[test]
fn an_empty_key_is_refused() {
    let error = ApiKey::new(" \n").expect_err("nothing but whitespace is refused");
    assert!(error.to_string().contains("empty"), "{error:#}");
}

#[test]
fn a_key_with_spaces_inside_is_refused() {
    let error = ApiKey::new("0123 abcd").expect_err("an inner space is refused");
    assert!(error.to_string().contains("spaces"), "{error:#}");
}

#[test]
fn debug_output_leaves_the_key_out() -> anyhow::Result<()> {
    let debug = format!("{:?}", ApiKey::new("0123abcd")?);
    assert!(!debug.contains("0123abcd"), "{debug}");
    Ok(())
}
