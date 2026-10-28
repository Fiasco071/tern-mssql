--!strict
-- mssql, window half: palette commands that open the tern-mssql binary in a
-- new tab or split. Saved connections live in the binary's own picker.

local GROUP = "SQL Server"

local function launch(cx: WindowCx, command: string, how: "tab" | "split")
	if how == "split" then
		local pane = cx.layout:focused()
		if pane then
			cx.layout:split(pane, "right", { command = command })
			return
		end
	end
	cx.layout:new_tab({ command = command })
end

tern.command({
	id = "open",
	title = "SQL Server: Open Tab",
	icon = "database",
	group = GROUP,
	run = function(cx)
		launch(cx, "tern-mssql", "tab")
	end,
})

tern.command({
	id = "open_split",
	title = "SQL Server: Open Beside Current Pane",
	icon = "database",
	group = GROUP,
	run = function(cx)
		launch(cx, "tern-mssql", "split")
	end,
})

tern.command({
	id = "new_connection",
	title = "SQL Server: New Connection",
	icon = "plus",
	group = GROUP,
	run = function(cx)
		launch(cx, "tern-mssql --new", "tab")
	end,
})

tern.command({
	id = "manage_connections",
	title = "SQL Server: Manage Connections",
	icon = "settings",
	group = GROUP,
	run = function(cx)
		launch(cx, "tern-mssql", "tab")
	end,
})
