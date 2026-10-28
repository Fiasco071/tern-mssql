//! The connection manager shown when no profile is named: list, add, edit,
//! delete and test saved connections, then connect.

use tern_sdk::{
	Input, Options, Session, SurfaceOptions,
	ui::{self, View},
};

use crate::{
	backend::mssql::Mssql,
	profile::{self, Profile},
};

/// What the list reports.
#[derive(Clone, Debug)]
enum Pick {
	/// A row was activated: its key.
	Chosen(String),
}

/// Form fields, in order (the connection string is held apart).
const LABELS: [&str; 7] = ["Name", "Server", "Port", "User", "Password", "Database", "Trust certificate"];
const NAME: usize = 0;
const PORT: usize = 2;
const PASSWORD: usize = 4;
const TRUST: usize = 6;

/// A line of the form.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
	Mode,
	Name,
	Server,
	Port,
	User,
	Password,
	Database,
	Trust,
	ConnString,
}

const FIELD_ROWS: [Row; 8] = [Row::Mode, Row::Name, Row::Server, Row::Port, Row::User, Row::Password, Row::Database, Row::Trust];
const STRING_ROWS: [Row; 4] = [Row::Mode, Row::Name, Row::ConnString, Row::Password];

/// A connection being added or edited.
struct Form {
	/// Field values (Trust is `yes` or `no`).
	values:   [String; 7],
	/// The connection string, when that is how it is entered.
	cs:       String,
	/// Entering a connection string instead of the separate fields.
	string:   bool,
	/// Index into the current rows.
	at:       usize,
	/// The saved name being edited, `None` for a new connection.
	original: Option<String>,
}

/// A form's result: the profile and the password that goes to the credential store.
struct Built {
	profile:  Profile,
	password: Option<String>,
}

impl Form {
	fn new(p: Option<&Profile>) -> Self {
		let Some(p) = p else {
			let mut values: [String; 7] = Default::default();
			values[PORT] = "1433".into();
			values[TRUST] = "no".into();
			return Self { values, cs: String::new(), string: false, at: 1, original: None };
		};
		let password = p.password_env.as_ref().map(|v| format!("${v}")).unwrap_or_default();
		Self {
			values:   [
				p.name.clone(),
				p.server.clone(),
				p.port.to_string(),
				p.user.clone(),
				password,
				p.database.clone().unwrap_or_default(),
				if p.trust_cert { "yes" } else { "no" }.into(),
			],
			cs:       p.connection_string.clone().unwrap_or_default(),
			string:   p.connection_string.is_some(),
			at:       1,
			original: Some(p.name.clone()),
		}
	}

