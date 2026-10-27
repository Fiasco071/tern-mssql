--!strict
-- mssql, window half: a palette command that opens the tern-mssql binary in a
-- new tab. The connection string comes from TERN_MSSQL_CONN in the shell.

tern.command({
	id = "open",
	title = "SQL Server: Open Tab",
	icon = "database",
	run = function(cx)
		cx.layout:new_tab({ command = "tern-mssql" })
	end,
})
