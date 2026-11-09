//! The system clipboard: the Windows API directly, and the platform's own tool
//! elsewhere. Tern does not honor the OSC 52 write escape.

#[cfg(not(windows))]
use std::{
	io::Write,
	process::{Command, Stdio},
};

#[cfg(not(windows))]
use anyhow::{Context, bail};

/// Pipes `bytes` into `cmd`'s stdin.
#[cfg(not(windows))]
fn pipe(mut cmd: Command, bytes: &[u8]) -> anyhow::Result<()> {
	let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
	child.stdin.take().context("no stdin")?.write_all(bytes)?;
	if !child.wait()?.success() {
		bail!("{:?} failed", cmd.get_program());
	}
	Ok(())
}

/// Runs `cmd` and returns what it prints.
#[cfg(not(windows))]
fn read(mut cmd: Command) -> anyhow::Result<String> {
	let out = cmd.stdin(Stdio::null()).stderr(Stdio::null()).output()?;
	if !out.status.success() {
		bail!("{:?} failed", cmd.get_program());
	}
	Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The clipboard's text, empty when it holds no text.
pub fn get() -> anyhow::Result<String> {
	#[cfg(windows)]
	{
		if !clipboard_win::raw::is_format_avail(clipboard_win::formats::CF_UNICODETEXT) {
			return Ok(String::new());
		}
		clipboard_win::get_clipboard_string().map_err(|e| anyhow::anyhow!("{e}"))
	}
	#[cfg(target_os = "macos")]
	{
		read(Command::new("pbpaste"))
	}
	#[cfg(all(unix, not(target_os = "macos")))]
	{
		let candidates: [(&str, &[&str]); 3] =
			[("wl-paste", &["--no-newline"]), ("xclip", &["-selection", "clipboard", "-o"]), ("xsel", &["--clipboard", "--output"])];
		for (prog, args) in candidates {
			let mut cmd = Command::new(prog);
			cmd.args(args);
			if let Ok(text) = read(cmd) {
				return Ok(text);
			}
		}
		bail!("no clipboard tool found (install wl-clipboard, xclip or xsel)")
	}
}

/// Puts `text` on the clipboard.
pub fn set(text: &str) -> anyhow::Result<()> {
	#[cfg(windows)]
	{
		clipboard_win::set_clipboard_string(text).map_err(|e| anyhow::anyhow!("{e}"))
	}
	#[cfg(target_os = "macos")]
	{
		pipe(Command::new("pbcopy"), text.as_bytes())
	}
	#[cfg(all(unix, not(target_os = "macos")))]
	{
		let candidates: [(&str, &[&str]); 3] =
			[("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])];
		for (prog, args) in candidates {
			let mut cmd = Command::new(prog);
			cmd.args(args);
			if pipe(cmd, text.as_bytes()).is_ok() {
				return Ok(());
			}
		}
		bail!("no clipboard tool found (install wl-clipboard, xclip or xsel)")
	}
}
