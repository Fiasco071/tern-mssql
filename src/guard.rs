//! A read-only guard for the default mode. It refuses statements that look
//! like they write. It is a seat belt, not a security boundary: use a login
//! with read-only rights when that matters.

/// Words that make a batch a write (or an escape hatch) in read-only mode.
const BLOCKED: &[&str] = &[
	"insert", "update", "delete", "drop", "alter", "create", "truncate", "merge", "grant",
	"revoke", "deny", "into", "backup", "restore", "shutdown", "kill", "reconfigure", "bulk",
	"openrowset", "opendatasource", "xp_cmdshell", "dbcc",
];

/// Stored procedures that only read.
const SAFE_PROCS: &[&str] = &["sp_help", "sp_columns", "sp_who", "sp_spaceused", "sp_tables"];

/// Drops string literals and comments so their words don't trip the guard.
fn strip(sql: &str) -> String {
	let mut out = String::with_capacity(sql.len());
	let mut chars = sql.chars().peekable();
	while let Some(c) = chars.next() {
		match c {
			'\'' => {
				while let Some(d) = chars.next() {
					if d == '\'' {
						if chars.peek() == Some(&'\'') {
							chars.next();
						} else {
							break;
						}
					}
				}
				out.push(' ');
			},
			'-' if chars.peek() == Some(&'-') => {
				for d in chars.by_ref() {
					if d == '\n' {
						break;
					}
				}
				out.push(' ');
			},
			'/' if chars.peek() == Some(&'*') => {
				chars.next();
				let mut prev = ' ';
				for d in chars.by_ref() {
					if prev == '*' && d == '/' {
						break;
					}
					prev = d;
				}
				out.push(' ');
			},
			_ => out.push(c),
		}
	}
	out
}

/// The offending word when `sql` looks like a write, else `None`.
pub fn check(sql: &str) -> Option<String> {
	let clean = strip(sql).to_lowercase();
	let words: Vec<&str> = clean
		.split(|c: char| !(c.is_alphanumeric() || c == '_'))
		.filter(|w| !w.is_empty())
		.collect();
	for (i, w) in words.iter().enumerate() {
		if BLOCKED.contains(w) {
			return Some((*w).to_owned());
		}
		if matches!(*w, "exec" | "execute") {
			let next = words.get(i + 1).copied().unwrap_or_default();
			if !SAFE_PROCS.iter().any(|p| next.starts_with(p)) {
				return Some((*w).to_owned());
			}
		}
	}
	None
}

#[cfg(test)]
mod tests {
	use super::check;

	#[test]
	fn selects_pass() {
		assert_eq!(check("select top 10 * from dbo.orders where note = 'drop table x'"), None);
		assert_eq!(check("with a as (select 1) select * from a -- delete me"), None);
		assert_eq!(check("exec sp_help 'dbo.orders'"), None);
	}

	#[test]
	fn writes_are_caught() {
		assert_eq!(check("UPDATE dbo.t SET a = 1").as_deref(), Some("update"));
		assert_eq!(check("select * into x from y").as_deref(), Some("into"));
		assert_eq!(check("exec dbo.doThing").as_deref(), Some("exec"));
		assert_eq!(check("/* hi */ DROP TABLE t").as_deref(), Some("drop"));
	}
}
