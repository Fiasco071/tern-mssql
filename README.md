# tern-mssql

SQL Server client for [Tern](https://stencil.so/tern), in Rust: `tern-sdk` for native surfaces, `tiberius` for TDS. One static binary per OS, no runtime.

    tern-mssql "server=tcp:localhost,1433;user id=sa;password=...;TrustServerCertificate=true"   # list tables
    tern-mssql "<conn>" "select top 10 * from sys.objects"                                       # run a query

`TERN_MSSQL_CONN` can hold the connection string instead. Output is a native Tern table inside Tern and aligned plain text elsewhere.

Status: v0 slice. Not yet run against a live server. Next: interactive explorer, query pane, Entra tokens, export.
