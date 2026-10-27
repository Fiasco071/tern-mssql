//! The interactive client: an explorer, query buffers and results on one
//! full-screen Tern surface.

use std::{
	collections::{HashSet, VecDeque},
	path::PathBuf,
	time::{Duration, Instant},
};

use tern_sdk::{
	Input, Options, Session, SurfaceOptions, nodes,
	ui::{self, Column, Side, Span, TableRow, Tone, TreeItem, View},
	wire::{self, Event, feature},
};

use crate::{
	backend::{Backend, Cancel, Rows},
	catalog::{Catalog, ObjKind},
	complete::{self, Completion, ItemKind},
	guard, tsql,
	worker::{Job, Reply, RunResult, Worker},
};

/// Rows kept per result set.
const LIMIT: usize = 1000;
/// The query editor's id.
const EDITOR: &str = "main.body.work.ed";
/// The explorer's id.
const TREE: &str = "main.body.tree";

/// What node handlers report.
#[derive(Clone, Debug)]
enum Msg {
	/// A tree row was activated.
	Activate(String),
	/// A tree row was folded or unfolded (the event says which).
	Toggle,
	/// Native editing changed the buffer.
	Edit(wire::Edit),
	/// Undo was asked for.
	Undo,
	/// The editor was clicked.
	Focus,
	/// A completion was chosen from the list.
	Pick(String),
	/// A query tab was chosen.
	Tab(String),
	/// A results tab was chosen.
	ResultTab(String),
}

/// One query buffer.
struct Buffer {
	name:        String,
	path:        Option<PathBuf>,
	text:        String,
	/// Caret, UTF-16.
	cursor:      usize,
	/// Snapshots to undo to.
	undo:        Vec<(String, usize)>,
	/// The last edit typed one character, so the next one joins its undo step.
	last_single: bool,
	dirty:       bool,
}

/// The byte offset of a UTF-16 offset.
fn byte_of(s: &str, units: usize) -> usize {
	let mut n = 0;
	for (i, c) in s.char_indices() {
		if n >= units {
			return i;
		}
		n += c.len_utf16();
	}
	s.len()
}

/// UTF-16 length.
fn u16_len(s: &str) -> usize {
	s.chars().map(char::len_utf16).sum()
}

impl Buffer {
	fn new(name: String, text: String) -> Self {
		let cursor = u16_len(&text);
		Self { name, path: None, text, cursor, undo: Vec::new(), last_single: false, dirty: false }
	}

	/// The caret as a byte offset.
	fn cursor_byte(&self) -> usize {
		byte_of(&self.text, self.cursor)
	}

	/// `(line, column)`, one-based.
	fn line_col(&self) -> (usize, usize) {
		let upto = &self.text[..self.cursor_byte()];
		let line = upto.matches('\n').count() + 1;
		let col = upto.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
		(line, col)
	}

	/// Applies a native edit. False when Tern's text length disagrees with ours.
	fn apply(&mut self, e: &wire::Edit) -> bool {
		if e.len != u16_len(&self.text) {
			return false;
		}
		let single = e.from == e.to && u16_len(&e.text) == 1 && e.text != "\n";
		if !(single && self.last_single) {
			self.undo.push((self.text.clone(), self.cursor));
			if self.undo.len() > 500 {
				self.undo.remove(0);
			}
		}
		self.last_single = single;
		let (a, b) = (byte_of(&self.text, e.from), byte_of(&self.text, e.to));
		self.text.replace_range(a.min(b)..b.max(a), &e.text);
		self.cursor = e.cursor;
		self.dirty = true;
		true
	}



