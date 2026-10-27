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