	fn rows(&self) -> &'static [Row] {
		if self.string { &STRING_ROWS } else { &FIELD_ROWS }
	}

	fn row(&self) -> Row {
		let rows = self.rows();
		rows[self.at.min(rows.len() - 1)]
	}

	/// The text a row edits, if it edits text.
	fn slot(&mut self, r: Row) -> Option<&mut String> {
		match r {
			Row::Name => Some(&mut self.values[NAME]),
			Row::Server => Some(&mut self.values[1]),
			Row::Port => Some(&mut self.values[PORT]),
			Row::User => Some(&mut self.values[3]),
			Row::Password => Some(&mut self.values[PASSWORD]),
			Row::Database => Some(&mut self.values[5]),
			Row::ConnString => Some(&mut self.cs),
			Row::Mode | Row::Trust => None,
		}
	}

	/// Switches between the fields and the connection string, carrying what it can across.
	fn switch(&mut self) -> Result<(), String> {
		if self.string {
			let sum = crate::connstr::summarize(&self.cs)?;
			let (_, pw) = crate::connstr::strip_password(&self.cs);
			self.values[1] = sum.server;
			self.values[PORT] = sum.port.to_string();
			self.values[3] = sum.user;
			self.values[5] = sum.database.unwrap_or_default();
			self.values[TRUST] = if sum.trust { "yes" } else { "no" }.into();
			if let Some(pw) = pw.filter(|_| self.values[PASSWORD].is_empty()) {
				self.values[PASSWORD] = pw;
			}
			self.string = false;
		} else {
			let v = &self.values;
			let mut cs = format!("Server=tcp:{},{};", v[1].trim(), v[PORT].trim());
			if !v[3].trim().is_empty() {
				cs.push_str(&format!("User Id={};", v[3].trim()));
			}
			if !v[5].trim().is_empty() {
				cs.push_str(&format!("Database={};", v[5].trim()));
			}
			cs.push_str("Encrypt=true;");
			if v[TRUST] == "yes" {
				cs.push_str("TrustServerCertificate=true;");
			}
			self.cs = cs;
			self.string = true;
		}
		self.at = 0;
		Ok(())
	}

	/// The profile these values describe, and the password to store.
	fn built(&self) -> Result<Built, String> {
		let v = &self.values;
		let name = v[NAME].trim();
		if name.is_empty() || name.contains(['\\', '/', '\n']) {
			return Err("Name is required".into());
		}
		let pw = v[PASSWORD].trim();
		let password_env = pw.strip_prefix('$').map(str::to_owned);
		let typed = self.literal().map(str::to_owned);
		if self.string {
			let sum = crate::connstr::summarize(&self.cs)?;
			let (stripped, in_string) = crate::connstr::strip_password(&self.cs);
			return Ok(Built {
				profile:  Profile {
					name: name.to_owned(),
					server: sum.server,
					port: sum.port,
					user: sum.user,
					password_env,
					database: sum.database,
					trust_cert: sum.trust,
					connection_string: Some(stripped),
				},
				password: typed.or(in_string),
			});
		}
		if v[1].trim().is_empty() {
			return Err("Server is required".into());
		}
		let port = v[PORT].trim().parse::<u16>().map_err(|_| "Port must be a number".to_owned())?;
		Ok(Built {
			profile:  Profile {
				name: name.to_owned(),
				server: v[1].trim().to_owned(),
				port,
				user: v[3].trim().to_owned(),
				password_env,
				database: Some(v[5].trim().to_owned()).filter(|d| !d.is_empty()),
				trust_cert: v[TRUST] == "yes",
				connection_string: None,
			},
			password: typed,
		})
	}

	/// The password typed, if it is a literal one.
	fn literal(&self) -> Option<&str> {
		let pw = self.values[PASSWORD].as_str();
		(!pw.is_empty() && !pw.starts_with('$')).then_some(pw)
	}

	/// Applies a key to the active row. Returns a message to show, if any.
	fn key(&mut self, k: &tern_sdk::keys::Key) -> Option<String> {
		let last = self.rows().len() - 1;
		match k.name.as_str() {
			"down" | "tab" => {
				self.at = (self.at + 1).min(last);
				return None;
			},
			"up" => {
				self.at = self.at.saturating_sub(1);
				return None;
			},
			_ => {},
		}
		let row = self.row();
		if row == Row::Mode {
			if matches!(k.name.as_str(), "left" | "right") || k.typed() == Some(" ") {
				return self.switch().err();
			}
			return None;
		}
		if row == Row::Trust {
			if let Some(s) = k.typed().filter(|s| matches!(*s, " " | "y" | "n")) {
				self.values[TRUST] = if s == "n" || self.values[TRUST] == "yes" { "no" } else { "yes" }.into();
			}
			return None;
		}
		match k.name.as_str() {
			"backspace" => {
				if let Some(v) = self.slot(row) {
					v.pop();
				}
			},
			"paste" => {
				let text = k.text.as_deref().unwrap_or_default().replace(['\r', '\n'], "");
				if let Some(v) = self.slot(row) {
					v.push_str(&text);
				}
			},
			_ => {
				if let Some(s) = k.typed().filter(|_| !k.ctrl && !k.meta).filter(|s| !s.contains(['\r', '\n'])) {
					let s = s.to_owned();
					if let Some(v) = self.slot(row) {
						v.push_str(&s);
					}
				}
			},
		}
		None
	}
}

/// A connection string with its password hidden, for showing.
fn masked(cs: &str) -> String {
	crate::connstr::parse(cs)
		.iter()
		.map(|p| if matches!(p.key.as_str(), "password" | "pwd") { "Password=******".to_owned() } else { p.raw.clone() })
		.collect::<Vec<_>>()
		.join(";")
}

/// Which screen is up.
enum Mode {
	List,
	Form(Form),
	/// Waiting for `y` to delete the selected connection.
	Delete,
}

struct State {
	profiles: Vec<Profile>,
	at:       usize,
	mode:     Mode,
	note:     String,
}

