//! Client-side shaping of a loaded result set: sort, filter, and the JSON and
//! INSERT writers. Cells are display text, so a value that reads as a plain
//! number is treated as one.

use std::cmp::Ordering;

pub type Row = Vec<Option<String>>;

/// True when `s` is a plain decimal number (no leading zeros, so `007` stays text).
pub fn numeric(s: &str) -> bool {
	let t = s.strip_prefix('-').unwrap_or(s);
	let int = t.split('.').next().unwrap_or("");
	!t.is_empty()
		&& t.chars().all(|c| c.is_ascii_digit() || c == '.')
		&& t.matches('.').count() <= 1
		&& !t.starts_with('.')
		&& !t.ends_with('.')
		&& (int == "0" || !int.starts_with('0'))
		&& s.parse::<f64>().is_ok()
}

/// Orders two cells: NULL first, numbers by value, everything else by text ignoring case.
pub fn compare(a: &Option<String>, b: &Option<String>) -> Ordering {
	match (a, b) {
		(None, None) => Ordering::Equal,
		(None, _) => Ordering::Less,
		(_, None) => Ordering::Greater,
		(Some(x), Some(y)) => match (numeric(x), numeric(y)) {
			(true, true) => x.parse::<f64>().unwrap_or(0.0).total_cmp(&y.parse::<f64>().unwrap_or(0.0)),
			_ => x.to_lowercase().cmp(&y.to_lowercase()),
		},
	}
}

/// `base` filtered to rows with `needle` in any cell (ignoring case), then sorted by `sort` (column, ascending).
pub fn shape(base: &[Row], needle: &str, sort: Option<(usize, bool)>) -> Vec<Row> {
	let needle = needle.to_lowercase();
	let mut rows: Vec<Row> = base
		.iter()
		.filter(|r| needle.is_empty() || r.iter().flatten().any(|c| c.to_lowercase().contains(&needle)))
		.cloned()
		.collect();
	if let Some((c, asc)) = sort {
		rows.sort_by(|a, b| {
			let o = compare(a.get(c).unwrap_or(&None), b.get(c).unwrap_or(&None));
			if asc { o } else { o.reverse() }
		});
	}
	rows
}

/// The rows as a JSON array of objects.
pub fn json(columns: &[String], rows: &[Row]) -> String {
	let list: Vec<serde_json::Value> = rows
		.iter()
		.map(|r| {
			let mut o = serde_json::Map::new();
			for (c, name) in columns.iter().enumerate() {
				let v = match r.get(c).and_then(|v| v.as_deref()) {
					None => serde_json::Value::Null,
					Some(s) if numeric(s) => s.parse::<serde_json::Number>().map_or_else(|_| s.into(), serde_json::Value::Number),
					Some(s) => s.into(),
				};
				o.insert(name.clone(), v);
			}
			serde_json::Value::Object(o)
		})
		.collect();
	serde_json::to_string_pretty(&list).unwrap_or_default()
}

/// `[name]` with `]` doubled.
fn bracket(name: &str) -> String {
	format!("[{}]", name.replace(']', "]]"))
}

/// One T-SQL literal for a cell.
fn literal(cell: Option<&str>) -> String {
	match cell {
		None => "NULL".to_owned(),
		Some(s) if numeric(s) => s.to_owned(),
		Some(s) => format!("N'{}'", s.replace('\'', "''")),
	}
}

/// One `INSERT` per row into `table` (already bracketed, or a placeholder).
pub fn inserts(table: &str, columns: &[String], rows: &[Row]) -> String {
	let cols = columns.iter().map(|c| bracket(c)).collect::<Vec<_>>().join(", ");
	rows.iter()
		.map(|r| {
			let vals = (0..columns.len()).map(|c| literal(r.get(c).and_then(|v| v.as_deref()))).collect::<Vec<_>>().join(", ");
			format!("INSERT INTO {table} ({cols}) VALUES ({vals});")
		})
		.collect::<Vec<_>>()
		.join("\n")
}

#[cfg(test)]
mod tests {
	use super::*;

	fn r(a: Option<&str>, b: Option<&str>) -> Row {
		vec![a.map(str::to_owned), b.map(str::to_owned)]
	}

	#[test]
	fn numbers_and_text() {
		assert!(numeric("12") && numeric("-3.5") && numeric("0.5"));
		assert!(!numeric("007") && !numeric("1e3") && !numeric("a") && !numeric("") && !numeric("1.2.3"));
	}

	#[test]
	fn sorts_numbers_by_value_and_filters() {
		let rows = vec![r(Some("10"), Some("b")), r(Some("9"), Some("A")), r(None, Some("c"))];
		let s = shape(&rows, "", Some((0, true)));
		assert_eq!(s[0][0], None);
		assert_eq!(s[1][0].as_deref(), Some("9"));
		assert_eq!(s[2][0].as_deref(), Some("10"));
		assert_eq!(shape(&rows, "a", None).len(), 1);
		assert_eq!(shape(&rows, "", Some((1, false)))[0][1].as_deref(), Some("c"));
	}

	#[test]
	fn writers() {
		let cols = vec!["Id".to_owned(), "Name".to_owned()];
		let rows = vec![r(Some("1"), Some("O'Brien")), r(None, Some("007"))];
		assert_eq!(
			inserts("[dbo].[T]", &cols, &rows),
			"INSERT INTO [dbo].[T] ([Id], [Name]) VALUES (1, N'O''Brien');\nINSERT INTO [dbo].[T] ([Id], [Name]) VALUES (NULL, N'007');"
		);
		let j: serde_json::Value = serde_json::from_str(&json(&cols, &rows)).unwrap();
		assert_eq!(j[0]["Id"], 1);
		assert!(j[1]["Id"].is_null());
		assert_eq!(j[1]["Name"], "007");
	}
}
