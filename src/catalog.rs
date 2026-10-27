//! The active database's objects and columns, loaded in the background after
//! connecting or switching. The explorer, completion and the editor's
//! unknown-name marks all read it.

use std::collections::HashMap;

use crate::backend::Backend;

/// What an object is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjKind {
	/// A user table.
	Table,
	/// A view.
	View,
	/// A stored procedure.
	Proc,
	/// A scalar or table-valued function.
	Func,
}

/// One column.
#[derive(Clone, Debug)]
pub struct Col {
	pub name:     String,
	/// Display type, `varchar(50)`.
	pub ty:       String,
	pub nullable: bool,
	pub identity: bool,
	pub key:      bool,
}

/// One schema object.
#[derive(Clone, Debug)]
pub struct Obj {
	pub schema:  String,
	pub name:    String,
	pub kind:    ObjKind,
	pub columns: Vec<Col>,
}

impl Obj {
	/// `schema.name`.
	pub fn full(&self) -> String {
		format!("{}.{}", self.schema, self.name)
	}
}

/// Everything known about one database.
#[derive(Default)]
pub struct Catalog {
	pub objects: Vec<Obj>,
	/// Distinct schema names, sorted.
	pub schemas: Vec<String>,
	/// Lowercase `schema.name` to index in `objects`.
	by_full:     HashMap<String, usize>,
	/// Lowercase `name` to every index with it.
	by_name:     HashMap<String, Vec<usize>>,
}

const OBJECTS: &str = "SELECT s.name, o.name, o.type \
	FROM {p}sys.objects o JOIN {p}sys.schemas s ON s.schema_id = o.schema_id \
	WHERE o.is_ms_shipped = 0 AND o.type IN ('U','V','P','FN','IF','TF','FS','FT') \
	ORDER BY s.name, o.name";

const COLUMNS: &str = "SELECT s.name, o.name, c.name, \
	CASE WHEN t.name IN ('varchar','char','varbinary','binary') \
		THEN t.name + '(' + IIF(c.max_length = -1, 'max', CAST(c.max_length AS varchar(10))) + ')' \
	WHEN t.name IN ('nvarchar','nchar') \
		THEN t.name + '(' + IIF(c.max_length = -1, 'max', CAST(c.max_length / 2 AS varchar(10))) + ')' \
	WHEN t.name IN ('decimal','numeric') \
		THEN t.name + '(' + CAST(c.precision AS varchar(5)) + ',' + CAST(c.scale AS varchar(5)) + ')' \
	ELSE t.name END, \
	c.is_nullable, c.is_identity, \
	CASE WHEN EXISTS (SELECT 1 FROM {p}sys.index_columns ic JOIN {p}sys.indexes i \
		ON i.object_id = ic.object_id AND i.index_id = ic.index_id \
		WHERE i.is_primary_key = 1 AND ic.object_id = c.object_id AND ic.column_id = c.column_id) \
		THEN 1 ELSE 0 END \
	FROM {p}sys.columns c \
	JOIN {p}sys.objects o ON o.object_id = c.object_id \
	JOIN {p}sys.schemas s ON s.schema_id = o.schema_id \
	JOIN {p}sys.types t ON t.user_type_id = c.user_type_id \
	WHERE o.is_ms_shipped = 0 AND o.type IN ('U','V','IF','TF') \
	ORDER BY s.name, o.name, c.column_id";

/// A cell as text, empty for NULL.
fn text(c: &Option<String>) -> &str {
	c.as_deref().unwrap_or_default()
}

impl Catalog {
	/// Loads a database's catalog: `database` by three-part names, or the connection's current one for `None`.
	pub fn load(db: &mut dyn Backend, database: Option<&str>) -> anyhow::Result<Self> {
		let p = database.map_or_else(String::new, |d| format!("[{}].", d.replace(']', "]]")));
		let mut cat = Self::default();
		for r in db.query(&OBJECTS.replace("{p}", &p), usize::MAX)?.rows {
			let kind = match text(&r[2]).trim() {
				"U" => ObjKind::Table,
				"V" => ObjKind::View,
				"P" => ObjKind::Proc,
				"IF" | "TF" | "FN" | "FS" | "FT" => ObjKind::Func,
				_ => continue,
			};
			cat.objects.push(Obj {
				schema: text(&r[0]).to_owned(),
				name: text(&r[1]).to_owned(),
				kind,
				columns: Vec::new(),
			});
		}
		cat.finish();
		for r in db.query(&COLUMNS.replace("{p}", &p), usize::MAX)?.rows {
			let full = format!("{}.{}", text(&r[0]), text(&r[1])).to_lowercase();
			let Some(&i) = cat.by_full.get(&full) else { continue };
			cat.objects[i].columns.push(Col {
				name: text(&r[2]).to_owned(),
				ty: text(&r[3]).to_owned(),
				nullable: text(&r[4]) == "true",
				identity: text(&r[5]) == "true",
				key: text(&r[6]) == "1",
			});
		}
		Ok(cat)
	}

	/// Builds the lookup maps and the schema list once `objects` is filled.
	pub fn finish(&mut self) {
		self.by_full.clear();
		self.by_name.clear();
		let mut schemas = Vec::new();
		for (i, o) in self.objects.iter().enumerate() {
			self.by_full.insert(o.full().to_lowercase(), i);
			self.by_name.entry(o.name.to_lowercase()).or_default().push(i);
			if !schemas.contains(&o.schema) {
				schemas.push(o.schema.clone());
			}
		}
		schemas.sort();
		self.schemas = schemas;
	}

	/// An object by `name` or `schema.name`, any case. A bare name prefers `dbo`.
	pub fn find(&self, schema: Option<&str>, name: &str) -> Option<&Obj> {
		let name = name.to_lowercase();
		if let Some(s) = schema {
			return self.by_full.get(&format!("{}.{name}", s.to_lowercase())).map(|&i| &self.objects[i]);
		}
		let hits = self.by_name.get(&name)?;
		let pick = hits.iter().find(|&&i| self.objects[i].schema.eq_ignore_ascii_case("dbo")).unwrap_or(&hits[0]);
		Some(&self.objects[*pick])
	}

	/// Objects of `kind`, in schema and name order.
	pub fn of(&self, kind: ObjKind) -> impl Iterator<Item = &Obj> {
		self.objects.iter().filter(move |o| o.kind == kind)
	}
}
