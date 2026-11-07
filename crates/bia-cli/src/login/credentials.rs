//! Where `bia login` keeps the credential it gets (ADR-0068): the OS keychain, or when there
//! is none, `credentials.json` in the config directory with mode 0600.
//!
//! The keychain is behind [`Keychain`] so tests never touch a developer's own. The
//! environment picks the directory (`BIA_CONFIG_DIR`) and can turn the keychain off
//! (`BIA_KEYCHAIN=off`), which is what every test that runs the binary does.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "bia";
const FILE_NAME: &str = "credentials.json";
const LOCK_NAME: &str = "credentials.lock";

/// A login: the tokens `bia login` got, until `bia logout`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoredLogin {
    pub(crate) access_token: String,
    pub(crate) refresh_token: String,
    /// When the access token stops working.
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) scope: String,
}

// Not derived: the tokens must not reach any output through `{:?}`.
impl fmt::Debug for StoredLogin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredLogin")
            .field("expires_at", &self.expires_at)
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

/// The keychain could not be used: there is none, it is locked, or it refused.
#[derive(Debug)]
pub(crate) struct KeychainUnavailable;

pub(crate) trait Keychain: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<String>, KeychainUnavailable>;
    fn set(&self, key: &str, value: &str) -> Result<(), KeychainUnavailable>;
    fn delete(&self, key: &str) -> Result<(), KeychainUnavailable>;
}

/// The operating system's keychain, through `keyring`.
pub(crate) struct OsKeychain;

impl OsKeychain {
    fn entry(key: &str) -> Result<keyring::Entry, KeychainUnavailable> {
        keyring::Entry::new(KEYCHAIN_SERVICE, key).map_err(|_| KeychainUnavailable)
    }
}

impl Keychain for OsKeychain {
    fn get(&self, key: &str) -> Result<Option<String>, KeychainUnavailable> {
        match Self::entry(key)?.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(KeychainUnavailable),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), KeychainUnavailable> {
        Self::entry(key)?
            .set_password(value)
            .map_err(|_| KeychainUnavailable)
    }

    fn delete(&self, key: &str) -> Result<(), KeychainUnavailable> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(KeychainUnavailable),
        }
    }
}

/// No keychain, so everything goes to the file.
pub(crate) struct NoKeychain;

impl Keychain for NoKeychain {
    fn get(&self, _: &str) -> Result<Option<String>, KeychainUnavailable> {
        Err(KeychainUnavailable)
    }

    fn set(&self, _: &str, _: &str) -> Result<(), KeychainUnavailable> {
        Err(KeychainUnavailable)
    }

    fn delete(&self, _: &str) -> Result<(), KeychainUnavailable> {
        Err(KeychainUnavailable)
    }
}

/// Where [`Credentials::save`] put a login.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stored {
    Keychain,
    File(PathBuf),
}

#[derive(Default, Serialize, Deserialize)]
struct FileContents {
    #[serde(default)]
    logins: BTreeMap<String, StoredLogin>,
}

pub(crate) struct Credentials {
    keychain: Box<dyn Keychain>,
    dir: PathBuf,
}

/// The key a server's login is kept under: its URL, without a trailing slash.
fn key(url: &str) -> String {
    url::Url::parse(url).map_or_else(
        |_| url.trim_end_matches('/').to_owned(),
        |url| url.as_str().trim_end_matches('/').to_owned(),
    )
}

impl Credentials {
    pub(crate) fn new(keychain: Box<dyn Keychain>, dir: PathBuf) -> Self {
        Self { keychain, dir }
    }

