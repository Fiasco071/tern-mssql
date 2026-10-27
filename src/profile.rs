//! Connection profiles. A profile names the env var that holds the password,
//! so the file never carries a secret and the password never rides on a
//! command line.

use std::path::PathBuf;

use anyhow::{Context, bail};
use serde::Deserialize;

/// One saved server.
#[derive(Deserialize)]
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
	/// Env var holding the password.
	pub password_env: String,
	/// Database to start in (default `master`).
	#[serde(default)]
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

/// Every saved profile.
pub fn load() -> anyhow::Result<Vec<Profile>> {
	let path = path();
	let text = std::fs::read_to_string(&path)
		.with_context(|| format!("no profiles at {}", path.display()))?;
	serde_json::from_str(&text).with_context(|| format!("bad JSON in {}", path.display()))
}

impl Profile {
	/// The ADO connection string, with the password read from its env var.
	pub fn ado(&self) -> anyhow::Result<String> {
		let Ok(pw) = std::env::var(&self.password_env) else {
			bail!("env var {} is not set", self.password_env);
		};
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
