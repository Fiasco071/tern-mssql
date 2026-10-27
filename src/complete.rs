//! Context-aware completion for T-SQL: what the caret is after decides what
//! is offered (tables after `FROM`, columns after `alias.`, databases after
//! `USE`), read from the active database's catalog.

use crate::{
	catalog::{Catalog, ObjKind},
	tsql::{self, Kind, Tok},
};

/// What a suggestion is, for its icon and order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ItemKind {
	Column,
	Table,
	View,
	Proc,
	Func,
	Schema,
	Database,
	Keyword,
}

/// One suggestion.
#[derive(Clone, Debug)]
pub struct Item {
	/// Shown label.
	pub label:  String,
	pub kind:   ItemKind,
	/// Dim text after the label.
	pub detail: String,
	/// What replaces the typed prefix.
	pub insert: String,
}

/// Suggestions and the byte where the typed prefix starts.
pub struct Completion {
	pub from:  usize,
	pub items: Vec<Item>,
}

const KEYWORDS: &[&str] = &[
	"SELECT", "FROM", "WHERE", "GROUP BY", "ORDER BY", "HAVING", "JOIN", "INNER JOIN", "LEFT JOIN",
	"RIGHT JOIN", "FULL JOIN", "CROSS JOIN", "ON", "AS", "AND", "OR", "NOT", "IN", "EXISTS",
	"BETWEEN", "LIKE", "IS NULL", "IS NOT NULL", "TOP", "DISTINCT", "UNION", "UNION ALL", "EXCEPT",
	"INTERSECT", "INSERT INTO", "VALUES", "UPDATE", "SET", "DELETE", "MERGE", "CREATE TABLE",
	"ALTER TABLE", "DROP TABLE", "TRUNCATE TABLE", "CASE", "WHEN", "THEN", "ELSE", "END", "WITH",
	"DECLARE", "BEGIN", "COMMIT", "ROLLBACK", "TRANSACTION", "TRY", "CATCH", "EXEC", "USE", "GO",
	"OVER", "PARTITION BY", "ASC", "DESC", "NULL", "OFFSET", "FETCH NEXT", "ROWS ONLY", "PRINT",
	"RETURN", "IF", "WHILE", "NOLOCK", "OUTPUT", "INTO",
];

const FUNCTIONS: &[&str] = &[
	"GETDATE()", "SYSDATETIME()", "ISNULL(", "COALESCE(", "NULLIF(", "COUNT(", "SUM(", "AVG(", "MIN(",
	"MAX(", "LEN(", "SUBSTRING(", "CHARINDEX(", "REPLACE(", "LTRIM(", "RTRIM(", "TRIM(", "UPPER(",
	"LOWER(", "CAST(", "CONVERT(", "TRY_CAST(", "TRY_CONVERT(", "DATEADD(", "DATEDIFF(", "DATEPART(",
	"EOMONTH(", "YEAR(", "MONTH(", "DAY(", "ROW_NUMBER()", "RANK()", "DENSE_RANK()", "NEWID()",
	"STRING_AGG(", "CONCAT(", "FORMAT(", "IIF(", "OBJECT_ID(", "DB_NAME()", "SCOPE_IDENTITY()",
	"ROUND(", "ABS(", "STUFF(", "LEFT(", "RIGHT(", "STRING_SPLIT(", "ISNUMERIC(", "ISDATE(",
];

/// Words after which a table name can follow.
const TABLE_CTX: &[&str] = &["FROM", "JOIN", "INTO", "UPDATE", "TABLE", "MERGE", "TRUNCATE"];
/// Words after which an expression with columns can follow.
const COLUMN_CTX: &[&str] = &[
	"SELECT", "WHERE", "ON", "BY", "SET", "HAVING", "AND", "OR", "WHEN", "THEN", "ELSE", "CASE", "TOP",
	"DISTINCT", "OUTPUT", "NOT",
];
/// Words that end a table reference, so cannot be an alias.
const NOT_ALIAS: &[&str] = &[
	"ON", "WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "OUTER", "GROUP", "ORDER",
	"HAVING", "UNION", "EXCEPT", "INTERSECT", "SET", "WITH", "OPTION", "USING", "WHEN", "OUTPUT",
	"VALUES", "SELECT", "FOR", "APPLY", "NATURAL", "FROM",
];

/// `[x]` and `"x"` to `x`.
fn unquote(s: &str) -> String {
	let b = s.as_bytes();
	if b.len() >= 2 && ((b[0] == b'[' && b[b.len() - 1] == b']') || (b[0] == b'"' && b[b.len() - 1] == b'"')) {
		return s[1..s.len() - 1].replace("]]", "]");
	}
	s.to_owned()
}

/// `name` as written in SQL: bracketed when it is not a plain identifier.
pub fn sql_name(name: &str) -> String {
	let plain = name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
		&& name.chars().all(|c| c.is_alphanumeric() || c == '_');
	if plain { name.to_owned() } else { format!("[{}]", name.replace(']', "]]")) }
}

/// A table reference in a statement: its alias (or name) and the object it names.
#[derive(Debug)]
pub struct TableRef {
	pub alias:  String,
	pub schema: Option<String>,
	pub name:   String,
}

