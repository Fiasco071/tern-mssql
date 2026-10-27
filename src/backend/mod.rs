//! The database surface the UI talks to. Mirrors Tern's built-in SQLite API
//! (`tables`, `query` with a row cap) so other engines can slot in. One
//! streaming call, `stream`, serves the grid, the row cap and CSV export.

use std::sync::Arc;

use tokio::sync::Notify;

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

/// Asks a running `stream` to stop. Clone it to the thread that cancels.
#[derive(Clone, Default)]
pub struct Cancel(Arc<Notify>);

impl Cancel {
	/// A token that has not fired.
	pub fn new() -> Self {
		Self::default()
	}

	/// Fires the token; a `stream` that starts later still sees it.
	pub fn cancel(&self) {
		self.0.notify_one();
	}

	/// Resolves once the token fires.
	pub async fn cancelled(&self) {
		self.0.notified().await;
	}
}

/// What a `stream` call ended with.
pub enum Outcome {
	/// The sink stopped early, or the batch ended.
	Done,
	/// The token fired; the in-flight statement was abandoned.
	Cancelled,
}

/// Receives a batch as it arrives.
pub trait Sink {
	/// A statement that returns no rows changed `n` rows.
	fn affected(&mut self, _n: u64) {}
	/// The column names, once, before any row.
	fn columns(&mut self, columns: &[String]);
	/// One row. Returns false to stop reading.
	fn row(&mut self, row: Vec<Option<String>>) -> bool;
}

/// Collects every result set, up to `limit` rows each, and the rows-affected counts.
pub struct Collect {
	/// The result sets so far.
	pub sets:     Vec<Rows>,
	/// Rows changed by statements that returned none, in order.
	pub affected: Vec<u64>,
	limit:        usize,
}

impl Collect {
	/// An empty collector with a per-set row cap.
	pub const fn new(limit: usize) -> Self {
		Self { sets: Vec::new(), affected: Vec::new(), limit }
	}
}

impl Sink for Collect {
	fn columns(&mut self, columns: &[String]) {
		self.sets.push(Rows { columns: columns.to_vec(), rows: Vec::new(), capped: false });
	}

	fn row(&mut self, row: Vec<Option<String>>) -> bool {
		let Some(set) = self.sets.last_mut() else { return true };
		if set.rows.len() >= self.limit {
			set.capped = true;
			return false;
		}
		set.rows.push(row);
		true
	}

	fn affected(&mut self, n: u64) {
		self.affected.push(n);
	}
}

/// A connected database.
pub trait Backend {
	/// Runs one batch, feeding `sink`, until it ends, the sink stops or `cancel` fires.
	fn stream(&mut self, sql: &str, cancel: &Cancel, sink: &mut dyn Sink) -> anyhow::Result<Outcome>;
	/// Switches the connection to `name`.
	fn use_database(&mut self, name: &str) -> anyhow::Result<()>;

	/// Runs one batch and returns at most `limit` rows.
	fn query(&mut self, sql: &str, limit: usize) -> anyhow::Result<Rows> {
		let mut sink = Collect::new(limit);
		self.stream(sql, &Cancel::new(), &mut sink)?;
		Ok(sink.sets.into_iter().next().unwrap_or(Rows { columns: Vec::new(), rows: Vec::new(), capped: false }))
	}

	/// The first column of every row, as text.
	fn column(&mut self, sql: &str) -> anyhow::Result<Vec<String>> {
		let r = self.query(sql, usize::MAX)?;
		Ok(r.rows.into_iter().filter_map(|mut r| r.drain(..).next().flatten()).collect())
	}

	/// Online databases on the server.
	fn databases(&mut self) -> anyhow::Result<Vec<String>> {
		self.column("SELECT name FROM sys.databases WHERE state = 0 ORDER BY name")
	}

	/// The database the connection is in.
	fn current(&mut self) -> anyhow::Result<String> {
		Ok(self.column("SELECT DB_NAME()")?.into_iter().next().unwrap_or_default())
	}
}
