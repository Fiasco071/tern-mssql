//! Connection profiles. `profiles.json` never carries a secret: a profile
//! either names the env var that holds the password or relies on one stored
//! in the OS credential store (Credential Manager, Keychain, Secret Service).
//! The password never rides on a command line.

use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

/// The credential store's service name.
const SERVICE: &str = "tern-mssql";

/// One saved server.
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Profile {
	/// The name typed on the command line.
	pub name:         String,
	/// Host name or address.
	pub server:       String,
	/// TCP port (default 1433).
	#[serde(default = "port")]
	pub port:         u16,
	/// SQL login.
	pub user:         String,
	/// Env var holding the password. `None` means the credential store holds it.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub password_env: Option<String>,
	/// Database to start in (default `master`).
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub database:     Option<String>,
	/// Accept the server's certificate without validating it.
	#[serde(default)]
	pub trust_cert:   bool,
}

const fn port() -> u16 {
	1433
}

/// `profiles.json`: `%APPDATA%\tern-mssql` on Windows, `$XDG_CONFIG_HOME` or `~/.config` elsewhere.
pub fn path() -> PathBuf {
	let base = if cfg!(windows) {
		std::env::var_os("APPDATA").map(PathBuf::from)
	} else {
		std::env::var_os("XDG_CONFIG_HOME")
			.map(PathBuf::from)
			.or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
	};
	base.unwrap_or_default().join("tern-mssql").join("profiles.json")
}

/// Every saved profile (none when the file does not exist yet).
pub fn load() -> anyhow::Result<Vec<Profile>> {
	let path = path();
	match std::fs::read_to_string(&path) {
		Ok(text) => serde_json::from_str(&text).with_context(|| format!("bad JSON in {}", path.display())),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
		Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
	}
}

/// Writes every profile.
pub fn store(profiles: &[Profile]) -> anyhow::Result<()> {
	let path = path();
	if let Some(dir) = path.parent() {
		std::fs::create_dir_all(dir)?;
	}
	std::fs::write(&path, serde_json::to_string_pretty(profiles)?).with_context(|| format!("writing {}", path.display()))
}

/// The credential store entry for a profile.
fn entry(name: &str) -> anyhow::Result<keyring::Entry> {
	keyring::Entry::new(SERVICE, name).context("opening the credential store")
}

/// Saves a profile's password in the credential store.
pub fn set_secret(name: &str, password: &str) -> anyhow::Result<()> {
	entry(name)?.set_password(password).context("saving the password")
}

/// Removes a profile's stored password (fine if there is none).
pub fn forget_secret(name: &str) {
	if let Ok(e) = entry(name) {
		let _ = e.delete_credential();
	}
}

impl Profile {
	/// The password: from the env var if one is named, else the credential store.
	fn password(&self) -> anyhow::Result<String> {
		match &self.password_env {
			Some(var) => std::env::var(var).map_err(|_| anyhow::anyhow!("env var {var} is not set")),
			None => entry(&self.name)?
				.get_password()
				.with_context(|| format!("no stored password for {} (edit the connection and enter one)", self.name)),
		}
	}

	/// The ADO connection string.
	pub fn ado(&self) -> anyhow::Result<String> {
		self.ado_with(&self.password()?)
	}

	/// The ADO connection string with an explicit password.
	pub fn ado_with(&self, pw: &str) -> anyhow::Result<String> {
		if self.server.trim().is_empty() {
			bail!("server is empty");
		}
		let mut s = format!(
			"server=tcp:{},{};user id={};password={{{}}};encrypt=true;database={}",
			self.server,
			self.port,
			self.user,
			pw.replace('}', "}}"),
			self.database.as_deref().unwrap_or("master"),
		);
		if self.trust_cert {
			s.push_str(";TrustServerCertificate=true");
		}
		Ok(s)
	}
}