	/// Edits from a plain key event, for terminals that don't send native edits. False when the key is not an edit.
	fn key(&mut self, key: &tern_sdk::keys::Key) -> bool {
		let c = self.cursor_byte();
		let single_insert = |s: &str| u16_len(s) == 1 && s != "\n";
		let (mutates, joins) = match key.name.as_str() {
			"enter" => (true, false),
			"tab" => (true, false),
			"backspace" => (c > 0, false),
			"delete" => (c < self.text.len(), false),
			"paste" => (true, false),
			_ => match key.typed() {
				Some(s) if !key.ctrl && !key.meta => (true, single_insert(s)),
				_ => (false, false),
			},
		};
		if mutates && !(joins && self.last_single) {
			self.undo.push((self.text.clone(), self.cursor));
			if self.undo.len() > 500 {
				self.undo.remove(0);
			}
		}
		if mutates {
			self.last_single = joins;
			self.dirty = true;
		}
		match key.name.as_str() {
			"enter" => self.put("\n"),
			"tab" => self.put("  "),
			"paste" => self.put(key.text.as_deref().unwrap_or_default()),
			"backspace" if c > 0 => {
				let (i, ch) = self.text[..c].char_indices().next_back().unwrap_or((0, ' '));
				self.text.remove(i);
				self.cursor -= ch.len_utf16();
			},
			"delete" if c < self.text.len() => {
				self.text.remove(c);
			},
			"left" => {
				if let Some((_, ch)) = self.text[..c].char_indices().next_back() {
					self.cursor -= ch.len_utf16();
				}
			},
			"right" => {
				if let Some(ch) = self.text[c..].chars().next() {
					self.cursor += ch.len_utf16();
				}
			},
			"home" => {
				let start = self.text[..c].rfind('\n').map_or(0, |i| i + 1);
				self.cursor -= u16_len(&self.text[start..c]);
			},
			"end" => {
				let end = self.text[c..].find('\n').map_or(self.text.len(), |i| c + i);
				self.cursor += u16_len(&self.text[c..end]);
			},
			"up" | "down" => self.vertical(key.name == "down"),
			_ => match key.typed() {
				Some(s) if !key.ctrl && !key.meta => self.put(s),
				_ => return false,
			},
		}
		true
	}

	/// Inserts text at the caret.
	fn put(&mut self, s: &str) {
		let c = self.cursor_byte();
		self.text.insert_str(c, s);
		self.cursor += u16_len(s);
	}

	/// Moves the caret one line up or down, keeping its column.
	fn vertical(&mut self, down: bool) {
		let c = self.cursor_byte();
		let line_start = self.text[..c].rfind('\n').map_or(0, |i| i + 1);
		let col = self.text[line_start..c].chars().count();
		let target = if down {
			let Some(nl) = self.text[c..].find('\n') else { return };
			c + nl + 1
		} else {
			if line_start == 0 {
				return;
			}
			self.text[..line_start - 1].rfind('\n').map_or(0, |i| i + 1)
		};
		let line_end = self.text[target..].find('\n').map_or(self.text.len(), |i| target + i);
		let len = self.text[target..line_end].chars().count();
		let at: usize = self.text[target..].chars().take(col.min(len)).map(char::len_utf8).sum();
		self.cursor = u16_len(&self.text[..target + at]);
	}
	/// Undoes the last edit step.
	fn undo(&mut self) {
		if let Some((text, cursor)) = self.undo.pop() {
			self.text = text;
			self.cursor = cursor;
			self.last_single = false;
			self.dirty = true;
		}
	}
}

/// What the results area shows.
#[derive(Default)]
struct Output {
	sets:     Vec<Rows>,
	messages: Vec<String>,
	error:    bool,
	/// Active tab: an index into `sets`, or `sets.len()` for Messages.
	tab:      usize,
}

/// A job waiting on or running in the worker.
struct Pending {
	since:  Instant,
	what:   &'static str,
	cancel: Cancel,
	asked:  bool,
}

/// The completion popup.
struct Popup {
	comp: Completion,
	sel:  usize,
}

/// The client's state.
struct App {
	worker:    Worker,
	/// Jobs in the worker, front first.
	queue:     VecDeque<Pending>,
	profile:   String,
	server:    String,
	databases: Vec<String>,
	current:   String,
	catalog:   Option<Catalog>,
	buffers:   Vec<Buffer>,
	active:    usize,
	next_id:   usize,
	out:       Output,
	popup:     Option<Popup>,
	/// Tree rows that are open, by id.
	expanded:  HashSet<String>,
	note:      String,
	write:     bool,
	in_editor: bool,
	/// Native `edit` events seen; while zero, plain keys edit the buffer.
	native:    usize,
}

