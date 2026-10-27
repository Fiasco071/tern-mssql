//! SQL Server over tiberius, driven from a private current-thread runtime so
//! the synchronous Tern SDK loop never sees async.

use std::{future::poll_fn, pin::Pin};

use anyhow::Context;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use futures_core::Stream;
use tiberius::{Client, ColumnData, Config, FromSql};
use tokio::{net::TcpStream, runtime::Runtime};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::{Backend, Cancel, Outcome, Sink};

/// The wire connection.
type Conn = Client<Compat<TcpStream>>;

/// A live connection plus the runtime that drives it.
pub struct Mssql {
	rt:       Runtime,
	ado:      String,
	/// The database the connection is in, restored after a reconnect.
	database: Option<String>,
	client:   Conn,
}

/// Opens a connection.
async fn open(ado: &str) -> anyhow::Result<Conn> {
	let config = Config::from_ado_string(ado).context("bad connection string")?;
	let tcp = TcpStream::connect(config.get_addr()).await?;
	tcp.set_nodelay(true)?;
	Ok(Client::connect(config, tcp.compat_write()).await?)
}

/// `[name]` with `]` doubled.
fn bracket(name: &str) -> String {
	format!("[{}]", name.replace(']', "]]"))
}

impl Mssql {
	/// Connects with an ADO connection string.
	pub fn connect(ado: &str) -> anyhow::Result<Self> {
		let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
		let client = rt.block_on(open(ado))?;
		Ok(Self { rt, ado: ado.to_owned(), database: None, client })
	}

	/// Replaces the connection, which closes the old socket and so stops
	/// whatever the server was running for it.
	fn reconnect(&mut self) -> anyhow::Result<()> {
		let ado = self.ado.clone();
		let db = self.database.clone();
		let fresh = self.rt.block_on(async {
			let mut fresh = open(&ado).await?;
			if let Some(db) = db {
				fresh.simple_query(format!("USE {}", bracket(&db))).await?.into_results().await?;
			}
			anyhow::Ok(fresh)
		})?;
		self.client = fresh;
		Ok(())
	}
}

/// One cell as display text; `None` is NULL.
fn cell(data: &ColumnData<'static>) -> Option<String> {
	use ColumnData as C;
	match data {
		C::U8(v) => v.map(|x| x.to_string()),
		C::I16(v) => v.map(|x| x.to_string()),
		C::I32(v) => v.map(|x| x.to_string()),
		C::I64(v) => v.map(|x| x.to_string()),
		C::F32(v) => v.map(|x| x.to_string()),
		C::F64(v) => v.map(|x| x.to_string()),
		C::Bit(v) => v.map(|x| x.to_string()),
		C::String(v) => v.as_ref().map(|x| x.to_string()),
		C::Guid(v) => v.map(|x| x.to_string()),
		C::Binary(v) => v
			.as_ref()
			.map(|b| format!("0x{}", b.iter().map(|x| format!("{x:02X}")).collect::<String>())),
		C::Numeric(v) => v.map(|x| x.to_string()),
		C::DateTime(_) | C::SmallDateTime(_) | C::DateTime2(_) => {
			NaiveDateTime::from_sql(data).ok().flatten().map(|d| d.to_string())
		},
		C::Date(_) => NaiveDate::from_sql(data).ok().flatten().map(|d| d.to_string()),
		C::Time(_) => NaiveTime::from_sql(data).ok().flatten().map(|d| d.to_string()),
		C::DateTimeOffset(_) => {
			DateTime::<FixedOffset>::from_sql(data).ok().flatten().map(|d| d.to_string())
		},
		C::Xml(v) => v.as_ref().map(|x| x.to_string()),
	}
}

/// `StreamExt::next` without pulling in all of futures.
async fn next<S: Stream + Unpin>(s: &mut S) -> Option<S::Item> {
	poll_fn(|cx| Pin::new(&mut *s).poll_next(cx)).await
}

impl Backend for Mssql {
	fn use_database(&mut self, name: &str) -> anyhow::Result<()> {
		self.query(&format!("USE {}", bracket(name)), 0)?;
		self.database = Some(name.to_owned());
		Ok(())
	}

	fn stream(&mut self, sql: &str, cancel: &Cancel, sink: &mut dyn Sink) -> anyhow::Result<Outcome> {
		let client = &mut self.client;
		let outcome = self.rt.block_on(async {
			let run = async {
				let mut stream = client.simple_query(sql).await?;
				if let Some(cols) = stream.columns().await? {
					let names: Vec<String> = cols.iter().map(|c| c.name().to_owned()).collect();
					sink.columns(&names);
				}
				let mut rows = stream.into_row_stream();
				while let Some(row) = next(&mut rows).await {
					let row = row?;
					if !sink.row(row.into_iter().map(|d| cell(&d)).collect()) {
						break;
					}
				}
				anyhow::Ok(())
			};
			tokio::select! {
				r = run => r.map(|()| Outcome::Done),
				() = cancel.cancelled() => anyhow::Ok(Outcome::Cancelled),
			}
		})?;
		if matches!(outcome, Outcome::Cancelled) {
			self.reconnect()?;
		}
		Ok(outcome)
	}
}
