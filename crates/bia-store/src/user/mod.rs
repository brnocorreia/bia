//! Users: the people who sign in and own Tokens. First-run setup creates the one User an
//! install has, and `bia serve --reset-admin` sends it back through setup (ADR-0062). Only an
//! argon2id hash of the password is stored; hashing it is the caller's job.

mod model;
mod repository;
mod row;

pub use model::{PasswordHash, User, UserId};
pub use repository::{PgUserRepository, UserRepository};
