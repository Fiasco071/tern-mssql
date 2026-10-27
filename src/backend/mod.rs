//! The database surface the UI talks to. Mirrors Tern's built-in SQLite API
//! (`tables`, `query` with a row cap) so other engines can slot in.

pub mod mssql;

/// Positional result set; every cell is already display text, `None` is NULL.
pub struct Rows {
	/// Column names in order.
	pub columns: Vec<String>,
	/// Row cells aligned with `columns`.
	pub rows:    Vec<Vec<Option<String>>>,
	/// More rows existed than the cap allowed.
	pub capped:  bool,
}

/// A connected database.
pub trait Backend {
	/// Online databases on the server.
	fn databases(&mut self) -> anyhow::Result<Vec<String>>;
	/// The database the connection is in.
	fn current(&mut self) -> anyhow::Result<String>;
	/// Switches the connection to `name`.
	fn use_database(&mut self, name: &str) -> anyhow::Result<()>;
	/// User tables and views of the current database as `schema.name`.
	fn tables(&mut self) -> anyhow::Result<Vec<String>>;
	/// Runs one batch and returns at most `limit` rows.
	fn query(&mut self, sql: &str, limit: usize) -> anyhow::Result<Rows>;
}
