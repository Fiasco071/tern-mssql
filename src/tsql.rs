//! A small T-SQL lexer and the things built on it: extra highlighting for what
//! Tern's SQL grammar does not know (variables, temp tables, `GO`), statement
//! and batch splitting.

use tern_sdk::ui::Decor;

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
	/// A bare word: keyword, name or function.
	Word,
	/// `[bracketed]` or `"quoted"` identifier.
	Quoted,
	/// `'string'`.
	Str,
	/// A number.
	Num,
	/// `-- line` or `/* block */`.
	Comment,
	/// `@var` or `@@system`.
	Var,
	/// `#temp` or `##global`.
	Temp,
	/// Anything else, one char: punctuation and operators.
	Punct,
}

/// A token as a byte range of the text.
#[derive(Clone, Copy, Debug)]
pub struct Tok {
	pub kind:  Kind,
	pub start: usize,
	pub end:   usize,
}

impl Tok {
	/// The token's text.
	pub fn text<'a>(&self, src: &'a str) -> &'a str {
		&src[self.start..self.end]
	}
}

/// Is `c` part of a bare word?
fn word_char(c: char) -> bool {
	c.is_alphanumeric() || matches!(c, '_' | '$' | '#' | '@')
}

/// Splits `src` into tokens, skipping whitespace.
pub fn lex(src: &str) -> Vec<Tok> {
	let mut out = Vec::new();
	let mut it = src.char_indices().peekable();
	while let Some((i, c)) = it.next() {
		let mut end = i + c.len_utf8();
		let kind = match c {
			c if c.is_whitespace() => continue,
			'-' if it.peek().is_some_and(|&(_, d)| d == '-') => {
				for (j, d) in it.by_ref() {
					end = j;
					if d == '\n' {
						break;
					}
					end = j + d.len_utf8();
				}
				Kind::Comment
			},
			'/' if it.peek().is_some_and(|&(_, d)| d == '*') => {
				it.next();
				let mut depth = 1;
				let mut prev = '\0';
				end = src.len();
				while let Some((j, d)) = it.next() {
					if prev == '*' && d == '/' {
						depth -= 1;
						if depth == 0 {
							end = j + 1;
							break;
						}
						prev = '\0';
						continue;
					}
					if prev == '/' && d == '*' {
						depth += 1;
						prev = '\0';
						continue;
					}
					prev = d;
				}
				Kind::Comment
			},
			'\'' => {
				end = src.len();
				while let Some((j, d)) = it.next() {
					if d == '\'' {
						if it.peek().is_some_and(|&(_, e)| e == '\'') {
							it.next();
						} else {
							end = j + 1;
							break;
						}
					}
				}
				Kind::Str
			},
			'[' | '"' => {
				let close = if c == '[' { ']' } else { '"' };
				end = src.len();
				while let Some((j, d)) = it.next() {
					if d == close {
						if it.peek().is_some_and(|&(_, e)| e == close) {
							it.next();
						} else {
							end = j + 1;
							break;
						}
					}
				}
				Kind::Quoted
			},
			'@' => {
				while let Some(&(j, d)) = it.peek() {
					if d == '@' || word_char(d) {
						end = j + d.len_utf8();
						it.next();
					} else {
						break;
					}
				}
				Kind::Var
			},
			'#' => {
				while let Some(&(j, d)) = it.peek() {
					if word_char(d) {
						end = j + d.len_utf8();
						it.next();
					} else {
						break;
					}
				}
				Kind::Temp
			},
			c if c.is_ascii_digit() => {
				while let Some(&(j, d)) = it.peek() {
					if d.is_ascii_alphanumeric() || d == '.' {
						end = j + d.len_utf8();
						it.next();
					} else {
						break;
					}
				}
				Kind::Num
			},
			c if word_char(c) => {
				while let Some(&(j, d)) = it.peek() {
					if word_char(d) {
						end = j + d.len_utf8();
						it.next();
					} else {
						break;
					}
				}
				Kind::Word
			},
			_ => Kind::Punct,
		};
		out.push(Tok { kind, start: i, end });
	}
	out
}

/// Cumulative UTF-16 offsets for ascending byte positions.
struct U16 {
	/// Byte position reached.
	byte: usize,
	/// UTF-16 units up to `byte`.
	units: usize,
}

impl U16 {
	/// The UTF-16 offset of `to`, which must not be before the last call's.
	fn at(&mut self, src: &str, to: usize) -> usize {
		self.units += src[self.byte..to].chars().map(char::len_utf16).sum::<usize>();
		self.byte = to;
		self.units
	}
}

/// Is the token a `GO` batch separator: alone on its line?
fn is_go(src: &str, toks: &[Tok], i: usize) -> bool {
	let t = toks[i];
	if t.kind != Kind::Word || !t.text(src).eq_ignore_ascii_case("go") {
		return false;
	}
	let line_start = src[..t.start].rfind('\n').map_or(0, |n| n + 1);
	let line_end = src[t.end..].find('\n').map_or(src.len(), |n| t.end + n);
	let before = src[line_start..t.start].trim().is_empty();
	let after = src[t.end..line_end].trim();
	before && (after.is_empty() || after.chars().all(|c| c.is_ascii_digit()))
}

