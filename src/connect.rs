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

/// Form fields, in order.
const LABELS: [&str; 7] = ["Name", "Server", "Port", "User", "Password", "Database", "Trust certificate"];
const NAME: usize = 0;
const PORT: usize = 2;
const PASSWORD: usize = 4;
const TRUST: usize = 6;

/// A connection being added or edited.
struct Form {
	/// Field values (Trust is `yes` or `no`).
	values:   [String; 7],
	at:       usize,
	/// The saved name being edited, `None` for a new connection.
	original: Option<String>,
}

impl Form {
	fn new(p: Option<&Profile>) -> Self {
		let Some(p) = p else {
			let mut values: [String; 7] = Default::default();
			values[PORT] = "1433".into();
			values[TRUST] = "no".into();
			return Self { values, at: 0, original: None };
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
			at:       0,
			original: Some(p.name.clone()),
		}
	}

	/// The profile these values describe.
	fn profile(&self) -> Result<Profile, String> {
		let v = &self.values;
		let name = v[NAME].trim();
		if name.is_empty() || name.contains(['\\', '/', '\n']) {
			return Err("Name is required".into());
		}
		if v[1].trim().is_empty() {
			return Err("Server is required".into());
		}
		let port = v[PORT].trim().parse::<u16>().map_err(|_| "Port must be a number".to_owned())?;
		let pw = v[PASSWORD].trim();
		Ok(Profile {
			name: name.to_owned(),
			server: v[1].trim().to_owned(),
			port,
			user: v[3].trim().to_owned(),
			password_env: pw.strip_prefix('$').map(str::to_owned),
			database: Some(v[5].trim().to_owned()).filter(|d| !d.is_empty()),
			trust_cert: v[TRUST] == "yes",
		})
	}

	/// The password typed, if it is a literal one.
	fn literal(&self) -> Option<&str> {
		let pw = self.values[PASSWORD].as_str();
		(!pw.is_empty() && !pw.starts_with('$')).then_some(pw)
	}

	/// Applies a key to the active field.
	fn key(&mut self, k: &tern_sdk::keys::Key) {
		match k.name.as_str() {
			"down" | "tab" => self.at = (self.at + 1).min(LABELS.len() - 1),
			"up" => self.at = self.at.saturating_sub(1),
			"backspace" if self.at != TRUST => {
				self.values[self.at].pop();
			},
			"paste" if self.at != TRUST => {
				let text = k.text.as_deref().unwrap_or_default().replace(['\r', '\n'], "");
				self.values[self.at].push_str(&text);
			},
			_ => {
				let Some(s) = k.typed().filter(|_| !k.ctrl && !k.meta) else { return };
				if self.at == TRUST {
					if s == " " || s == "y" || s == "n" {
						self.values[TRUST] = if s == "n" || self.values[TRUST] == "yes" { "no" } else { "yes" }.into();
					}
				} else if !s.contains(['\r', '\n']) {
					self.values[self.at].push_str(s);
				}
			},
		}
	}
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
			for (i, label) in LABELS.iter().enumerate() {
				let mut v = if i == PASSWORD && f.literal().is_some() { "*".repeat(f.values[i].chars().count()) } else { f.values[i].clone() };
				if i == f.at {
					v.push('|');
				}
				list = list.child(ui::item(*label).key(format!("f{i}")).detail(v).icon(if i == PASSWORD { "key" } else { "tag" }));
			}
			help = "type to edit, tab/down next, up back, enter save, ctrl+t test, esc cancel. Password: type it (stored in the OS credential store) or $ENVVAR".to_owned();
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
	let p = f.profile()?;
	if s.profiles.iter().any(|o| o.name == p.name && f.original.as_deref() != Some(&o.name)) {
		return Err(format!("A connection named {} already exists", p.name));
	}
	if let Some(pw) = f.literal() {
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
pub fn choose() -> anyhow::Result<Option<(Mssql, String, String)>> {
	let Some(mut session) = Session::<Pick>::connect(Options::new())? else {
		anyhow::bail!("pick a profile by name outside Tern: tern-mssql PROFILE");
	};
	let sf = session.open(SurfaceOptions::screen().title("SQL Server"))?;
	let profiles = profile::load()?;
	let mode = if profiles.is_empty() { Mode::Form(Form::new(None)) } else { Mode::List };
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
						s.note = match f.profile().and_then(|p| test(&p, f.literal())) {
							Ok(_) => "Connection works".into(),
							Err(e) => e,
						};
						s.mode = Mode::Form(f);
					} else {
						s.note.clear();
						f.key(&k);
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
