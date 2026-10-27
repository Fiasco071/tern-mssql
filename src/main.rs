//! `tern-mssql CONNECTION_STRING [SQL]`: lists tables, or runs SQL and prints
//! the result as a native Tern table (aligned plain text outside Tern).

mod backend;

use backend::{Backend, Rows, mssql::Mssql};
use tern_sdk::{
	PrintOptions,
	ui::{View, html},
};

const CSS: &str = ".rs{border-spacing:0} .rs th{text-align:left;padding:0 14px 3px 0;color:var(--sf-c-muted);font-weight:500} .rs td{padding:1px 14px 1px 0} .rs .null{color:var(--sf-c-muted);font-style:italic}";

/// A result set as a table.
fn view(rows: &Rows) -> View {
	let head = html::tr().children(rows.columns.iter().map(|c| html::th(c.as_str())));
	let body = rows.rows.iter().map(|r| {
		html::tr().children(r.iter().map(|c| match c {
			Some(s) => html::td(s.as_str()),
			None => html::td("NULL").class("null"),
		}))
	});
	View::new().main([html::table()
		.class("rs")
		.child(html::thead().child(head))
		.child(html::tbody().children(body))])
}

fn main() -> anyhow::Result<()> {
	let mut args = std::env::args().skip(1);
	let conn = args.next().or_else(|| std::env::var("TERN_MSSQL_CONN").ok());
	let Some(conn) = conn else {
		eprintln!("usage: tern-mssql CONNECTION_STRING [SQL]   (or set TERN_MSSQL_CONN)");
		std::process::exit(2);
	};
	let mut db = Mssql::connect(&conn)?;
	let rows = match args.next() {
		Some(sql) => db.query(&sql, 1000)?,
		None => Rows {
			columns: vec!["table".into()],
			rows:    db.tables()?.into_iter().map(|t| vec![Some(t)]).collect(),
			capped:  false,
		},
	};
	tern_sdk::print(view(&rows), PrintOptions::new().css(CSS))?;
	if rows.capped {
		eprintln!("(capped at 1000 rows)");
	}
	Ok(())
}