/// Highlights what Tern's SQL grammar misses: variables, temp tables, `GO`.
pub fn decorate(src: &str) -> Vec<Decor> {
	let toks = lex(src);
	let mut map = U16 { byte: 0, units: 0 };
	let mut out = Vec::new();
	for (i, t) in toks.iter().enumerate() {
		let s = match t.kind {
			Kind::Var => "info",
			Kind::Temp => "warning",
			Kind::Word if is_go(src, &toks, i) => "accent strong",
			_ => continue,
		};
		let from = map.at(src, t.start);
		let to = map.at(src, t.end);
		out.push(Decor { from, to, s: s.to_owned(), fx: None });
	}
	out
}

/// Splits a script into batches at `GO` lines.
pub fn batches(src: &str) -> Vec<String> {
	let toks = lex(src);
	let mut out = Vec::new();
	let mut from = 0;
	for (i, t) in toks.iter().enumerate() {
		if is_go(src, &toks, i) {
			let line_start = src[..t.start].rfind('\n').map_or(0, |n| n + 1);
			let line_end = src[t.end..].find('\n').map_or(src.len(), |n| t.end + n);
			let chunk = src[from..line_start].trim();
			if !chunk.is_empty() {
				out.push(chunk.to_owned());
			}
			from = line_end;
		}
	}
	let tail = src[from..].trim();
	if !tail.is_empty() {
		out.push(tail.to_owned());
	}
	out
}

/// Whether `src` holds anything SQL Server would do: not just comments, `GO` lines and semicolons.
pub fn has_code(src: &str) -> bool {
	let toks = lex(src);
	toks.iter().enumerate().any(|(i, t)| t.kind != Kind::Comment && !(t.kind == Kind::Punct && t.text(src) == ";") && !is_go(src, &toks, i))
}

/// The trimmed statement around `cursor` (a byte offset): bounded by `;`, `GO` lines and blank lines.
pub fn statement_at(src: &str, cursor: usize) -> &str {
	let (a, b) = statement_range(src, cursor);
	&src[a..b]
}

/// Byte range of the trimmed statement around `cursor`.
pub fn statement_range(src: &str, cursor: usize) -> (usize, usize) {
	let cursor = cursor.min(src.len());
	let toks = lex(src);
	let mut start = 0;
	let mut end = src.len();
	for (i, t) in toks.iter().enumerate() {
		let boundary = (t.kind == Kind::Punct && t.text(src) == ";") || is_go(src, &toks, i);
		if !boundary {
			continue;
		}
		if t.end <= cursor {
			start = t.end;
		} else if t.start >= cursor {
			end = t.start;
			break;
		}
	}
	// Blank lines also end a statement.
	let before = &src[start..cursor];
	if let Some(p) = before.rfind("\n\n").or_else(|| before.rfind("\r\n\r\n")) {
		start += p;
	}
	let after = &src[cursor..end];
	if let Some(p) = after.find("\n\n").or_else(|| after.find("\r\n\r\n")) {
		end = cursor + p;
	}
	let raw = &src[start..end];
	let lead = raw.len() - raw.trim_start().len();
	let trimmed = raw.trim();
	let a = start + lead;
	(a, a + trimmed.len())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_code_counts_as_something_to_run() {
		assert!(has_code("SELECT 1"));
		assert!(has_code("-- note
SELECT 1;"));
		assert!(!has_code(""));
		assert!(!has_code("   
"));
		assert!(!has_code("-- a comment
/* and another */"));
		assert!(!has_code("GO"));
		assert!(!has_code(";
GO
"));
	}

	#[test]
	fn lexes_tsql_pieces() {
		let src = "select [a b], @x, #t, 'it''s' -- hi\nfrom dbo.t";
		let kinds: Vec<Kind> = lex(src).iter().map(|t| t.kind).collect();
		assert!(kinds.contains(&Kind::Quoted) && kinds.contains(&Kind::Var));
		assert!(kinds.contains(&Kind::Temp) && kinds.contains(&Kind::Str));
		assert!(kinds.contains(&Kind::Comment));
	}

	#[test]
	fn splits_batches_on_go() {
		let b = batches("select 1\nGO\nselect 2\ngo 3\nselect 3");
		assert_eq!(b, vec!["select 1", "select 2", "select 3"]);
		assert_eq!(batches("select 'go'"), vec!["select 'go'"]);
	}

	#[test]
	fn finds_the_statement_at_the_caret() {
		let src = "select 1;\nselect 2\nfrom t;\n\nselect 3";
		assert_eq!(statement_at(src, 14), "select 2\nfrom t");
		assert_eq!(statement_at(src, src.len()), "select 3");
	}

	#[test]
	fn decor_uses_utf16_offsets() {
		let d = decorate("select 'é😀', @x");
		assert_eq!(d.len(), 1);
		assert_eq!((d[0].from, d[0].to), (14, 16));
	}
}
