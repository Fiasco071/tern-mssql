//! The database surface the UI talks to. Mirrors Tern's built-in SQLite API
//! (`tables`, `schema`, `query` with a row cap) so other engines can slot in.

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
	/// User tables and views as `schema.name`.
	fn tables(&mut self) -> anyhow::Result<Vec<String>>;
	/// Runs one statement and returns at most `limit` rows.
	fn query(&mut self, sql: &str, limit: usize) -> anyhow::Result<Rows>;
}
