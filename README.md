# tern-mssql

SQL Server client for [Tern](https://stencil.so/tern), in Rust: `tern-sdk` for native surfaces, `tiberius` for TDS. One native binary per OS, no runtime.

    tern-mssql [PROFILE] [FILE.sql...]   query window (a profile picker if no PROFILE)
    tern-mssql query PROFILE "SQL"       one batch printed as a table
    tern-mssql profiles                  list saved profiles

Profiles live in `%APPDATA%\tern-mssql\profiles.json` (`~/.config/tern-mssql/profiles.json` elsewhere). A profile names the env var that holds the password, so the file holds no secret:

    [{ "name": "qas", "server": "host", "user": "sa", "password_env": "QAS_DB_PW", "trust_cert": true }]

## Query window

- Explorer on the left, styled like Tern's file explorer: server, databases (the active one marked), Tables, Views, Programmability, columns under a table with type and key icons. Columns load on expand. Enter or double-click a table to open `SELECT TOP (1000)` in a new tab.
- Query tabs with a full-height buffer, T-SQL highlighting, undo. Multiple result sets, a Messages tab with SSMS-style errors (`Msg 208, Level 16, State 1, Line 3`), and row counts.
- Completion from the active database's catalog (loaded in the background): tables after `FROM`/`JOIN`, columns after `alias.`, databases after `USE`, schemas, functions, keywords. It shows in the dock above the status line.

Keys: F5 runs the script (split at `GO`), ctrl+enter runs the statement at the caret, esc cancels, ctrl+space completes (tab or enter accepts, up/down moves), ctrl+e exports the statement's full result to CSV, ctrl+n/ctrl+w new/close tab, ctrl+s saves, F8 toggles write mode, F6 switches explorer/editor, ctrl+q quits.

Read-only is the default. The guard refuses statements that look like writes; it is a seat belt, not a security boundary, so use a read-only login when that matters.

## Install

`cargo install --path .`, then `tern plugin link plugin`; palette command "SQL Server: Open Tab". The status line says `native edit` or `key edit` depending on how the terminal delivers typing.

Built and tested on Windows; macOS arm64 builds and passes tests (not run against a server). CI builds all three OSes. Not yet built: Entra auth, query history, results sort/filter/copy, execution plans, Script as, table designer.
