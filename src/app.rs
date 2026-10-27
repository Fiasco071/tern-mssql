//! The interactive client: an explorer tree, a results table and a query
//! editor on one full-screen Tern surface.

use std::{path::PathBuf, time::{Duration, Instant}};

use tern_sdk::{
	Input, Options, Session, SurfaceOptions, nodes,
	ui::{self, Column, Side, TableRow, Tone, TreeItem, View},
	wire::feature,
};

use crate::{
	backend::{Backend, Cancel, Rows},
	guard,
	worker::{Job, Reply, Worker},
};

/// Rows kept per result.
const LIMIT: usize = 1000;
/// The editor's id: `dock` region, key `ed`.
const EDITOR: &str = "dock.ed";
/// The tree's id inside `main`.
const TREE: &str = "main.body.tree";

/// What node handlers report.
#[derive(Clone, Debug)]
enum Msg {
	/// A tree row was activated (`db:NAME` or `t:SCHEMA.TABLE`).
	Activate(String),
	/// The editor was clicked.
	Focus,
}

/// What the results area shows.
enum Shown {
	/// Nothing run yet.
	Nothing,
	/// A result set.
	Rows(Rows),
	/// An error message.
	Error(String),
}

/// The job the worker is running.
struct Busy {
	/// When it started.
	since:  Instant,
	/// What it is, for the status strip.
	what:   &'static str,
	/// Stops it.
	cancel: Cancel,
	/// Cancel was requested.
	asked:  bool,
}

/// The client's state.
struct App {
	/// The database thread.
	worker:    Worker,
	/// The running job.
	busy:      Option<Busy>,
	/// Profile name, for the status strip.
	profile:   String,
	/// Databases on the server.
	databases: Vec<String>,
	/// The current database.
	current:   String,
	/// Its tables.
	tables:    Vec<String>,
	/// The query text.
	draft:     String,
	/// The caret, in chars.
	cursor:    usize,
	/// What the results area shows.
	shown:     Shown,
	/// A note for the status strip (timing).
	note:      String,
	/// Writes allowed.
	write:     bool,
	/// Keys go to the editor (else the explorer).
	in_editor: bool,
}

/// `[schema].[name]` from `schema.name`.
fn quoted(full: &str) -> String {
	full.split_once('.').map_or_else(
		|| format!("[{}]", full.replace(']', "]]")),
		|(s, n)| format!("[{}].[{}]", s.replace(']', "]]"), n.replace(']', "]]")),
	)
}

/// A cell for the table: newlines collapsed so rows stay one line tall.
fn flat(cell: &str) -> String {
	cell.replace(['\r', '\n'], " ")
}