/// `[schema].[name]`.
fn quoted(schema: &str, name: &str) -> String {
	format!("[{}].[{}]", schema.replace(']', "]]"), name.replace(']', "]]"))
}

/// A styled span.
fn span(t: impl Into<String>, s: &str) -> Span {
	Span { t: t.into(), s: Some(s.to_owned()), fx: None, href: None }
}

/// A table cell with newlines collapsed so rows stay one line tall.
fn flat(cell: &str) -> String {
	cell.replace(['\r', '\n'], " ")
}

impl App {
	/// Loads the server's database list, starts the catalog load and hands the connection to the worker.
	fn new<B: Backend + Send + 'static>(mut db: B, profile: String, server: String, files: &[PathBuf]) -> anyhow::Result<Self> {
		let databases = db.databases()?;
		let current = db.current()?;
		let mut buffers: Vec<Buffer> = Vec::new();
		for (i, f) in files.iter().enumerate() {
			let mut b = Buffer::new(
				f.file_name().map_or_else(|| format!("SQLQuery_{}.sql", i + 1), |n| n.to_string_lossy().into_owned()),
				std::fs::read_to_string(f)?,
			);
			b.path = Some(f.clone());
			buffers.push(b);
		}
		if buffers.is_empty() {
			buffers.push(Buffer::new("SQLQuery_1.sql".into(), String::new()));
		}
		let mut app = Self {
			worker: Worker::spawn(db),
			queue: VecDeque::new(),
			profile,
			server,
			databases,
			current: current.clone(),
			catalog: None,
			next_id: buffers.len() + 1,
			buffers,
			active: 0,
			out: Output::default(),
			popup: None,
			expanded: ["srv".to_owned(), format!("db:{current}"), "g:tables".to_owned()].into(),
			note: String::new(),
			write: false,
			in_editor: true,
			native: 0,
		};
		app.start(Job::Catalog, "loading catalog");
		Ok(app)
	}

	fn buf(&mut self) -> &mut Buffer {
		&mut self.buffers[self.active]
	}

	/// Queues a job in the worker.
	fn start(&mut self, job: Job, what: &'static str) {
		let cancel = self.worker.submit(job);
		self.queue.push_back(Pending { since: Instant::now(), what, cancel, asked: false });
		self.note = if self.queue.len() > 1 { format!("{what} queued") } else { format!("{what}... (esc cancels)") };
	}

	/// Asks the running job to stop.
	fn cancel(&mut self) {
		if let Some(p) = self.queue.front_mut().filter(|p| !p.asked) {
			p.cancel.cancel();
			p.asked = true;
			self.note = "cancelling...".into();
		}
	}

	/// Opens a new buffer and makes it active.
	fn new_buffer(&mut self, text: String) {
		let name = format!("SQLQuery_{}.sql", self.next_id);
		self.next_id += 1;
		self.buffers.push(Buffer::new(name, text));
		self.active = self.buffers.len() - 1;
		self.popup = None;
	}

	/// Closes the active buffer, keeping at least one.
	fn close_buffer(&mut self) {
		if self.buffers.len() > 1 {
			self.buffers.remove(self.active);
			self.active = self.active.min(self.buffers.len() - 1);
		} else {
			self.buffers[0] = Buffer::new("SQLQuery_1.sql".into(), String::new());
		}
		self.popup = None;
	}

	/// Writes the active buffer to its file (in the pane's directory when it has none).
	fn save(&mut self) {
		let b = self.buf();
		let path = b.path.clone().unwrap_or_else(|| PathBuf::from(&b.name));
		match std::fs::write(&path, &b.text) {
			Ok(()) => {
				b.dirty = false;
				let full = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
				b.path = Some(path);
				self.note = format!("saved {}", full.display());
			},
			Err(e) => self.note = format!("save failed: {e}"),
		}
	}

	/// Refuses batches that look like writes in read-only mode.
	fn allowed(&mut self, batches: &[String]) -> bool {
		if self.write {
			return true;
		}
		for b in batches {
			if let Some(word) = guard::check(b) {
				self.out = Output {
					messages: vec![format!("Read-only mode refused `{word}`. Press F8 to allow writes.")],
					error: true,
					..Output::default()
				};
				return false;
			}
		}
		true
	}

	/// Runs the whole buffer, batch by batch at `GO`.
	fn run_all(&mut self) {
		let batches = tsql::batches(&self.buffers[self.active].text);
		self.run(batches);
	}

	/// Runs the statement at the caret.
	fn run_statement(&mut self) {
		let b = &self.buffers[self.active];
		let s = tsql::statement_at(&b.text, b.cursor_byte()).to_owned();
		self.run(vec![s]);
	}

	fn run(&mut self, batches: Vec<String>) {
		self.popup = None;
		if batches.is_empty() || !self.allowed(&batches) {
			return;
		}
		self.start(Job::Run { batches, limit: LIMIT }, "running");
	}

	/// Writes the statement at the caret to a CSV with no row cap.
	fn export(&mut self) {
		let b = &self.buffers[self.active];
		let sql = tsql::statement_at(&b.text, b.cursor_byte()).to_owned();
		if sql.is_empty() || !self.allowed(std::slice::from_ref(&sql)) {
			return;
		}
		let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
		let db: String = self.current.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
		self.start(Job::Export { sql, path: PathBuf::from(format!("tern-mssql-{db}-{stamp}.csv")) }, "exporting");
	}

	/// Takes the worker's reply, if a job finished. Returns whether anything changed.
	fn poll(&mut self) -> bool {
		let Some(reply) = self.worker.poll() else {
			if let Some(p) = self.queue.front().filter(|p| !p.asked) {
				self.note = format!("{} {}s (esc cancels)", p.what, p.since.elapsed().as_secs());
				return true;
			}
			return false;
		};
		let since = self.queue.pop_front().map(|p| p.since);
		let took = since.map_or(0, |s| s.elapsed().as_millis());
		self.note.clear();
		match reply {
			Reply::Ran(RunResult { sets, messages, ms, error }) => {
				self.note = format!("{ms} ms");
				let tab = if error.is_some() || sets.is_empty() { sets.len() } else { 0 };
				self.out = Output { sets, messages, error: error.is_some(), tab };
			},
			Reply::Exported(Ok((path, n))) => {
				let full = std::fs::canonicalize(&path).unwrap_or(path);
				self.note = format!("{n} rows in {took} ms to {}", full.display());
			},
			Reply::Exported(Err(e)) => self.show_error(e),
			Reply::Switched(name, Ok(cat)) => {
				self.expanded.insert(format!("db:{name}"));
				self.current = name;
				self.catalog = Some(cat);
				self.popup = None;
			},
			Reply::Switched(_, Err(e)) | Reply::Catalog(Err(e)) => self.show_error(e),
			Reply::Catalog(Ok(cat)) => self.catalog = Some(cat),
			Reply::Cancelled => self.note = "cancelled".into(),
		}
		if let Some(p) = self.queue.front() {
			self.note = format!("{} (esc cancels)", p.what);
		}
		true
	}

	fn show_error(&mut self, e: String) {
		self.out = Output { messages: vec![e], error: true, ..Output::default() };
	}

	/// A tree row was activated.
	fn activate(&mut self, id: &str) {
		if let Some(name) = id.strip_prefix("db:") {
			if name != self.current {
				self.catalog = None;
				self.start(Job::Switch(name.to_owned()), "switching");
			}
		} else if let Some(full) = id.strip_prefix("o:") {
			let Some((schema, name)) = full.split_once('.') else { return };
			let Some(kind) = self.catalog.as_ref().and_then(|c| c.find(Some(schema), name)).map(|o| o.kind) else { return };
			let q = quoted(schema, name);
			let sql = match kind {
				ObjKind::Table | ObjKind::View => format!("SELECT TOP (1000) *\nFROM {q}"),
				ObjKind::Proc => format!("EXEC {q}"),
				ObjKind::Func => format!("SELECT * FROM {q}()"),
			};
			self.new_buffer(sql.clone());
			if matches!(kind, ObjKind::Table | ObjKind::View) {
				self.run(vec![sql]);
			}
		}
	}

	/// Recomputes the completion popup after an edit or on request.
	fn complete(&mut self, force: bool) {
		let b = &self.buffers[self.active];
		self.popup = complete::complete(&b.text, b.cursor_byte(), self.catalog.as_ref(), &self.databases, force)
			.map(|comp| Popup { comp, sel: 0 });
	}

	/// Inserts the selected suggestion.
	fn accept(&mut self, index: Option<usize>) {
		let Some(p) = self.popup.take() else { return };
		let Some(item) = p.comp.items.get(index.unwrap_or(p.sel)) else { return };
		let b = self.buf();
		let end = b.cursor_byte();
		b.undo.push((b.text.clone(), b.cursor));
		b.text.replace_range(p.comp.from..end, &item.insert);
		b.cursor = u16_len(&b.text[..p.comp.from + item.insert.len()]);
		b.last_single = false;
		b.dirty = true;
	}

	/// The explorer, styled like the file explorer: folders that open and close, an icon per kind, the active database marked.
	fn explorer(&self) -> ui::Tree<Msg> {
		let open = |id: &str| self.expanded.contains(id);
		let mut srv = TreeItem::new("srv", vec![span(self.profile.clone(), "strong"), span(format!("  {}", self.server), "dim")])
			.open(open("srv"));
		srv.icon = Some("server".into());
		for d in &self.databases {
			let id = format!("db:{d}");
			let active = *d == self.current;
			let label = if active { vec![span("● ", "accent"), span(d.clone(), "strong")] } else { vec![span(d.clone(), "")] };
			let mut item = TreeItem::new(id.clone(), label).open(active && open(&id));
			item.icon = Some("database".into());
			if active {
				item.children = self.catalog_items();
			}
			srv = srv.child(item);
		}
		ui::tree().key("tree").basis(0.24).grow(0.0).on_activate(|id| Msg::Activate(id.to_owned())).on_toggle(|_| Msg::Toggle).item(srv)
	}

	/// The active database's folders and objects.
	fn catalog_items(&self) -> Vec<TreeItem> {
		let Some(cat) = &self.catalog else {
			let mut loading = TreeItem::new("loading", vec![span("loading objects...", "dim")]);
			loading.icon = Some("timer".into());
			return vec![loading];
		};
		let folder = |id: &str, title: &str, count: usize| {
			let mut f = TreeItem::new(id, vec![span(title.to_owned(), ""), span(format!("  {count}"), "dim")]).open(self.expanded.contains(id));
			f.icon = Some(if self.expanded.contains(id) { "folder-open" } else { "folder" }.into());
			f
		};
		let objects = |kind: ObjKind, icon: &'static str| -> Vec<TreeItem> {
			cat.of(kind)
				.map(|o| {
					let id = format!("o:{}", o.full());
					let mut label = vec![span(o.name.clone(), "")];
					if !o.schema.eq_ignore_ascii_case("dbo") {
						label.push(span(format!("  {}", o.schema), "dim"));
					}
					let mut item = TreeItem::new(id.clone(), label).open(self.expanded.contains(&id));
					item.icon = Some(icon.into());
					if !o.columns.is_empty() {
						if self.expanded.contains(&id) {
							for c in &o.columns {
								let mut col = TreeItem::new(
									format!("c:{}.{}", o.full(), c.name),
									vec![
										span(c.name.clone(), if c.key { "strong" } else { "" }),
										span(format!("  {}{}", c.ty, if c.nullable { "" } else { " not null" }), "dim"),
									],
								);
								col.icon = Some(if c.key { "key" } else if c.identity { "hash" } else { "tag" }.into());
								item.children.push(col);
							}
						} else {
							item.children.push(TreeItem::new(format!("p:{id}"), vec![span("...", "dim")]));
						}
					}
					item
				})
				.collect()
		};
		let mut tables = folder("g:tables", "Tables", cat.of(ObjKind::Table).count());
		tables.children = objects(ObjKind::Table, "grid");
		let mut views = folder("g:views", "Views", cat.of(ObjKind::View).count());
		views.children = objects(ObjKind::View, "eye");
		let mut procs = folder("g:procs", "Stored Procedures", cat.of(ObjKind::Proc).count());
		procs.children = objects(ObjKind::Proc, "code");
		let mut funcs = folder("g:funcs", "Functions", cat.of(ObjKind::Func).count());
		funcs.children = objects(ObjKind::Func, "braces");
		let mut prog = folder("g:prog", "Programmability", cat.of(ObjKind::Proc).count() + cat.of(ObjKind::Func).count());
		prog.children = vec![procs, funcs];
		vec![tables, views, prog]
	}

	/// The results area: a tab per result set, then Messages.
	fn results(&self) -> ui::Col<Msg> {
		let o = &self.out;
		let mut tabs = ui::tabs().key("rtabs").min(ui::Bound::h(ui::Extent::Lines(2.0))).on_select(|id| Msg::ResultTab(id.to_owned()));
		for i in 0..o.sets.len() {
			tabs = tabs.tab(format!("r{i}"), format!("Results {}", i + 1));
		}
		tabs = tabs.tab("msg", if o.error { vec![span("Messages", "error")] } else { vec![span("Messages", "")] });
		tabs = tabs.active(if o.tab >= o.sets.len() { "msg".to_owned() } else { format!("r{}", o.tab) });
		let body: ui::Col<Msg> = if o.tab >= o.sets.len() {
			let mut col = ui::col().key("body");
			if o.messages.is_empty() {
				col = col.child(ui::md("F5 runs the script, ctrl+enter the statement at the caret. Results and messages show here.").key("hint"));
			}
			for (i, m) in o.messages.iter().enumerate() {
				let bad = o.error && i + 1 == o.messages.len();
				col = col.child(ui::text(vec![span(m.clone(), if bad { "error" } else { "dim" })]).key(i));
			}
			col
		} else {
			let r = &o.sets[o.tab];
			let mut table = ui::table().key("grid");
			for (i, name) in r.columns.iter().enumerate() {
				table = table.col(Column::new(format!("c{i}"), name.clone()));
			}
			for (n, row) in r.rows.iter().enumerate() {
				let mut tr = TableRow::new(n.to_string());
				for (i, c) in row.iter().enumerate() {
					tr = tr.cell(format!("c{i}"), c.as_deref().map_or_else(|| "NULL".to_owned(), flat));
				}
				table = table.row(tr);
			}
			ui::col().key("body").child(table)
		};
		ui::col().key("res").max(ui::Bound::h(ui::Extent::Lines(16.0))).child(tabs).child(body)
	}

	/// The query tabs and the active buffer's editor.
	fn work(&self) -> ui::Col<Msg> {
		let b = &self.buffers[self.active];
		let mut tabs = ui::tabs().key("tabs").min(ui::Bound::h(ui::Extent::Lines(2.0))).on_select(|id| Msg::Tab(id.to_owned()));
		for (i, buf) in self.buffers.iter().enumerate() {
			let label = if buf.dirty { vec![span(buf.name.clone(), ""), span(" ●", "accent")] } else { vec![span(buf.name.clone(), "")] };
			tabs = tabs.tab(i.to_string(), label);
		}
		tabs = tabs.active(self.active.to_string());
		let editor = ui::editor()
			.key("ed")
			.grow(1.0)
			.min(ui::Bound::h(ui::Extent::Lines(10.0)))
			.text(b.text.clone())
			.cursor(b.cursor)
			.lang("sql")
			.decor(tsql::decorate(&b.text))
			.placeholder("-- T-SQL. F5 runs, ctrl+enter runs the statement, ctrl+space completes")
			.on_edit(|e| Msg::Edit(e.clone()))
			.on_undo(Msg::Undo)
			.on_focus(Msg::Focus);
		ui::col().key("work").grow(1.0).child(tabs).child(editor).child(self.results())
	}


	/// The completion list, shown in the dock above the status strip.
	fn popup_view(&self) -> Option<ui::List<Msg>> {
		let p = self.popup.as_ref()?;
		let mut list = ui::list().key("comp").max_lines(9).selected(p.sel.to_string()).on_activate(|id| Msg::Pick(id.to_owned()));
		for (i, it) in p.comp.items.iter().enumerate() {
			let icon = match it.kind {
				ItemKind::Column => "tag",
				ItemKind::Table => "grid",
				ItemKind::View => "eye",
				ItemKind::Proc => "code",
				ItemKind::Func => "braces",
				ItemKind::Schema => "folder",
				ItemKind::Database => "database",
				ItemKind::Keyword => "key-round",
			};
			list = list.child(ui::item(it.label.clone()).key(i).detail(it.detail.clone()).icon(icon));
		}
		Some(list)
	}

	/// The whole view.
	fn view(&self) -> View<Msg> {
		let b = &self.buffers[self.active];
		let (line, col) = b.line_col();
		let body = ui::row().key("body").grow(1.0).align(ui::Align::Stretch).child(self.explorer()).child(self.work());
		let catalog = self.catalog.as_ref().map_or_else(|| "loading catalog...".to_owned(), |c| format!("{} objects", c.objects.len()));
		let status = ui::status()
			.key("bar")
			.child(ui::seg(format!("{} / {}", self.profile, self.current)).icon("database"))
			.child(ui::seg(format!("Ln {line}, Col {col}")))
			.child(ui::seg(catalog).tone(Tone::Muted))
			.child(ui::seg(if self.native > 0 { "native edit" } else { "key edit" }).tone(Tone::Muted))
			.child(ui::seg(self.note.clone()).tone(Tone::Muted))
			.child(
				ui::seg(if self.write { "WRITE" } else { "read-only" })
					.side(Side::Right)
					.tone(if self.write { Tone::Warning } else { Tone::Success }),
			);
		let main = ui::col().min(ui::Bound::h(ui::Extent::Fraction(1.0))).align(ui::Align::Stretch).child(body);
		match self.popup_view() {
			Some(list) => View::new().main(main).dock(nodes![list, status]),
			None => View::new().main(main).dock(nodes![status]),
		}
	}
}