/// The `FROM`/`JOIN`/`UPDATE`/`INTO` tables of a statement, with aliases.
pub fn table_refs(src: &str, toks: &[Tok]) -> Vec<TableRef> {
	let word = |i: usize| -> Option<&str> {
		toks.get(i).filter(|t| matches!(t.kind, Kind::Word | Kind::Quoted)).map(|t| t.text(src))
	};
	let punct = |i: usize, p: &str| toks.get(i).is_some_and(|t| t.kind == Kind::Punct && t.text(src) == p);
	let mut out = Vec::new();
	let mut i = 0;
	while i < toks.len() {
		let is_ctx = toks[i].kind == Kind::Word
			&& matches!(toks[i].text(src).to_uppercase().as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO" | "MERGE");
		i += 1;
		if !is_ctx {
			continue;
		}
		loop {
			// name(.name){0,3}
			let mut parts = Vec::new();
			while let Some(w) = word(i) {
				parts.push(unquote(w));
				i += 1;
				if punct(i, ".") {
					i += 1;
				} else {
					break;
				}
			}
			let Some(name) = parts.pop() else { break };
			let schema = parts.pop();
			let mut alias = name.clone();
			if word(i).is_some_and(|w| w.eq_ignore_ascii_case("as")) {
				i += 1;
			}
			if let Some(w) = word(i) {
				if !NOT_ALIAS.contains(&w.to_uppercase().as_str()) {
					alias = unquote(w);
					i += 1;
				}
			}
			out.push(TableRef { alias, schema, name });
			if punct(i, ",") {
				i += 1;
			} else {
				break;
			}
		}
	}
	out
}

/// Does `hay` contain `needle`'s chars in order (both lowercase)?
fn subsequence(hay: &str, needle: &str) -> bool {
	let mut it = hay.chars();
	needle.chars().all(|n| it.any(|h| h == n))
}

/// Computes suggestions at `cursor`. `force` asks for them with no prefix typed.
pub fn complete(src: &str, cursor: usize, cat: Option<&Catalog>, dbs: &[String], force: bool) -> Option<Completion> {
	let cursor = cursor.min(src.len());
	let mut from = cursor;
	for (i, c) in src[..cursor].char_indices().rev() {
		if c.is_alphanumeric() || matches!(c, '_' | '@' | '#' | '$') {
			from = i;
		} else {
			break;
		}
	}
	let prefix = src[from..cursor].to_lowercase();
	let (s0, s1) = tsql::statement_range(src, cursor);
	let (s0, s1) = (s0.min(cursor), s1.max(cursor));
	let stmt = &src[s0..s1];
	let all = tsql::lex(stmt);
	let before: Vec<Tok> = all.iter().copied().filter(|t| s0 + t.end <= from).collect();
	// A comment or string under the caret gets nothing.
	if all.iter().any(|t| matches!(t.kind, Kind::Comment | Kind::Str) && s0 + t.start < cursor && cursor <= s0 + t.end) {
		return None;
	}
	let last = before.last().map(|t| (t.kind, t.text(stmt)));
	let after_dot = matches!(last, Some((Kind::Punct, ".")));
	if prefix.is_empty() && !after_dot && !force {
		return None;
	}
	let refs = table_refs(stmt, &all);
	let mut items: Vec<Item> = Vec::new();

	if after_dot {
		let q = before.len().checked_sub(2).and_then(|i| before.get(i)).map(|t| unquote(t.text(stmt)));
		let q = q?;
		if let Some(r) = refs.iter().find(|r| r.alias.eq_ignore_ascii_case(&q)) {
			columns_of(cat, r, &mut items);
		} else if let Some(cat) = cat {
			if cat.schemas.iter().any(|s| s.eq_ignore_ascii_case(&q)) {
				for o in cat.objects.iter().filter(|o| o.schema.eq_ignore_ascii_case(&q)) {
					items.push(object_item(o.kind, &o.name, &o.schema, false));
				}
			} else if let Some(o) = cat.find(None, &q) {
				for c in &o.columns {
					items.push(column_item(&c.name, &c.ty, c.key, c.nullable));
				}
			}
		}
	} else {
		let ctx = before
			.iter()
			.rev()
			.filter(|t| t.kind == Kind::Word)
			.map(|t| t.text(stmt).to_uppercase())
			.find(|w| {
				TABLE_CTX.contains(&w.as_str()) || COLUMN_CTX.contains(&w.as_str()) || matches!(w.as_str(), "EXEC" | "EXECUTE" | "USE")
			})
			.unwrap_or_default();
		match ctx.as_str() {
			"USE" => items.extend(dbs.iter().map(|d| Item { label: d.clone(), kind: ItemKind::Database, detail: "database".into(), insert: sql_name(d) })),
			"EXEC" | "EXECUTE" => {
				if let Some(cat) = cat {
					items.extend(cat.of(ObjKind::Proc).map(|o| object_item(o.kind, &o.name, &o.schema, true)));
				}
			},
			c if TABLE_CTX.contains(&c) => {
				if let Some(cat) = cat {
					items.extend(cat.objects.iter().filter(|o| o.kind != ObjKind::Proc).map(|o| object_item(o.kind, &o.name, &o.schema, true)));
					items.extend(cat.schemas.iter().map(|s| Item { label: s.clone(), kind: ItemKind::Schema, detail: "schema".into(), insert: sql_name(s) }));
				}
			},
			_ => {
				for r in &refs {
					columns_of(cat, r, &mut items);
				}
				items.extend(FUNCTIONS.iter().map(|f| Item { label: (*f).into(), kind: ItemKind::Func, detail: "function".into(), insert: (*f).into() }));
				if let Some(cat) = cat {
					if refs.is_empty() {
						items.extend(cat.objects.iter().filter(|o| o.kind != ObjKind::Proc).map(|o| object_item(o.kind, &o.name, &o.schema, true)));
					}
				}
				items.extend(KEYWORDS.iter().map(|k| Item { label: (*k).into(), kind: ItemKind::Keyword, detail: String::new(), insert: (*k).into() }));
			},
		}
	}

	// Rank: prefix matches first, then contains, then in-order letters.
	let mut ranked: Vec<(u8, Item)> = items
		.into_iter()
		.filter_map(|it| {
			let l = it.label.to_lowercase();
			let rank = if prefix.is_empty() || l.starts_with(&prefix) {
				0
			} else if l.contains(&prefix) {
				1
			} else if subsequence(&l, &prefix) {
				2
			} else {
				return None;
			};
			Some((rank, it))
		})
		.collect();
	ranked.sort_by_cached_key(|a| (a.0, a.1.kind, a.1.label.to_lowercase()));
	ranked.dedup_by(|a, b| a.1.label == b.1.label && a.1.kind == b.1.kind);
	ranked.truncate(60);
	if ranked.is_empty() || (ranked.len() == 1 && ranked[0].1.insert.eq_ignore_ascii_case(&prefix)) {
		return None;
	}
	Some(Completion { from, items: ranked.into_iter().map(|(_, i)| i).collect() })
}

/// Adds the columns of the object a table reference names.
fn columns_of(cat: Option<&Catalog>, r: &TableRef, out: &mut Vec<Item>) {
	let Some(o) = cat.and_then(|c| c.find(r.schema.as_deref(), &r.name)) else { return };
	for c in &o.columns {
		out.push(column_item(&c.name, &format!("{} {}", r.alias, c.ty), c.key, c.nullable));
	}
}

/// A column suggestion.
fn column_item(name: &str, detail: &str, key: bool, nullable: bool) -> Item {
	let mark = if key { " key" } else if nullable { "" } else { " not null" };
	Item { label: name.to_owned(), kind: ItemKind::Column, detail: format!("{detail}{mark}"), insert: sql_name(name) }
}

/// An object suggestion; `qualify` inserts `schema.name`.
fn object_item(kind: ObjKind, name: &str, schema: &str, qualify: bool) -> Item {
	let k = match kind {
		ObjKind::Table => ItemKind::Table,
		ObjKind::View => ItemKind::View,
		ObjKind::Proc => ItemKind::Proc,
		ObjKind::Func => ItemKind::Func,
	};
	let insert = if qualify { format!("{}.{}", sql_name(schema), sql_name(name)) } else { sql_name(name) };
	Item { label: if qualify { format!("{schema}.{name}") } else { name.to_owned() }, kind: k, detail: schema.to_owned(), insert }
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::catalog::{Col, Obj};

	fn cat() -> Catalog {
		let mut c = Catalog::default();
		c.objects.push(Obj {
			schema: "dbo".into(),
			name: "Orders".into(),
			kind: ObjKind::Table,
			columns: vec![
				Col { name: "OrderId".into(), ty: "int".into(), nullable: false, identity: true, key: true },
				Col { name: "Total".into(), ty: "money".into(), nullable: true, identity: false, key: false },
			],
		});
		c.objects.push(Obj { schema: "dbo".into(), name: "Customers".into(), kind: ObjKind::Table, columns: vec![] });
		c.finish();
		c
	}

	#[test]
	fn tables_after_from() {
		let src = "select * from ord";
		let c = complete(src, src.len(), Some(&cat()), &[], false).unwrap();
		assert_eq!(c.items[0].insert, "dbo.Orders");
		assert_eq!(&src[c.from..], "ord");
	}

	#[test]
	fn columns_after_alias_dot() {
		let src = "select o. from dbo.Orders o";
		let c = complete(src, 9, Some(&cat()), &[], false).unwrap();
		assert_eq!(c.items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(), ["OrderId", "Total"]);
	}

	#[test]
	fn databases_after_use() {
		let src = "use ma";
		let c = complete(src, src.len(), None, &["master".into(), "model".into()], false).unwrap();
		assert_eq!(c.items[0].label, "master");
	}

	#[test]
	fn nothing_in_strings_or_with_no_prefix() {
		let src = "select 'ord";
		assert!(complete(src, src.len(), Some(&cat()), &[], false).is_none());
		assert!(complete("select ", 7, Some(&cat()), &[], false).is_none());
	}
}
