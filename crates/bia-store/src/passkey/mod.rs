//! Passkeys a User signs in with instead of the password, and the short-lived state of the
//! WebAuthn ceremonies that register and use them (ADR-0065). The credential is stored as the
//! opaque JSON `bia-server` serialises, so this crate knows nothing about WebAuthn.

mod model;
mod repository;
mod row;

pub use model::{CeremonyId, CeremonyKind, NewPasskey, Passkey, PasskeyId};
pub use repository::{PasskeyRepository, PgPasskeyRepository};