/// Runs the client until the user quits.
pub fn run<B: Backend + Send + 'static>(db: B, profile: String, server: String, files: &[PathBuf]) -> anyhow::Result<()> {
	let Some(mut session) = Session::connect(Options::new().feature(feature::SEND).feature(feature::EDIT).feature(feature::UNDO))? else {
		anyhow::bail!("interactive mode needs a terminal that speaks TSP (Tern); use `tern-mssql query` instead");
	};
	let mut app = App::new(db, profile, server, files)?;
	let sf = session.open(SurfaceOptions::screen().title("SQL Server").role("sqlq"))?;
	draw(&mut session, sf, &app)?;
	session.focus(sf, Some(EDITOR))?;
	loop {
		// Poll while jobs run so the clock ticks and replies show up; block when idle.
		let wait = (!app.queue.is_empty()).then_some(Duration::from_millis(100));
		let input = match session.next(wait) {
			Ok(input) => input,
			Err(tern_sdk::Error::InputClosed) => break,
			Err(e) => return Err(e.into()),
		};
		let changed = app.poll();
		let Some(input) = input else {
			if changed {
				draw(&mut session, sf, &app)?;
			}
			continue;
		};
		match input {
			Input::Key(key) => {
				let popup = app.popup.is_some();
				if key.is("ctrl+q") || key.is("ctrl+c") {
					break;
				} else if popup && key.is("escape") {
					app.popup = None;
				} else if popup && (key.name == "down" || key.name == "up") {
					if let Some(p) = app.popup.as_mut() {
						let n = p.comp.items.len();
						p.sel = if key.name == "down" { (p.sel + 1) % n } else { (p.sel + n - 1) % n };
					}
				} else if popup && (key.name == "tab" || key.name == "enter") && !key.ctrl {
					app.accept(None);
				} else if key.is("f5") {
					app.run_all();
				} else if key.name == "enter" && key.ctrl {
					app.run_statement();
				} else if key.is("ctrl+space") {
					app.complete(true);
				} else if key.is("ctrl+e") {
					app.export();
				} else if key.is("escape") || key.is("ctrl+x") {
					app.cancel();
				} else if key.is("f8") {
					app.write = !app.write;
				} else if key.is("ctrl+n") {
					app.new_buffer(String::new());
				} else if key.is("ctrl+w") {
					app.close_buffer();
				} else if key.is("ctrl+s") {
					app.save();
				} else if key.is("f6") {
					app.in_editor = !app.in_editor;
					session.focus(sf, Some(if app.in_editor { EDITOR } else { TREE }))?;
				} else if app.native == 0 && app.buf().key(&key) {
					app.complete(false);
				}
			},
			Input::Msg(Msg::Edit(e), _) => {
				app.native += 1;
				let ok = app.buf().apply(&e);
				if ok {
					app.complete(false);
				}
			},
			Input::Msg(Msg::Undo, _) => {
				app.buf().undo();
				app.popup = None;
			},
			Input::Msg(Msg::Focus, _) => {
				app.in_editor = true;
				session.focus(sf, Some(EDITOR))?;
			},
			Input::Msg(Msg::Activate(id), _) => app.activate(&id),
			Input::Msg(Msg::Toggle, Event::Toggle(t)) => {
				if let Some(id) = t.key.as_ref().and_then(|k| k.as_str()) {
					if t.collapsed {
						app.expanded.remove(id);
					} else {
						app.expanded.insert(id.to_owned());
					}
				}
			},
			Input::Msg(Msg::Pick(id), _) => {
				if let Ok(i) = id.parse::<usize>() {
					app.accept(Some(i));
				}
				session.focus(sf, Some(EDITOR))?;
			},
			Input::Msg(Msg::Tab(id), _) => {
				if let Ok(i) = id.parse::<usize>() {
					if i < app.buffers.len() {
						app.active = i;
						app.popup = None;
					}
				}
			},
			Input::Msg(Msg::ResultTab(id), _) => {
				app.out.tab = if id == "msg" { app.out.sets.len() } else { id.trim_start_matches('r').parse().unwrap_or(0) };
			},
			_ => continue,
		}
		draw(&mut session, sf, &app)?;
	}
	session.close()?;
	Ok(())
}

