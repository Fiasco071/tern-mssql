//! ADO.NET style connection strings (`Server=tcp:host,1433;User Id=sa;...`):
//! reading the parts the client shows, and taking the password out so it can
//! live in the credential store instead of `profiles.json`.

/// One `key=value` piece, keeping its original text so a rebuilt string reads like the one that was typed.
#[derive(Debug, PartialEq)]
pub struct Part {
	/// Lower-case key.
	pub key:   String,
	pub value: String,
	/// The piece as written, without its `;`.
	pub raw:   String,
}

/// Splits on `;` outside braces and quotes.
pub fn parse(s: &str) -> Vec<Part> {
	let mut parts = Vec::new();
	let mut cur = String::new();
	let (mut brace, mut quote) = (false, None::<char>);
	let mut chars = s.chars().peekable();
	let flush = |cur: &mut String, parts: &mut Vec<Part>| {
		let raw = std::mem::take(cur);
		if let Some((k, v)) = raw.split_once('=') {
			let v = v.trim();
			let value = if let Some(inner) = v.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
				inner.replace("}}", "}")
			} else {
				v.trim_matches(|c| c == '"' || c == '\'').to_owned()
			};
			parts.push(Part { key: k.trim().to_lowercase(), value, raw: raw.trim().to_owned() });
		}
	};
	while let Some(c) = chars.next() {
		match c {
			'{' if quote.is_none() && !brace && cur.trim_end().ends_with('=') => {
				brace = true;
				cur.push(c);
			},
			'}' if brace => {
				if chars.peek() == Some(&'}') {
					cur.push_str("}}");
					chars.next();
				} else {
					brace = false;
					cur.push(c);
				}
			},
			'"' | '\'' if !brace => {
				match quote {
					Some(q) if q == c => quote = None,
					None if cur.trim_end().ends_with('=') => quote = Some(c),
					_ => {},
				}
				cur.push(c);
			},
			';' if !brace && quote.is_none() => flush(&mut cur, &mut parts),
			_ => cur.push(c),
		}
	}
	flush(&mut cur, &mut parts);
	parts
}

/// The first value among `keys` (already lower-case).
pub fn get<'a>(parts: &'a [Part], keys: &[&str]) -> Option<&'a str> {
	parts.iter().find(|p| keys.contains(&p.key.as_str()) && !p.value.is_empty()).map(|p| p.value.as_str())
}

const PASSWORD: [&str; 2] = ["password", "pwd"];
const SERVER: [&str; 4] = ["server", "data source", "address", "addr"];

/// The string without any password piece, and the password that was in it.
pub fn strip_password(s: &str) -> (String, Option<String>) {
	let parts = parse(s);
	let pw = get(&parts, &PASSWORD).map(str::to_owned);
	let kept: Vec<&str> = parts.iter().filter(|p| !PASSWORD.contains(&p.key.as_str())).map(|p| p.raw.as_str()).collect();
	(kept.join(";"), pw)
}

/// Host and port from a `Server` value such as `tcp:host,1433` or `host`.
pub fn host_port(server: &str) -> (String, u16) {
	let s = server.trim().trim_start_matches("tcp:");
	match s.rsplit_once(',') {
		Some((h, p)) => (h.trim().to_owned(), p.trim().parse().unwrap_or(1433)),
		None => (s.to_owned(), 1433),
	}
}

/// What the client shows about a string: server, port, user, database, trust.
pub struct Summary {
	pub server:   String,
	pub port:     u16,
	pub user:     String,
	pub database: Option<String>,
	pub trust:    bool,
}

/// The parts the client needs from `s`, or why it cannot connect.
pub fn summarize(s: &str) -> Result<Summary, String> {
	let parts = parse(s);
	let server = get(&parts, &SERVER).ok_or_else(|| "The connection string has no Server".to_owned())?;
	let (server, port) = host_port(server);
	let trust = get(&parts, &["trustservercertificate"]).is_some_and(|v| matches!(v.to_lowercase().as_str(), "true" | "yes" | "1"));
	Ok(Summary {
		server,
		port,
		user: get(&parts, &["user id", "uid", "user"]).unwrap_or_default().to_owned(),
		database: get(&parts, &["database", "initial catalog"]).map(str::to_owned),
		trust,
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn reads_the_usual_keys() {
		let s = summarize("Server=tcp:qas-db.example.com,1450;Initial Catalog=AppDb;User Id=sa;Password=p@ss;TrustServerCertificate=True").unwrap();
		assert_eq!((s.server.as_str(), s.port, s.user.as_str(), s.database.as_deref(), s.trust), ("qas-db.example.com", 1450, "sa", Some("AppDb"), true));
		assert!(summarize("Database=x").is_err());
	}

	#[test]
	fn password_comes_out_and_the_rest_stays() {
		let (rest, pw) = strip_password("Server=h;User Id=sa;Password=a=b;Encrypt=true");
		assert_eq!((rest.as_str(), pw.as_deref()), ("Server=h;User Id=sa;Encrypt=true", Some("a=b")));
		let (rest, pw) = strip_password("Server=h; Pwd={x;y}}z} ;Database=d");
		assert_eq!((rest.as_str(), pw.as_deref()), ("Server=h;Database=d", Some("x;y}z")));
		assert_eq!(strip_password("Server=h").1, None);
	}

	#[test]
	fn host_and_port() {
		assert_eq!(host_port("tcp:a.b,1433"), ("a.b".to_owned(), 1433));
		assert_eq!(host_port("a.b"), ("a.b".to_owned(), 1433));
		assert_eq!(host_port("a.b, 1500"), ("a.b".to_owned(), 1500));
	}
}