/// The saved connections or the form.
fn view(s: &State) -> View<Pick> {
	let mut list = ui::list::<Pick>().key("profiles").on_activate(|id| Pick::Chosen(id.to_owned()));
	let help;
	match &s.mode {
		Mode::Form(f) => {
			list = list.selected(format!("f{}", f.at));
			for (i, row) in f.rows().iter().enumerate() {
				let (label, mut v, icon) = match row {
					Row::Mode => ("Mode", if f.string { "Connection string" } else { "Fields" }.to_owned(), "settings"),
					Row::ConnString => ("Connection string", masked(&f.cs), "tag"),
					Row::Password => ("Password", if f.literal().is_some() { "*".repeat(f.values[PASSWORD].chars().count()) } else { f.values[PASSWORD].clone() }, "key"),
					Row::Name => (LABELS[NAME], f.values[NAME].clone(), "tag"),
					Row::Server => (LABELS[1], f.values[1].clone(), "tag"),
					Row::Port => (LABELS[PORT], f.values[PORT].clone(), "tag"),
					Row::User => (LABELS[3], f.values[3].clone(), "tag"),
					Row::Database => (LABELS[5], f.values[5].clone(), "tag"),
					Row::Trust => (LABELS[TRUST], f.values[TRUST].clone(), "tag"),
				};
				if i == f.at && !matches!(row, Row::Mode | Row::Trust) {
					v.push('|');
				}
				list = list.child(ui::item(label).key(format!("f{i}")).detail(v).icon(icon));
			}
			help = "type to edit, tab/down next, up back, enter save, ctrl+t test, esc cancel. Mode: space switches between fields and a connection string. Password: type it (kept in the OS credential store) or $ENVVAR".to_owned();
		},
		_ => {
			if let Some(p) = s.profiles.get(s.at) {
				list = list.selected(p.name.clone());
			}
			for p in &s.profiles {
				list = list.child(ui::item(p.name.clone()).key(p.name.clone()).detail(format!("{}  {}", p.server, p.user)).icon("database"));
			}
			help = if matches!(s.mode, Mode::Delete) {
				format!("delete {}? y to confirm, any other key cancels", s.profiles.get(s.at).map_or("", |p| p.name.as_str()))
			} else if s.profiles.is_empty() {
				"no connections yet: press n to add one".to_owned()
			} else {
				"enter connect, n new, e edit, d delete, esc quit".to_owned()
			};
		},
	}
	let title = match &s.mode {
		Mode::Form(f) if f.original.is_some() => "**Edit connection**",
		Mode::Form(_) => "**New connection**",
		_ => "**Connect to**",
	};
	let mut col = ui::col().child(ui::md(title).key("h")).child(list).child(ui::md(help).key("help"));
	if !s.note.is_empty() {
		col = col.child(ui::md(s.note.clone()).key("note"));
	}
	View::new().main(col)
}

/// Tries a connection and reports the outcome as display text.
fn test(p: &Profile, password: Option<&str>) -> Result<Mssql, String> {
	let ado = match password {
		Some(pw) => p.ado_with(pw),
		None => p.ado(),
	}
	.map_err(|e| format!("{e:#}"))?;
	Mssql::connect(&ado).map_err(|e| format!("{e:#}"))
}

/// Saves the form: password to the credential store, the rest to the profile file.
fn save(s: &mut State, f: &Form) -> Result<String, String> {
	let Built { profile: p, password } = f.built()?;
	if s.profiles.iter().any(|o| o.name == p.name && f.original.as_deref() != Some(&o.name)) {
		return Err(format!("A connection named {} already exists", p.name));
	}
	if let Some(pw) = password.as_deref() {
		profile::set_secret(&p.name, pw).map_err(|e| format!("{e:#}"))?;
	} else if p.password_env.is_some() {
		profile::forget_secret(&p.name);
	} else if f.original.as_deref() != Some(&p.name) {
		if let Some(old) = &f.original {
			// A rename keeps the stored password by moving it.
			if let Ok(e) = keyring::Entry::new("tern-mssql", old)
				&& let Ok(pw) = e.get_password()
			{
				profile::set_secret(&p.name, &pw).map_err(|e| format!("{e:#}"))?;
				profile::forget_secret(old);
			}
		}
	}
	match &f.original {
		Some(old) => {
			if let Some(slot) = s.profiles.iter_mut().find(|o| &o.name == old) {
				*slot = p.clone();
			}
		},
		None => s.profiles.push(p.clone()),
	}
	profile::store(&s.profiles).map_err(|e| format!("{e:#}"))?;
	s.at = s.profiles.iter().position(|o| o.name == p.name).unwrap_or(0);
	Ok(format!("Saved {}", p.name))
}

