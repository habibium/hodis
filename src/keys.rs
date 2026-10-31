//! API keys for the providers that need one. Keys are kept in the system
//! keychain, never in a file. A provider's environment variable takes
//! precedence, and is how a key reaches hodis where there is no keychain.

use std::env;
use std::fmt;
use std::sync::Arc;

use anyhow::{Context, Result, ensure};
use clap::ValueEnum;
use keyring_core::{CredentialStore, Entry, Error};

/// The keychain service hodis files its entries under, one per provider.
const SERVICE: &str = "hodis";

/// A provider that needs an API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum KeyedProvider {
    /// Google results through serper.dev
    Serper,
}

impl KeyedProvider {
    /// The provider's name as people write it.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Serper => "Serper",
        }
    }

    /// The environment variable that holds the provider's key, taking
    /// precedence over one in the keychain.
    #[must_use]
    pub fn env_var(self) -> &'static str {
        match self {
            Self::Serper => "SERPER_API_KEY",
        }
    }

    /// Whether the provider's environment variable is set to anything.
    #[must_use]
    pub fn env_var_is_set(self) -> bool {
        env::var_os(self.env_var()).is_some_and(|value| !value.is_empty())
    }

    /// The provider's name on the command line, which also names its keychain
    /// entry, so it must never change: a new name would orphan every key
    /// stored under the old one.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::Serper => "serper",
        }
    }
}

/// Finds a provider's key: in its environment variable when that is set, or
/// else in the system keychain.
pub fn find(provider: KeyedProvider) -> Result<ApiKey> {
    let (name, var) = (provider.name(), provider.env_var());
    if let Some(value) = env::var_os(var).filter(|value| !value.is_empty()) {
        let value = value
            .to_str()
            .with_context(|| format!("{var} is not valid UTF-8"))?;
        return ApiKey::new(value).with_context(|| format!("{var} does not hold a valid key"));
    }

    let keychain = Keychain::open().with_context(|| {
        format!("no {name} API key: set {var}, as there is no system keychain to store one in")
    })?;
    keychain.get(provider)?.with_context(|| {
        format!(
            "no {name} API key: store one with `hodis key set {}` or set {var}",
            provider.id()
        )
    })
}

/// An API key, kept out of its `Debug` output so it cannot reach a log or an
/// error message by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Takes a key as typed or pasted, without the whitespace around it.
    pub fn new(text: &str) -> Result<Self> {
        let key = text.trim();
        ensure!(!key.is_empty(), "the key is empty");
        ensure!(
            !key.chars().any(|c| c.is_whitespace() || c.is_control()),
            "the key contains spaces or control characters"
        );
        Ok(Self(key.to_owned()))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(..)")
    }
}

/// A credential store holding API keys.
#[derive(Debug)]
pub struct Keychain {
    store: Arc<CredentialStore>,
}

impl Keychain {
    /// The system keychain: Keychain Services on macOS, Credential Manager on
    /// Windows, and the Secret Service on other Unix systems.
    pub fn open() -> Result<Self> {
        Ok(Self::new(platform_store()?))
    }

    /// A keychain kept in `store`, such as one of keyring-core's test stores.
    #[must_use]
    pub fn new(store: Arc<CredentialStore>) -> Self {
        Self { store }
    }

    pub fn get(&self, provider: KeyedProvider) -> Result<Option<ApiKey>> {
        let name = provider.name();
        match self.entry(provider)?.get_password() {
            Ok(text) => ApiKey::new(&text)
                .map(Some)
                .with_context(|| format!("the {name} key in the keychain is invalid")),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => {
                Err(e).with_context(|| format!("cannot read the {name} key from the keychain"))
            }
        }
    }

    /// Stores `key`, replacing any key the provider had.
    pub fn set(&self, provider: KeyedProvider, key: &ApiKey) -> Result<()> {
        self.entry(provider)?
            .set_password(key.expose())
            .with_context(|| format!("cannot store the {} key in the keychain", provider.name()))
    }

    /// Deletes the provider's key, returning whether there was one.
    pub fn delete(&self, provider: KeyedProvider) -> Result<bool> {
        match self.entry(provider)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(Error::NoEntry) => Ok(false),
            Err(e) => Err(e).with_context(|| {
                format!(
                    "cannot delete the {} key from the keychain",
                    provider.name()
                )
            }),
        }
    }

    fn entry(&self, provider: KeyedProvider) -> Result<Entry> {
        Ok(self.store.build(SERVICE, provider.id(), None)?)
    }
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(windows)]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "android"))
))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(windows, all(unix, not(any(target_os = "ios", target_os = "android"))))))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(Error::NotSupportedByStore(
        "hodis supports no keychain on this platform".to_owned(),
    ))
}