/// Renders the view.
fn draw(session: &mut Session<Msg>, sf: tern_sdk::Surface, app: &App) -> anyhow::Result<()> {
	session.render(sf, app.view())?;
	Ok(())
}

/// What the profile picker reports.
#[derive(Clone, Debug)]
enum Pick {
	/// A row was activated: the profile's name.
	Chosen(String),
}

/// The picker's list.
fn picker_view(profiles: &[(String, String)], at: usize) -> View<Pick> {
	let mut list = ui::list::<Pick>().key("profiles").selected(profiles[at].0.clone()).on_activate(|id| Pick::Chosen(id.to_owned()));
	for (name, detail) in profiles {
		list = list.child(ui::item(name.clone()).key(name.clone()).detail(detail.clone()).icon("database"));
	}
	View::new().main(ui::col().child(ui::md("**Connect to**").key("h")).child(list))
}

/// Shows the profiles as a list and returns the one chosen, or `None` on quit.
/// Up, down and enter work from the keyboard as well as the list's own events.
pub fn pick(profiles: &[(String, String)]) -> anyhow::Result<Option<String>> {
	if profiles.is_empty() {
		anyhow::bail!("no profiles in {}", crate::profile::path().display());
	}
	let Some(mut session) = Session::<Pick>::connect(Options::new())? else {
		anyhow::bail!("pick a profile by name outside Tern: tern-mssql PROFILE");
	};
	let sf = session.open(SurfaceOptions::screen().title("SQL Server"))?;
	let mut at = 0;
	session.render(sf, picker_view(profiles, at))?;
	session.focus(sf, Some("main.profiles"))?;
	let chosen = loop {
		match session.next(None) {
			Ok(Some(Input::Msg(Pick::Chosen(name), _))) => break Some(name),
			Ok(Some(Input::Key(k))) => {
				if k.is("escape") || k.is("ctrl+q") || k.is("ctrl+c") {
					break None;
				}
				match k.name.as_str() {
					"enter" => break Some(profiles[at].0.clone()),
					"down" => at = (at + 1).min(profiles.len() - 1),
					"up" => at = at.saturating_sub(1),
					_ => continue,
				}
				session.render(sf, picker_view(profiles, at))?;
			},
			Ok(_) => {},
			Err(tern_sdk::Error::InputClosed) => break None,
			Err(e) => return Err(e.into()),
		}
	};
	session.close()?;
	Ok(chosen)
}
