# tern-mssql

SQL Server client for [Tern](https://stencil.so/tern), in Rust: `tern-sdk` for native surfaces, `tiberius` for TDS. One native binary per OS, no runtime.

    tern-mssql [PROFILE] [FILE.sql...]   query window (a profile picker if no PROFILE)
    tern-mssql query PROFILE "SQL"       one batch printed as a table
    tern-mssql profiles                  list saved profiles

With no profile named, the connection manager opens: add, edit, delete, test and connect to saved connections, entered as separate fields or as a connection string. Passwords go to the OS credential store (a password in a pasted connection string is stripped out and stored there).

Profiles live in `%APPDATA%\tern-mssql\profiles.json` (`~/.config/tern-mssql/profiles.json` elsewhere). The file holds no secret; a profile can instead name the env var that holds the password:

    [{ "name": "qas", "server": "host", "user": "sa", "password_env": "QAS_DB_PW", "trust_cert": true }]

## Query window

- Explorer on the left, styled like Tern's file explorer: server, databases (the active one marked), Tables, Views, Programmability, columns under a table with type and key icons. Columns load on expand. Enter or double-click a table to open `SELECT TOP (1000)` in a new tab.
- Query tabs with a full-height buffer, T-SQL highlighting, undo. Multiple result sets, a Messages tab with SSMS-style errors (`Msg 208, Level 16, State 1, Line 3`), and row counts.
- Results are a spreadsheet-style grid: click a cell, shift-click to extend, click a column header or a row number to select the whole column or row, arrows and shift+arrows to move and extend, ctrl+a to select all. Ctrl+c copies the selection as tab-separated text (ctrl+shift+c adds the column headers), so it pastes into Excel as cells. NULL copies as an empty cell. In the grid, s sorts by the selected column (again to reverse), / or ctrl+f filters rows, j and i save the current view as JSON or INSERT statements. Esc returns focus to the editor.
- Script as SELECT, INSERT, UPDATE, DELETE or CREATE from a table or view's menu in the explorer. Script as SELECT runs the statement.
- The editor session autosaves per launch directory and connection and comes back on the next start.
- Completion from the active database's catalog (loaded in the background): tables after `FROM`/`JOIN`, columns after `alias.`, databases after `USE`, schemas, functions, keywords. It shows in the dock above the status line.

Opening any database in the tree loads its objects without changing the editor's database; double-click a database to switch to it.

Keys: F5 runs the script (split at `GO`), ctrl+enter runs the statement at the caret, esc cancels, ctrl+space completes (tab or enter accepts, up/down moves), ctrl+e exports the statement's full result to CSV, ctrl+n/ctrl+w new/close tab, ctrl+s saves, F8 toggles write mode, F6 switches explorer/editor, ctrl+q quits.

Read-only is the default. The guard refuses statements that look like writes; it is a seat belt, not a security boundary, so use a read-only login when that matters.

## Install

`cargo install --path .`, then `tern plugin link plugin`. Palette commands: "SQL Server: Open Tab", "Open Beside Current Pane", "New Connection", "Manage Connections". The status line says `native edit` or `key edit` depending on how the terminal delivers typing.

Built and tested on Windows; macOS arm64 builds and passes tests (not run against a server). CI builds all three OSes. Not yet built, in this order: query history, execution plans, Entra sign-in, a security explorer (users, roles, permissions), SQL Agent jobs, table designer.
