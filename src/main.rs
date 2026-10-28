//! `tern-mssql [PROFILE] [FILE.sql...]`: the interactive client (a picker when PROFILE is omitted).
//! `tern-mssql query PROFILE SQL`: one batch printed as a table.
//! `tern-mssql --new`: the picker, opened on a blank new-connection form.
//! `tern-mssql profiles`: the saved profiles.

mod app;
mod backend;
mod catalog;
mod clip;
mod complete;
mod connect;
mod guard;
mod profile;
mod script;
mod tsql;
mod view;
mod worker;

use std::path::PathBuf;

use anyhow::Context;
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

/// The named profile, connected, with its server name.
fn connect(name: &str) -> anyhow::Result<(Mssql, String)> {
	let profiles = profile::load()?;
	let p = profiles
		.iter()
		.find(|p| p.name == name)
		.with_context(|| format!("no profile named {name}"))?;
	let db = Mssql::connect(&p.ado()?).with_context(|| format!("connecting to {}", p.server))?;
	Ok((db, p.server.clone()))
}

fn main() -> anyhow::Result<()> {
	let args: Vec<String> = std::env::args().skip(1).collect();
	// `--trace FILE` logs every input the client receives, for diagnosing what Tern sends.
	let (args, trace) = match args.iter().position(|a| a == "--trace") {
		Some(i) if i + 1 < args.len() => {
			let mut rest = args.clone();
			let file = rest.remove(i + 1);
			rest.remove(i);
			(rest, Some(file))
		},
		_ => (args, None),
	};
	if let Some(file) = trace {
		// Nothing else is running yet, so setting the environment is safe.
		unsafe { std::env::set_var("TERN_MSSQL_TRACE", file) };
	}
	match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
		["profiles"] => {
			for p in profile::load()? {
				println!("{}\t{}:{}\t{}", p.name, p.server, p.port, p.user);
			}
			Ok(())
		},
		["query", name, sql] => {
			let rows = connect(name)?.0.query(sql, 1000)?;
			tern_sdk::print(view(&rows), PrintOptions::new().css(CSS))?;
			if rows.capped {
				eprintln!("(capped at 1000 rows)");
			}
			Ok(())
		},
		[] | ["--new"] => match connect::choose(!args.is_empty())? {
			Some((db, name, server)) => app::run(db, name, server, &[]),
			None => Ok(()),
		},
		[name, files @ ..] => {
			let (db, server) = connect(name)?;
			let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
			app::run(db, (*name).to_owned(), server, &files)
		},
	}
}
