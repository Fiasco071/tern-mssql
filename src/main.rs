//! `tern-mssql PROFILE`: the interactive client.
//! `tern-mssql query PROFILE SQL`: one batch printed as a table.
//! `tern-mssql profiles`: the saved profiles.

mod app;
mod backend;
mod guard;
mod profile;

use anyhow::{Context, bail};
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
			Some(s) => html::td(s.replace(['\r', '\n'], " ")),
			None => html::td("NULL").class("null"),
		}))
	});
	View::new().main([html::table()
		.class("rs")
		.child(html::thead().child(head))
		.child(html::tbody().children(body))])
}

/// The named profile, connected.
fn connect(name: &str) -> anyhow::Result<Mssql> {
	let profiles = profile::load()?;
	let p = profiles
		.iter()
		.find(|p| p.name == name)
		.with_context(|| format!("no profile named {name}"))?;
	Mssql::connect(&p.ado()?).with_context(|| format!("connecting to {}", p.server))
}

fn main() -> anyhow::Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
		["profiles"] => {
			for p in profile::load()? {
				println!("{}\t{}:{}\t{}", p.name, p.server, p.port, p.user);
			}
			Ok(())
		},
		["query", name, sql] => {
			let rows = connect(name)?.query(sql, 1000)?;
			tern_sdk::print(view(&rows), PrintOptions::new().css(CSS))?;
			if rows.capped {
				eprintln!("(capped at 1000 rows)");
			}
			Ok(())
		},
		[name] => app::run(connect(name)?, (*name).to_owned()),
		_ => bail!(
			"usage: tern-mssql PROFILE | query PROFILE SQL | profiles   (profiles: {})",
			profile::path().display()
		),
	}
}