    /// The store the environment names, or `None` when there is no home directory to put a
    /// fallback file in.
    pub(crate) fn from_env() -> Option<Self> {
        let dir = std::env::var_os("BIA_CONFIG_DIR")
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("XDG_CONFIG_HOME")
                    .filter(|dir| !dir.is_empty())
                    .map(|dir| PathBuf::from(dir).join("bia"))
            })
            .or_else(|| std::env::home_dir().map(|home| home.join(".config").join("bia")))?;
        let keychain: Box<dyn Keychain> = match std::env::var("BIA_KEYCHAIN").as_deref() {
            Ok("off") => Box::new(NoKeychain),
            _ => Box::new(OsKeychain),
        };
        Some(Self::new(keychain, dir))
    }

    pub(crate) fn file(&self) -> PathBuf {
        self.dir.join(FILE_NAME)
    }

    /// The login for `url`, from the keychain or else the file. Unreadable contents count as
    /// no login: the way out is `bia login` either way.
    pub(crate) fn load(&self, url: &str) -> anyhow::Result<Option<StoredLogin>> {
        let key = key(url);
        if let Ok(Some(value)) = self.keychain.get(&key)
            && let Ok(login) = serde_json::from_str(&value)
        {
            return Ok(Some(login));
        }
        Ok(self.read_file()?.logins.remove(&key))
    }

    /// Keeps `login` in the keychain, or in the file when the keychain cannot.
    pub(crate) fn save(&self, url: &str, login: &StoredLogin) -> anyhow::Result<Stored> {
        let key = key(url);
        let value = serde_json::to_string(login)?;
        if self.keychain.set(&key, &value).is_ok() {
            // A file entry left by an earlier login would shadow nothing, but would outlive
            // `logout` on a machine whose keychain came back.
            self.forget_in_file(&key)?;
            return Ok(Stored::Keychain);
        }
        let mut contents = self.read_file()?;
        contents.logins.insert(key, login.clone());
        self.write_file(&contents)?;
        Ok(Stored::File(self.file()))
    }

    /// Removes the login for `url` wherever it is.
    pub(crate) fn delete(&self, url: &str) -> anyhow::Result<()> {
        let key = key(url);
        // An unavailable keychain has nothing to remove.
        let _ = self.keychain.delete(&key);
        self.forget_in_file(&key)
    }

    fn forget_in_file(&self, key: &str) -> anyhow::Result<()> {
        if !self.file().exists() {
            return Ok(());
        }
        let mut contents = self.read_file()?;
        if contents.logins.remove(key).is_some() {
            self.write_file(&contents)?;
        }
        Ok(())
    }

    fn read_file(&self) -> anyhow::Result<FileContents> {
        let path = self.file();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).with_context(|| {
                format!(
                    "{} is not valid; remove it and log in again",
                    path.display()
                )
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(FileContents::default())
            }
            Err(error) => Err(anyhow!("cannot read {}: {error}", path.display())),
        }
    }

    fn write_file(&self, contents: &FileContents) -> anyhow::Result<()> {
        create_private_dir(&self.dir)?;
        let path = self.file();
        let temporary = self
            .dir
            .join(format!("{FILE_NAME}.{}.tmp", std::process::id()));
        write_private(
            &temporary,
            serde_json::to_string_pretty(contents)?.as_bytes(),
        )
        .with_context(|| format!("cannot write {}", temporary.display()))?;
        std::fs::rename(&temporary, &path)
            .with_context(|| format!("cannot write {}", path.display()))
    }

    /// Holds an exclusive lock until the guard is dropped, so two `bia` processes do not
    /// refresh one login at once: a refresh token is single-use, and the second use of a
    /// spent one revokes the whole Grant. Blocks.
    pub(crate) fn lock(&self) -> anyhow::Result<File> {
        create_private_dir(&self.dir)?;
        let path = self.dir.join(LOCK_NAME);
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        file.lock()
            .with_context(|| format!("cannot lock {}", path.display()))?;
        Ok(file)
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .with_context(|| format!("cannot create {}", dir.display()))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = File::options()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::collections::HashMap;
    use std::sync::Mutex;

    use chrono::TimeDelta;

    use super::*;

    #[derive(Default)]
    struct FakeKeychain(Mutex<HashMap<String, String>>);

    impl Keychain for FakeKeychain {
        fn get(&self, key: &str) -> Result<Option<String>, KeychainUnavailable> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }

        fn set(&self, key: &str, value: &str) -> Result<(), KeychainUnavailable> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_owned());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), KeychainUnavailable> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn login(tag: &str) -> StoredLogin {
        StoredLogin {
            access_token: format!("access-{tag}"),
            refresh_token: format!("refresh-{tag}"),
            expires_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap() + TimeDelta::hours(1),
            scope: "admin".to_owned(),
        }
    }

    #[test]
    fn a_login_goes_to_the_keychain_when_there_is_one_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = Credentials::new(Box::<FakeKeychain>::default(), dir.path().to_owned());

        let stored = store.save("http://localhost:8080/", &login("a")).unwrap();

        assert_eq!(stored, Stored::Keychain);
        assert!(!store.file().exists());
        assert_eq!(
            store.load("http://localhost:8080").unwrap(),
            Some(login("a"))
        );
        assert_eq!(store.load("http://localhost:9090").unwrap(), None);
        store.delete("http://localhost:8080").unwrap();
        assert_eq!(store.load("http://localhost:8080").unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn without_a_keychain_the_file_is_private_and_holds_one_login_per_server() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config");
        let store = Credentials::new(Box::new(NoKeychain), config.clone());

        let stored = store.save("http://one.test", &login("1")).unwrap();
        store.save("http://two.test", &login("2")).unwrap();
        store.save("http://one.test", &login("3")).unwrap();

        assert_eq!(stored, Stored::File(config.join("credentials.json")));
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&store.file()), 0o600);
        assert_eq!(mode(&config), 0o700);
        assert_eq!(store.load("http://one.test").unwrap(), Some(login("3")));
        assert_eq!(store.load("http://two.test").unwrap(), Some(login("2")));
        store.delete("http://one.test").unwrap();
        assert_eq!(store.load("http://one.test").unwrap(), None);
        assert_eq!(store.load("http://two.test").unwrap(), Some(login("2")));
    }

    #[test]
    fn a_login_in_the_file_is_found_and_removed_when_the_keychain_has_nothing() {
        let dir = tempfile::tempdir().unwrap();
        Credentials::new(Box::new(NoKeychain), dir.path().to_owned())
            .save("http://one.test", &login("1"))
            .unwrap();
        let store = Credentials::new(Box::<FakeKeychain>::default(), dir.path().to_owned());

        assert_eq!(store.load("http://one.test").unwrap(), Some(login("1")));
        store.delete("http://one.test").unwrap();
        assert_eq!(store.load("http://one.test").unwrap(), None);
    }

    #[test]
    fn debug_output_shows_no_token() {
        let shown = format!("{:?}", login("secret"));

        assert!(!shown.contains("secret"), "{shown}");
    }
}
