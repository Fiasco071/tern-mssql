--!strict
-- mssql, window half: palette commands that open the tern-mssql binary in a
-- new tab or split. Saved connections live in the binary's own picker.

local GROUP = "SQL Server"

-- The explorer's share of the tab's width (0.1 to 0.9). Edit it and run
-- `tern plugin reload` to change the default.
local EXPLORER_SHARE = 0.22

local function launch(cx: WindowCx, command: string, how: "tab" | "split")
	if how == "split" then
		local pane = cx.session:focused()
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
		-- The explorer on the left, an idle editor pane on the right that the explorer fills in.
		local tab = cx.layout:tab({
			name = "SQL Server",
			split = "right",
			ratio = EXPLORER_SHARE,
			{ launch = { command = "tern-mssql" } },
			{ launch = { command = "tern-mssql waiting" } },
		})
		-- Focus starts on the explorer (the left leaf), whichever pane Tern picked.
		local layout = if tab then cx.session:layout(tab) else nil
		local left = if layout then layout.root[1] else nil
		if left and left.pane then
			cx.layout:focus(left.pane)
		end
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
