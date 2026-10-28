//! The operating system's own Save dialog, opened as a separate process (the
//! Tern SDK has none). Windows uses the Windows Forms SaveFileDialog through
//! PowerShell, macOS uses `osascript`, Linux uses `zenity` or `kdialog`.

use std::path::{Path, PathBuf};

/// What the dialog ended with.
#[derive(Debug)]
pub enum Outcome {
	Chosen(PathBuf),
	Cancelled,
	/// No dialog could be opened.
	Unavailable,
}

/// Whether a native dialog can be tried on this machine.
pub fn available() -> bool {
	if cfg!(windows) || cfg!(target_os = "macos") {
		return true;
	}
	["zenity", "kdialog"].iter().any(|t| std::process::Command::new(t).arg("--version").output().is_ok())
}

/// Blocks until the user picks a file or cancels. Run it on a thread.
pub fn save_as(dir: &Path, name: &str) -> Outcome {
	run(dir, name)
}

/// The text of a finished dialog process: its stdout when it succeeded.
fn finish(out: std::io::Result<std::process::Output>) -> Outcome {
	match out {
		Err(_) => Outcome::Unavailable,
		Ok(o) if o.status.success() => {
			let text = String::from_utf8_lossy(&o.stdout).trim().to_owned();
			if text.is_empty() { Outcome::Cancelled } else { Outcome::Chosen(PathBuf::from(text)) }
		},
		Ok(_) => Outcome::Cancelled,
	}
}

#[cfg(windows)]
fn run(dir: &Path, name: &str) -> Outcome {
	use std::os::windows::process::CommandExt;

	// Single quotes only, so the script survives being one argument.
	let script = "Add-Type -AssemblyName System.Windows.Forms; \
		$f = New-Object System.Windows.Forms.Form; $f.TopMost = $true; $f.ShowInTaskbar = $false; $f.Opacity = 0; \
		$f.StartPosition = 'CenterScreen'; $f.Show(); \
		$d = New-Object System.Windows.Forms.SaveFileDialog; $d.Title = 'Save query as'; \
		$d.InitialDirectory = $env:TM_DIR; $d.FileName = $env:TM_NAME; \
		$d.Filter = 'SQL files (*.sql)|*.sql|All files (*.*)|*.*'; $d.DefaultExt = 'sql'; $d.AddExtension = $true; $d.OverwritePrompt = $true; \
		if ($d.ShowDialog($f) -eq 'OK') { [Console]::Out.Write($d.FileName) }; $f.Close()";
	finish(
		std::process::Command::new("powershell.exe")
			.args(["-NoProfile", "-STA", "-Command", script])
			.env("TM_DIR", dir)
			.env("TM_NAME", name)
			.stdin(std::process::Stdio::null())
			.creation_flags(0x0800_0000)
			.output(),
	)
}

#[cfg(target_os = "macos")]
fn run(dir: &Path, name: &str) -> Outcome {
	finish(
		std::process::Command::new("osascript")
			.args([
				"-e",
				"on run argv",
				"-e",
				"POSIX path of (choose file name with prompt \"Save query as\" default name (item 2 of argv) default location (POSIX file (item 1 of argv)))",
				"-e",
				"end run",
			])
			.arg(dir)
			.arg(name)
			.stdin(std::process::Stdio::null())
			.output(),
	)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run(dir: &Path, name: &str) -> Outcome {
	let start = dir.join(name);
	let zenity = std::process::Command::new("zenity")
		.args(["--file-selection", "--save", "--confirm-overwrite", "--title", "Save query as", "--filename"])
		.arg(&start)
		.stdin(std::process::Stdio::null())
		.output();
	if zenity.is_ok() {
		return finish(zenity);
	}
	finish(std::process::Command::new("kdialog").args(["--getsavefilename"]).arg(&start).arg("*.sql").stdin(std::process::Stdio::null()).output())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn empty_output_is_a_cancel() {
		let ok = |stdout: &str| {
			#[cfg(windows)]
			let status = {
				use std::os::windows::process::ExitStatusExt;
				std::process::ExitStatus::from_raw(0)
			};
			#[cfg(unix)]
			let status = {
				use std::os::unix::process::ExitStatusExt;
				std::process::ExitStatus::from_raw(0)
			};
			Ok(std::process::Output { status, stdout: stdout.as_bytes().to_vec(), stderr: Vec::new() })
		};
		assert!(matches!(finish(ok("")), Outcome::Cancelled));
		assert!(matches!(finish(ok("C:\\work\\a.sql\r\n")), Outcome::Chosen(p) if p == PathBuf::from("C:\\work\\a.sql")));
	}
}
