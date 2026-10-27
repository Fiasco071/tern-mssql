//! SQL Server over tiberius, driven from a private current-thread runtime so
//! the synchronous Tern SDK loop never sees async.

use anyhow::Context;
use tiberius::{Client, ColumnData, Config};
use tokio::{net::TcpStream, runtime::Runtime};
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use super::{Backend, Rows};

/// A live connection plus the runtime that drives it.
pub struct Mssql {
	rt:     Runtime,
	client: Client<Compat<TcpStream>>,
}

impl Mssql {
	/// Connects with an ADO connection string (`server=tcp:host,1433;user id=...`).
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
fn cell(data: &ColumnData<'_>) -> Option<String> {
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
		C::Binary(v) => v.as_ref().map(|b| format!("0x{}", b.iter().map(|x| format!("{x:02X}")).collect::<String>())),
		C::Numeric(v) => v.map(|x| x.to_string()),
		other => Some(format!("{other:?}")),
	}
}

impl Backend for Mssql {
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
			use tokio_stream_shim::next;
			while let Some(row) = next(&mut it).await {
				if rows.len() >= limit {
					capped = true;
					break;
				}
				rows.push(row?.into_iter().map(|d| cell(&d)).collect());
			}
			Ok(Rows { columns, rows, capped })
		})
	}
}

/// `futures::StreamExt::next` without pulling in all of futures.
mod tokio_stream_shim {
	use std::{future::poll_fn, pin::Pin};

	use futures_core::Stream;

	pub async fn next<S: Stream + Unpin>(s: &mut S) -> Option<S::Item> {
		poll_fn(|cx| Pin::new(&mut *s).poll_next(cx)).await
	}
}