/// Shows the connections and returns a live connection with its profile name and server, or `None` on quit.
pub fn choose(new: bool) -> anyhow::Result<Option<(Mssql, String, String)>> {
	let Some(mut session) = Session::<Pick>::connect(Options::new())? else {
		anyhow::bail!("pick a profile by name outside Tern: tern-mssql PROFILE");
	};
	let sf = session.open(SurfaceOptions::screen().title("SQL Server"))?;
	let profiles = profile::load()?;
	let mode = if new || profiles.is_empty() { Mode::Form(Form::new(None)) } else { Mode::List };
	let mut s = State { profiles, at: 0, mode, note: String::new() };
	session.render(sf, view(&s))?;
	session.focus(sf, Some("main.profiles"))?;
	let chosen = loop {
		let input = match session.next(None) {
			Ok(Some(i)) => i,
			Ok(None) => continue,
			Err(tern_sdk::Error::InputClosed) => break None,
			Err(e) => return Err(e.into()),
		};
		let mut connect = None;
		match input {
			Input::Msg(Pick::Chosen(id), _) if matches!(s.mode, Mode::List) => connect = Some(id),
			Input::Key(k) => match std::mem::replace(&mut s.mode, Mode::List) {
				Mode::Form(mut f) => {
					if k.is("escape") {
						s.mode = Mode::List;
						s.note.clear();
						if s.profiles.is_empty() {
							break None;
						}
					} else if k.is("enter") {
						match save(&mut s, &f) {
							Ok(note) => {
								s.note = note;
								s.mode = Mode::List;
							},
							Err(e) => {
								s.note = e;
								s.mode = Mode::Form(f);
							},
						}
					} else if k.is("ctrl+t") {
						s.note = match f.built().and_then(|b| test(&b.profile, b.password.as_deref())) {
							Ok(_) => "Connection works".into(),
							Err(e) => e,
						};
						s.mode = Mode::Form(f);
					} else {
						s.note = f.key(&k).unwrap_or_default();
						s.mode = Mode::Form(f);
					}
				},
				Mode::Delete => {
					if k.typed() == Some("y") && s.at < s.profiles.len() {
						let gone = s.profiles.remove(s.at);
						profile::forget_secret(&gone.name);
						s.note = match profile::store(&s.profiles) {
							Ok(()) => format!("Deleted {}", gone.name),
							Err(e) => format!("{e:#}"),
						};
						s.at = s.at.min(s.profiles.len().saturating_sub(1));
					}
				},
				Mode::List => {
					s.note.clear();
					if k.is("escape") || k.is("ctrl+q") || k.is("ctrl+c") {
						break None;
					}
					match (k.name.as_str(), k.typed()) {
						("enter", _) => connect = s.profiles.get(s.at).map(|p| p.name.clone()),
						("down", _) => s.at = (s.at + 1).min(s.profiles.len().saturating_sub(1)),
						("up", _) => s.at = s.at.saturating_sub(1),
						(_, Some("n")) => s.mode = Mode::Form(Form::new(None)),
						(_, Some("e")) => {
							if let Some(p) = s.profiles.get(s.at) {
								s.mode = Mode::Form(Form::new(Some(p)));
							}
						},
						(_, Some("d")) if !s.profiles.is_empty() => s.mode = Mode::Delete,
						_ => {},
					}
				},
			},
			_ => continue,
		}
		if let Some(name) = connect {
			if let Some(i) = s.profiles.iter().position(|p| p.name == name) {
				s.at = i;
			}
			s.note = format!("Connecting to {name}...");
			session.render(sf, view(&s))?;
			let p = s.profiles[s.at].clone();
			match test(&p, None) {
				Ok(db) => break Some((db, p.name, p.server)),
				Err(e) => s.note = e,
			}
		}
		session.render(sf, view(&s))?;
		session.focus(sf, Some("main.profiles"))?;
	};
	session.close()?;
	Ok(chosen)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_pasted_string_keeps_its_password_out_of_the_profile() {
		let mut f = Form::new(None);
		f.values[NAME] = "qas".into();
		f.cs = "Server=tcp:qas-db.example.com,1450;User Id=sa;Password=secret;Database=AppDb;TrustServerCertificate=true".into();
		f.string = true;
		let b = f.built().unwrap();
		assert_eq!(b.password.as_deref(), Some("secret"));
		let p = b.profile;
		assert_eq!((p.server.as_str(), p.port, p.user.as_str(), p.trust_cert), ("qas-db.example.com", 1450, "sa", true));
		assert!(!p.connection_string.as_deref().unwrap().to_lowercase().contains("password"));
		assert_eq!(masked(&f.cs).matches("secret").count(), 0);
	}

	#[test]
	fn switching_modes_carries_the_values() {
		let mut f = Form::new(None);
		f.values[NAME] = "qas".into();
		f.values[1] = "h.example.com".into();
		f.values[3] = "sa".into();
		f.values[5] = "AppDb".into();
		f.values[TRUST] = "yes".into();
		f.switch().unwrap();
		assert!(f.string && f.cs.contains("Server=tcp:h.example.com,1433") && f.cs.contains("Database=AppDb") && f.cs.contains("TrustServerCertificate=true"));
		f.cs = "Server=other,1500;User Id=bob".into();
		f.switch().unwrap();
		assert_eq!((f.values[1].as_str(), f.values[PORT].as_str(), f.values[3].as_str(), f.string), ("other", "1500", "bob", false));
		assert!(Form { cs: "Database=x".into(), string: true, ..Form::new(None) }.switch().is_err());
	}
}
