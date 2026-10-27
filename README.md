# tern-mssql

SQL Server client for [Tern](https://stencil.so/tern), in Rust: `tern-sdk` for native surfaces, `tiberius` for TDS. One native binary per OS, no runtime.

    tern-mssql PROFILE                 interactive: explorer, query editor, results grid
    tern-mssql query PROFILE "SQL"     one batch printed as a table
    tern-mssql profiles                list saved profiles

Profiles live in `%APPDATA%\tern-mssql\profiles.json` (`~/.config/tern-mssql/profiles.json` elsewhere). A profile names the env var that holds the password, so the file holds no secret:

    [{ "name": "qas", "server": "host", "user": "sa", "password_env": "QAS_DB_PW", "trust_cert": true }]

Keys: F5 or ctrl+enter runs, tab switches explorer/editor, ctrl+w toggles write mode, ctrl+q quits.
Read-only is the default. The guard refuses statements that look like writes; it is a seat belt, not a security boundary, so use a read-only login when that matters.

Status: runs against a live server. Not yet built or tested: cancel, Entra auth, export, macOS/Linux.
