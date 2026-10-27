//! SQL Server over tiberius, driven from a private current-thread runtime so
//! the synchronous Tern SDK loop never sees async.

use std::{future::poll_fn, pin::Pin};

use anyhow::Context;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime};
use futures_core::Stream;
use tiberius::{Client, ColumnData, Config, FromSql};
use tokio::{net::TcpStream, runtime::Runtime};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::{Backend, Rows};

/// A live connection plus the runtime that drives it.
pub struct Mssql {
	rt:     Runtime,
	client: Client<Compat<TcpStream>>,
}

impl Mssql {
	/// Connects with an ADO connection string.
	pub fn connect(ado: &str) -> anyhow::Result<Self> {
		let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
		let config = Config::from_ado_string(ado).context("bad connection string")?;
		let client = rt.block_on(async {
			let tcp = TcpStream::connect(config.get_addr()).await?;
			tcp.set_nodelay(true)?;
			Ok::<_, anyhow::Error>(Client::connect(config, tcp.compat_write()).await?)
		})?;
		Ok(Self { rt, client })
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
	fn databases(&mut self) -> anyhow::Result<Vec<String>> {
		let r = self.query("SELECT name FROM sys.databases WHERE state = 0 ORDER BY name", usize::MAX)?;
		Ok(r.rows.into_iter().filter_map(|mut r| r.pop().flatten()).collect())
	}

	fn current(&mut self) -> anyhow::Result<String> {
		let r = self.query("SELECT DB_NAME()", 1)?;
		Ok(r.rows.into_iter().next().and_then(|mut r| r.pop().flatten()).unwrap_or_default())
	}

	fn use_database(&mut self, name: &str) -> anyhow::Result<()> {
		self.query(&format!("USE [{}]", name.replace(']', "]]")), 0)?;
		Ok(())
	}

	fn tables(&mut self) -> anyhow::Result<Vec<String>> {
		let rows = self.query(
			"SELECT TABLE_SCHEMA + '.' + TABLE_NAME FROM INFORMATION_SCHEMA.TABLES ORDER BY 1",
			usize::MAX,
		)?;
		Ok(rows.rows.into_iter().filter_map(|mut r| r.pop().flatten()).collect())
	}

	fn query(&mut self, sql: &str, limit: usize) -> anyhow::Result<Rows> {
		let client = &mut self.client;
		self.rt.block_on(async {
			let mut stream = client.simple_query(sql).await?;
			let columns = stream
				.columns()
				.await?
				.map(|c| c.iter().map(|c| c.name().to_owned()).collect())
				.unwrap_or_default();
			let mut rows = Vec::new();
			let mut capped = false;
			let mut it = stream.into_row_stream();
			while let Some(row) = next(&mut it).await {
				let row = row?;
				if rows.len() >= limit {
					capped = true;
					break;
				}
				rows.push(row.into_iter().map(|d| cell(&d)).collect());
			}
			Ok(Rows { columns, rows, capped })
		})
	}
}
