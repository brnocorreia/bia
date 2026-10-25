//! Secrets of the internal Secret Provider: names and metadata in the clear, values only
//! encrypted under the master key (ADR-0017, ADR-0049). Only the Worker's resolver reads a
//! value back, through [`SecretRepository::get`] and [`MasterKey::open`].

mod crypto;
mod model;
mod repository;

pub use crypto::{CryptoError, MasterKey, MasterKeyError};
pub use model::{InvalidSecretName, Sealed, SecretInfo, SecretName};
pub use repository::{PgSecretRepository, SecretRepository};
