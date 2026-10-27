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

use crate::{
	backend::{Backend, Cancel, Collect, Outcome, Rows, Sink},
	catalog::Catalog,
};

/// Work for the thread.
pub enum Job {
	/// Run batches in order, keeping up to `limit` rows per result set.
	Run { batches: Vec<String>, limit: usize },
	/// Run a statement and write every row to a CSV file.
	Export { sql: String, path: PathBuf },
	/// Switch database, then load its catalog.
	Switch(String),
	/// Load the current database's catalog.
	Catalog,
}

/// Everything a script run produced.
pub struct RunResult {
	/// Result sets in order.
	pub sets:     Vec<Rows>,
	/// Lines for the Messages tab, in order.
	pub messages: Vec<String>,
	/// Wall time.
	pub ms:       u128,
	/// The error that stopped the run.
	pub error:    Option<String>,
}

/// What a job produced. Errors are display text.
pub enum Reply {
	/// A script finished.
	Ran(RunResult),
	/// An export finished: the file and its row count.
	Exported(Result<(PathBuf, usize), String>),
	/// A database switch finished: the name and its catalog.
	Switched(String, Result<Catalog, String>),
	/// The catalog was (re)loaded.
	Catalog(Result<Catalog, String>),
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
		Job::Run { batches, limit } => match script(db, &batches, limit, cancel) {
			Some(r) => Reply::Ran(r),
			None => Reply::Cancelled,
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
			let result = db.use_database(&name).and_then(|()| Catalog::load(db));
			Reply::Switched(name, result.map_err(|e| format!("{e:#}")))
		},
		Job::Catalog => Reply::Catalog(Catalog::load(db).map_err(|e| format!("{e:#}"))),
	}
}

/// Runs batches until one fails. `None` when cancelled.
fn script<B: Backend>(db: &mut B, batches: &[String], limit: usize, cancel: &Cancel) -> Option<RunResult> {
	let started = Instant::now();
	let mut sink = Collect::new(limit);
	let mut messages = Vec::new();
	let mut error = None;
	for sql in batches {
		let (sets_before, aff_before) = (sink.sets.len(), sink.affected.len());
		match db.stream(sql, cancel, &mut sink) {
			Ok(Outcome::Done) => {},
			Ok(Outcome::Cancelled) => return None,
			Err(e) => {
				error = Some(format!("{e:#}"));
				break;
			},
		}
		for set in &sink.sets[sets_before..] {
			messages.push(format!("({} row{} returned{})", set.rows.len(), plural(set.rows.len()), if set.capped { ", capped; later results skipped" } else { "" }));
		}
		for n in &sink.affected[aff_before..] {
			messages.push(format!("({n} row{} affected)", plural(*n as usize)));
		}
		if sink.sets.len() == sets_before && sink.affected.len() == aff_before {
			messages.push("Commands completed successfully.".to_owned());
		}
	}
	if let Some(e) = &error {
		messages.push(e.clone());
	}
	Some(RunResult { sets: sink.sets, messages, ms: started.elapsed().as_millis(), error })
}

/// `s` unless `n` is one.
const fn plural(n: usize) -> &'static str {
	if n == 1 { "" } else { "s" }
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

/// Streams a statement to `path`. `None` when cancelled.
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
