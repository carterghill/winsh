# Simple-command parsing

`winsh::syntax::parse_simple_command(&str)` returns
`Result<Option<SimpleCommand>, ParseError>`. Blank or comment-only input produces
`None`; a command contains words, and each word contains its original quoted and
escaped pieces. This is a syntax API, not an argument-expansion or execution API.

The implementation is a single forward scan with no intermediate token list or
external dependencies. The parser is private; syntax types and the parsing
function are the public library interface. It performs no I/O.

## Supported input

- One command, separated into words by unquoted spaces and tabs. Newlines end the
  command; leading/trailing blank lines and comments are allowed.
- Single quotes, double quotes, and adjacent quoted/unquoted text in one word.
  Empty quoted arguments are retained.
- Backslash escapes outside quotes. In double quotes, backslash escapes `$`,
  backtick, double quote, and backslash; it stays literal before other characters.
- Backslash followed by LF continues a line outside single quotes. It does not
  create a word or split a word, including a variable name split over two lines.
- `#` starts a comment at the beginning of an unquoted word. A `#` embedded in a
  word, quoted, or escaped is ordinary text. Comments end at the next physical LF.
- UTF-8 source, including non-ASCII filenames. All source ranges are half-open
  byte offsets into the original input, including quote and escape syntax.

These rules follow the Bash reference sections on
[escapes](https://www.gnu.org/s/bash/manual/html_node/Escape-Character.html),
[single quotes](https://www.gnu.org/s/bash/manual/html_node/Single-Quotes.html),
[double quotes](https://www.gnu.org/s/bash/manual/html_node/Double-Quotes.html), and
[comments](https://www.gnu.org/s/bash/manual/html_node/Comments.html).

## Preserved information

For `echo pre"$HOME\$USER"' literal'`, the second word contains:

```text
Text("pre")
DoubleQuoted([
    Text("$HOME"),
    Escaped('$'),
    Text("USER"),
])
SingleQuoted(" literal")
```

The `$HOME` text is available for later expansion while the escaped dollar sign
and single-quoted text retain their literal meaning. `~`, glob patterns, simple
parameter references, and brace patterns are preserved without evaluation.
Adjacent text across a removed continuation is joined, with its source range
still covering the original continuation. Quote groups remain separate, even
when empty. The shell's separate expansion stage constructs executable arguments
from this structure, keeping the parser independent of shell state.

## Current limits

Operators (including pipes, redirects, and command separators), command
substitution, braced parameter expansion, arithmetic expansion, and dollar-prefixed
quoting return explicit unsupported-syntax errors. They require more grammar and
must not be treated as literal arguments by accident. Ordinary assignment prefixes
and reserved words in command position are also rejected. This first subset is
not a general Bash syntax validator; array assignments, contextual reserved-word
rules, aliases, history expansion, and compound commands remain future work.

Unclosed quotes and a trailing backslash return an error for which
`is_incomplete()` is true, so the interactive reader can request more input.
The trailing-backslash rule is a streaming-input convention: Bash may accept it
literally at the end of a completed script. NUL bytes are rejected. The parser
does not normalize CRLF: CR remains word content, and LF is the line separator.
The CLI input loop normalizes CRLF to LF, including within multiline quoted
input. The parser API and `-c` command text preserve the supplied source.

The [working shell](shell.md) evaluates literal words and leading `~`/`~/`.
Parameter, wildcard, and brace expansion markers preserved by this parser
currently produce explicit errors during expansion. Quoted or escaped literal
markers remain usable.

Windows paths follow the same quoting rules. Use `'C:\Program Files\winsh'` or
`C:/Users/Carter` to preserve path characters. An unquoted `C:\temp` becomes
`C:temp` under Bash escaping; the parser does not silently invent Windows rules.

## Tests

```powershell
cargo test --locked --offline --workspace --all-targets
```

The suite checks literal quoting fixtures, quote/escape structure, byte ranges,
empty words, continuations, unsupported syntax, and incomplete input. A bounded
exhaustive corpus checks that parsing short combinations of syntax characters and
Unicode never panics and always produces valid source ranges.

To additionally compare literal arguments with Bash (Git Bash example path):

```powershell
$env:WINSH_TEST_BASH = 'C:\Program Files\Git\bin\bash.exe'
cargo test --test syntax literal_quoting_matches_bash -- --ignored --exact
```

This test feeds only fixed fixtures to Bash and compares NUL-delimited output,
including empty arguments. It is opt-in so ordinary development needs only Rust.
On Windows, raw-CR fixtures stay in the Rust tests but are excluded from the Bash
comparison because the Windows Bash port used during development discards them
while reading source. Running the comparison on Linux includes those fixtures.
