//! "Script as": T-SQL generated from the catalog.

use crate::catalog::Obj;

/// What to script.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
	Select,
	Insert,
	Update,
	Delete,
	Create,
}

impl Kind {
	/// The context-menu action name.
	pub const fn act(self) -> &'static str {
		match self {
			Self::Select => "Script as SELECT",
			Self::Insert => "Script as INSERT",
			Self::Update => "Script as UPDATE",
			Self::Delete => "Script as DELETE",
			Self::Create => "Script as CREATE",
		}
	}
}

/// `[name]` with `]` doubled.
fn b(name: &str) -> String {
	format!("[{}]", name.replace(']', "]]"))
}

/// The script for `obj`, addressed as `q` (already quoted, maybe three-part).
pub fn generate(kind: Kind, obj: &Obj, q: &str) -> String {
	let cols = &obj.columns;
	let list = |sep: &str, f: &dyn Fn(&crate::catalog::Col) -> String| cols.iter().map(f).collect::<Vec<_>>().join(sep);
	match kind {
		Kind::Select => format!("SELECT {}\nFROM {q};", list(",\n       ", &|c| b(&c.name))),
		Kind::Insert => {
			let writable: Vec<_> = cols.iter().filter(|c| !c.identity).collect();
			format!(
				"INSERT INTO {q}\n\t({})\nVALUES\n\t({});",
				writable.iter().map(|c| b(&c.name)).collect::<Vec<_>>().join(", "),
				writable.iter().map(|c| format!("<{}, {}>", c.name, c.ty)).collect::<Vec<_>>().join(", ")
			)
		},
		Kind::Update => {
			let set = cols.iter().filter(|c| !c.identity && !c.key).map(|c| format!("{} = <{}, {}>", b(&c.name), c.name, c.ty)).collect::<Vec<_>>().join(",\n\t");
			format!("UPDATE {q}\nSET {set}\nWHERE {};", key_filter(cols))
		},
		Kind::Delete => format!("DELETE FROM {q}\nWHERE {};", key_filter(cols)),
		Kind::Create => {
			let mut lines: Vec<String> = cols
				.iter()
				.map(|c| format!("\t{} {}{}{}", b(&c.name), c.ty, if c.identity { " IDENTITY(1,1)" } else { "" }, if c.nullable { " NULL" } else { " NOT NULL" }))
				.collect();
			let keys: Vec<_> = cols.iter().filter(|c| c.key).map(|c| b(&c.name)).collect();
			if !keys.is_empty() {
				lines.push(format!("\tCONSTRAINT {} PRIMARY KEY ({})", b(&format!("PK_{}", obj.name)), keys.join(", ")));
			}
			format!("CREATE TABLE {q}\n(\n{}\n);", lines.join(",\n"))
		},
	}
}

/// A WHERE clause on the key columns, or a placeholder when there are none.
fn key_filter(cols: &[crate::catalog::Col]) -> String {
	let keys: Vec<_> = cols.iter().filter(|c| c.key).map(|c| format!("{} = <{}, {}>", b(&c.name), c.name, c.ty)).collect();
	if keys.is_empty() { "<search condition>".to_owned() } else { keys.join("\n  AND ") }
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::catalog::{Col, ObjKind};

	fn table() -> Obj {
		let col = |name: &str, ty: &str, identity: bool, key: bool| Col { name: name.into(), ty: ty.into(), nullable: !key, identity, key };
		Obj {
			schema:  "dbo".into(),
			name:    "T".into(),
			kind:    ObjKind::Table,
			columns: vec![col("Id", "int", true, true), col("Name", "nvarchar(50)", false, false)],
		}
	}

	#[test]
	fn scripts() {
		let t = table();
		assert_eq!(generate(Kind::Insert, &t, "[dbo].[T]"), "INSERT INTO [dbo].[T]\n\t([Name])\nVALUES\n\t(<Name, nvarchar(50)>);");
		assert!(generate(Kind::Create, &t, "[dbo].[T]").contains("[Id] int IDENTITY(1,1) NOT NULL"));
		assert!(generate(Kind::Delete, &t, "[dbo].[T]").contains("[Id] = <Id, int>"));
	}
}