impl App {
	/// Loads the explorer, then hands the connection to the worker thread.
	fn new<B: Backend + Send + 'static>(mut db: B, profile: String) -> anyhow::Result<Self> {
		let databases = db.databases()?;
		let current = db.current()?;
		let tables = db.tables()?;
		Ok(Self {
			worker: Worker::spawn(db),
			busy: None,
			profile,
			databases,
			current,
			tables,
			draft: String::new(),
			cursor: 0,
			shown: Shown::Nothing,
			note: String::new(),
			write: false,
			in_editor: true,
		})
	}

	/// The byte offset of a char index in the draft.
	fn byte(&self, chars: usize) -> usize {
		self
			.draft
			.char_indices()
			.nth(chars)
			.map_or(self.draft.len(), |(i, _)| i)
	}

	/// Inserts typed or pasted text at the caret.
	fn insert(&mut self, text: &str) {
		let at = self.byte(self.cursor);
		self.draft.insert_str(at, text);
		self.cursor += text.chars().count();
	}

	/// Replaces the draft.
	fn set_draft(&mut self, text: String) {
		self.cursor = text.chars().count();
		self.draft = text;
	}

	/// Moves the caret one line up or down, keeping its column.
	fn vertical(&mut self, down: bool) {
		let chars: Vec<char> = self.draft.chars().collect();
		let line_start =
			|at: usize| chars[..at].iter().rposition(|c| *c == '\n').map_or(0, |i| i + 1);
		let start = line_start(self.cursor);
		let col = self.cursor - start;
		if down {
			let Some(nl) = chars[self.cursor..].iter().position(|c| *c == '\n') else {
				self.cursor = chars.len();
				return;
			};
			let next = self.cursor + nl + 1;
			let len = chars[next..]
				.iter()
				.position(|c| *c == '\n')
				.unwrap_or(chars.len() - next);
			self.cursor = next + col.min(len);
		} else {
			if start == 0 {
				self.cursor = 0;
				return;
			}
			let prev = line_start(start - 1);
			self.cursor = prev + col.min(start - 1 - prev);
		}
	}

	/// Applies one editor key. Returns whether it changed anything.
	fn edit(&mut self, key: &tern_sdk::keys::Key) -> bool {
		match key.name.as_str() {
			"enter" => self.insert("\n"),
			"backspace" if self.cursor > 0 => {
				let at = self.byte(self.cursor - 1);
				self.draft.remove(at);
				self.cursor -= 1;
			},
			"delete" if self.cursor < self.draft.chars().count() => {
				let at = self.byte(self.cursor);
				self.draft.remove(at);
			},
			"left" => self.cursor = self.cursor.saturating_sub(1),
			"right" => self.cursor = (self.cursor + 1).min(self.draft.chars().count()),
			"up" => self.vertical(false),
			"down" => self.vertical(true),
			"home" => {
				let chars: Vec<char> = self.draft.chars().collect();
				self.cursor = chars[..self.cursor]
					.iter()
					.rposition(|c| *c == '\n')
					.map_or(0, |i| i + 1);
			},
			"end" => {
				let chars: Vec<char> = self.draft.chars().collect();
				self.cursor += chars[self.cursor..]
					.iter()
					.position(|c| *c == '\n')
					.unwrap_or(chars.len() - self.cursor);
			},
			"paste" => self.insert(key.text.as_deref().unwrap_or_default()),
			_ => match key.typed() {
				Some(text) if !key.ctrl && !key.meta => self.insert(text),
				_ => return false,
			},
		}
		true
	}

	/// Starts `job` unless one is running.
	fn start(&mut self, job: Job, what: &'static str) {
		if self.busy.is_some() {
			self.note = "busy: esc cancels the running job".into();
			return;
		}
		let cancel = self.worker.submit(job);
		self.busy = Some(Busy { since: Instant::now(), what, cancel, asked: false });
		self.note = format!("{what}... (esc cancels)");
	}

	/// The draft, if it may run: not empty and past the read-only guard.
	fn runnable(&mut self) -> Option<String> {
		let sql = self.draft.trim().to_owned();
		if sql.is_empty() {
			return None;
		}
		if !self.write {
			if let Some(word) = guard::check(&sql) {
				self.shown = Shown::Error(format!(
					"Read-only mode refused `{word}`. Press ctrl+w to allow writes."
				));
				return None;
			}
		}
		Some(sql)
	}

	/// Runs the draft.
	fn run(&mut self) {
		if let Some(sql) = self.runnable() {
			self.start(Job::Query { sql, limit: LIMIT }, "running");
		}
	}

	/// Writes the draft's full result to a CSV file in the pane's directory.
	fn export(&mut self) {
		if let Some(sql) = self.runnable() {
			let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
			let db: String = self
				.current
				.chars()
				.map(|c| if c.is_alphanumeric() { c } else { '_' })
				.collect();
			let path = PathBuf::from(format!("tern-mssql-{db}-{stamp}.csv"));
			self.start(Job::Export { sql, path }, "exporting");
		}
	}

	/// Asks the running job to stop.
	fn cancel(&mut self) {
		if let Some(b) = self.busy.as_mut().filter(|b| !b.asked) {
			b.cancel.cancel();
			b.asked = true;
			self.note = "cancelling...".into();
		}
	}

	/// Takes the worker's reply, if the job finished. Returns whether anything changed.
	fn poll(&mut self) -> bool {
		let Some(reply) = self.worker.poll() else {
			if let Some(b) = self.busy.as_ref().filter(|b| !b.asked) {
				self.note = format!("{} {}s (esc cancels)", b.what, b.since.elapsed().as_secs());
				return true;
			}
			return false;
		};
		let ms = self.busy.take().map_or(0, |b| b.since.elapsed().as_millis());
		match reply {
			Reply::Rows(Ok(rows), took) => {
				self.note = format!(
					"{} rows in {took} ms{}",
					rows.rows.len(),
					if rows.capped { " (capped, ctrl+e exports all)" } else { "" }
				);
				self.shown = Shown::Rows(rows);
			},
			Reply::Rows(Err(e), _) | Reply::Exported(Err(e)) => {
				self.note.clear();
				self.shown = Shown::Error(e);
			},
			Reply::Exported(Ok((path, n))) => {
				let full = std::fs::canonicalize(&path).unwrap_or(path);
				self.note = format!("{n} rows in {ms} ms to {}", full.display());
			},
			Reply::Switched(name, Ok(tables)) => {
				self.current = name;
				self.tables = tables;
				self.note.clear();
			},
			Reply::Switched(_, Err(e)) => self.shown = Shown::Error(e),
			Reply::Cancelled => self.note = "cancelled".into(),
		}
		true
	}

	/// A tree row was activated.
	fn activate(&mut self, id: &str) {
		if let Some(name) = id.strip_prefix("db:") {
			if name != self.current {
				self.start(Job::Switch(name.to_owned()), "switching");
			}
		} else if let Some(table) = id.strip_prefix("t:") {
			self.set_draft(format!("SELECT TOP (100) * FROM {}", quoted(table)));
			self.run();
		}
	}

	/// The explorer: databases, with the current one's tables nested.
	fn explorer(&self) -> ui::Tree<Msg> {
		let mut tree = ui::tree()
			.key("tree")
			.basis(0.25)
			.grow(0.0)
			.on_activate(|id| Msg::Activate(id.to_owned()));
		for d in &self.databases {
			let mut item = TreeItem::new(format!("db:{d}"), d.clone());
			if *d == self.current {
				item = item.open(true);
				for t in &self.tables {
					item = item.child(TreeItem::new(format!("t:{t}"), t.clone()));
				}
			}
			tree = tree.item(item);
		}
		tree
	}

	/// The results area.
	fn results(&self) -> ui::Col<Msg> {
		let col = ui::col().key("res").grow(1.0).basis(0.75);
		match &self.shown {
			Shown::Nothing => col.child(
				ui::md("Run a query with **F5** or **ctrl+enter**, or open a table in the explorer.")
					.key("hint"),
			),
			Shown::Error(e) => col.child(
				ui::card()
					.key("err")
					.tone(Tone::Error)
					.child(ui::text(e.clone()).key("t")),
			),
			Shown::Rows(r) => {
				let mut table = ui::table().key("grid");
				for (i, name) in r.columns.iter().enumerate() {
					table = table.col(Column::new(format!("c{i}"), name.clone()));
				}
				for (n, row) in r.rows.iter().enumerate() {
					let mut tr = TableRow::new(n.to_string());
					for (i, c) in row.iter().enumerate() {
						tr = tr.cell(
							format!("c{i}"),
							c.as_deref().map_or_else(|| "NULL".to_owned(), flat),
						);
					}
					table = table.row(tr);
				}
				col.child(table)
			},
		}
	}

	/// The whole view.
	fn view(&self) -> View<Msg> {
		let cursor: usize = self.draft.chars().take(self.cursor).map(char::len_utf16).sum();
		let body = ui::row().key("body").child(self.explorer()).child(self.results());
		View::new().main(ui::col().child(body)).dock(nodes![
			ui::editor()
				.key("ed")
				.text(self.draft.clone())
				.cursor(cursor)
				.lang("sql")
				.placeholder("SQL: F5 runs, ctrl+e exports CSV, esc cancels, tab switches pane, ctrl+q quits")
				.on_focus(Msg::Focus),
			ui::status()
				.key("bar")
				.child(ui::seg(format!("{} / {}", self.profile, self.current)).icon("database"))
				.child(ui::seg(self.note.clone()).tone(Tone::Muted))
				.child(
					ui::seg(if self.write { "WRITE" } else { "read-only" })
						.side(Side::Right)
						.tone(if self.write { Tone::Warning } else { Tone::Success }),
				),
		])
	}
}

