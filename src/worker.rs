//! Runs database work on its own thread so the UI keeps drawing and can
//! cancel. One job at a time; the UI polls for the reply.

use std::{
	fs::File,
	io::{BufWriter, Write},
	path::PathBuf,
	sync::mpsc::{Receiver, Sender, channel},
	thread,
	time::Instant,
};

use crate::backend::{Backend, Cancel, Collect, Outcome, Rows, Sink};

/// Work for the thread.
pub enum Job {
	/// Run a batch and keep up to `limit` rows.
	Query { sql: String, limit: usize },
	/// Run a batch and write every row to a CSV file.
	Export { sql: String, path: PathBuf },
	/// Switch database and list its tables.
	Switch(String),
}

/// What a job produced. Errors are display text.
pub enum Reply {
	/// A query finished.
	Rows(Result<Rows, String>, u128),
	/// An export finished: the file and its row count.
	Exported(Result<(PathBuf, usize), String>),
	/// A database switch finished: the name and its tables.
	Switched(String, Result<Vec<String>, String>),
	/// The job was cancelled and the connection replaced.
	Cancelled,
}

/// The UI's handle on the thread.
pub struct Worker {
	jobs:    Sender<(Job, Cancel)>,
	replies: Receiver<Reply>,
}

impl Worker {
	/// Moves `db` onto a new thread.
	pub fn spawn<B: Backend + Send + 'static>(mut db: B) -> Self {
		let (jobs, rx) = channel::<(Job, Cancel)>();
		let (tx, replies) = channel();
		thread::spawn(move || {
			for (job, cancel) in rx {
				if tx.send(run(&mut db, job, &cancel)).is_err() {
					break;
				}
			}
		});
		Self { jobs, replies }
	}

	/// Queues a job; returns the token that cancels it.
	pub fn submit(&self, job: Job) -> Cancel {
		let cancel = Cancel::new();
		// The thread only ends when this handle drops, so the send cannot fail.
		let _ = self.jobs.send((job, cancel.clone()));
		cancel
	}

	/// The reply to the running job, if it has finished.
	pub fn poll(&self) -> Option<Reply> {
		self.replies.try_recv().ok()
	}
}

/// Runs one job to its reply.
fn run<B: Backend>(db: &mut B, job: Job, cancel: &Cancel) -> Reply {
	match job {
		Job::Query { sql, limit } => {
			let started = Instant::now();
			let mut sink = Collect::new(limit);
			match db.stream(&sql, cancel, &mut sink) {
				Ok(Outcome::Done) => Reply::Rows(Ok(sink.rows), started.elapsed().as_millis()),
				Ok(Outcome::Cancelled) => Reply::Cancelled,
				Err(e) => Reply::Rows(Err(format!("{e:#}")), started.elapsed().as_millis()),
			}
		},
		Job::Export { sql, path } => match export(db, &sql, &path, cancel) {
			Ok(Some(n)) => Reply::Exported(Ok((path, n))),
			Ok(None) => {
				let _ = std::fs::remove_file(&path);
				Reply::Cancelled
			},
			Err(e) => {
				let _ = std::fs::remove_file(&path);
				Reply::Exported(Err(format!("{e:#}")))
			},
		},
		Job::Switch(name) => {
			let result = db.use_database(&name).and_then(|()| db.tables());
			Reply::Switched(name, result.map_err(|e| format!("{e:#}")))
		},
	}
}

/// One CSV field: quoted when it holds a comma, quote or line break.
fn field(s: &str) -> String {
	if s.contains([',', '"', '\r', '\n']) {
		format!("\"{}\"", s.replace('"', "\"\""))
	} else {
		s.to_owned()
	}
}

/// Writes rows as CSV (NULL is an empty field), remembering the first I/O error.
struct Csv {
	out:   BufWriter<File>,
	rows:  usize,
	error: Option<std::io::Error>,
}

impl Csv {
	/// Writes one line unless an earlier write failed.
	fn line(&mut self, cells: impl Iterator<Item = String>) {
		if self.error.is_some() {
			return;
		}
		let line = cells.collect::<Vec<_>>().join(",");
		if let Err(e) = writeln!(self.out, "{line}") {
			self.error = Some(e);
		}
	}
}

impl Sink for Csv {
	fn columns(&mut self, columns: &[String]) {
		self.line(columns.iter().map(|c| field(c)));
	}

	fn row(&mut self, row: Vec<Option<String>>) -> bool {
		self.rows += 1;
		self.line(row.iter().map(|c| c.as_deref().map(field).unwrap_or_default()));
		self.error.is_none()
	}
}

/// Streams a batch to `path`. `None` when cancelled.
fn export<B: Backend>(db: &mut B, sql: &str, path: &PathBuf, cancel: &Cancel) -> anyhow::Result<Option<usize>> {
	let mut csv = Csv { out: BufWriter::new(File::create(path)?), rows: 0, error: None };
	let outcome = db.stream(sql, cancel, &mut csv)?;
	if let Some(e) = csv.error {
		return Err(e.into());
	}
	csv.out.flush()?;
	Ok(match outcome {
		Outcome::Done => Some(csv.rows),
		Outcome::Cancelled => None,
	})
}