/// Runs the client until the user quits.
pub fn run<B: Backend + Send + 'static>(db: B, profile: String) -> anyhow::Result<()> {
	let Some(mut session) = Session::connect(Options::new().feature(feature::SEND))? else {
		anyhow::bail!(
			"interactive mode needs a terminal that speaks TSP (Tern); use `tern-mssql query` instead"
		);
	};
	let mut app = App::new(db, profile)?;
	let sf = session.open(SurfaceOptions::screen().title("SQL Server"))?;
	session.render(sf, app.view())?;
	session.focus(sf, Some(EDITOR))?;
	loop {
		// Poll while a job runs so the clock ticks and its reply shows up; block when idle.
		let wait = app.busy.is_some().then_some(Duration::from_millis(100));
		let input = match session.next(wait) {
			Ok(input) => input,
			Err(tern_sdk::Error::InputClosed) => break,
			Err(e) => return Err(e.into()),
		};
		if app.poll() && input.is_none() {
			session.render(sf, app.view())?;
		}
		let Some(input) = input else { continue };
		match input {
			Input::Key(key) => {
				if key.is("ctrl+q") || key.is("ctrl+c") {
					break;
				}
				if key.is("f5") || (key.name == "enter" && key.ctrl) {
					app.run();
				} else if key.is("ctrl+e") {
					app.export();
				} else if key.is("escape") || key.is("ctrl+x") {
					app.cancel();
				} else if key.is("ctrl+w") {
					app.write = !app.write;
				} else if key.name == "tab" {
					app.in_editor = !app.in_editor;
					session.focus(sf, Some(if app.in_editor { EDITOR } else { TREE }))?;
				} else if !(app.in_editor && app.edit(&key)) {
					continue;
				}
			},
			Input::Msg(Msg::Activate(id), _) => {
				app.activate(&id);
				app.in_editor = true;
				session.focus(sf, Some(EDITOR))?;
			},
			Input::Msg(Msg::Focus, _) => {
				app.in_editor = true;
				session.focus(sf, Some(EDITOR))?;
			},
			_ => continue,
		}
		session.render(sf, app.view())?;
	}
	session.close()?;
	Ok(())
}
