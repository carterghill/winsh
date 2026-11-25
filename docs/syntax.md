# Script parsing

`winsh::syntax::parse_script(&str)` returns `Result<Script, ParseError>` and is
the CLI/engine entry point. Parsing preserves quote context and UTF-8 byte ranges,
performs no I/O, and never expands or executes words.

The tree is:

~~~text
Script
  AndOr list                 separated by newline / semicolon
    ScriptPipeline           connected by && / ||
      Command                connected by | / |&; optional leading !
        SimpleCommand        words and ordered redirections
        Group                { list; } or (list)
        If                   condition/body pairs and optional else
        For / While          loop words/conditions and body
        Function             name and deferred compound body
~~~

The parser recognizes `if`/`elif`/`else`/`fi`, `for`/`in`/`do`/`done`,
`while`/`until`, groups, and `name() { ...; }` in command position. Quoted words
with reserved-word spelling remain ordinary words. The evaluator identifies
assignment prefixes from unquoted leading `name=` text; the parser retains them
as words. Compound commands may have trailing redirections and appear in pipelines.

The earlier APIs remain available:

- `parse_simple_command` returns `Result<Option<SimpleCommand>, ParseError>`.
- `parse_pipeline` returns `Result<Option<Pipeline>, ParseError>` for one
  pipeline of simple commands. A `Pipeline` has one `PipeKind` per connection.

These restricted inspection APIs return `None` for blank/comment-only input and
reject multiple commands. Use `parse_script` for contextual reserved-word grammar
and compound commands. They are syntax APIs, not execution APIs or general Bash
validators. A command can consist entirely of redirections.

## Words and source ranges

Words support single quotes, double quotes, adjacent quoted/unquoted text, empty
quotes, and Bash backslash escaping. Backslash-LF continues a line outside single
quotes, including within operators, keywords, and parameter names. `#` begins a
comment only at the beginning of an unquoted word; comments end at physical LF.

For `echo pre"$HOME\$USER"' literal'`, the second word contains:

~~~text
Text("pre")
DoubleQuoted([
    Text("$HOME"),
    Escaped('$'),
    Text("USER"),
])
SingleQuoted(" literal")
~~~

Simple `$name` references stay in text for later expansion. `${expression}`,
`$(commands)`, and `$((expression))` use `Parameter`, `CommandSubstitution`, and
`Arithmetic` word parts. Command substitutions are syntax-checked recursively
without execution. Tildes and pathname patterns also remain unevaluated.

Every word and part retains a half-open byte range into the original UTF-8
source, including quotes, escapes, and removed continuations. Adjacent text
across a continuation joins while retaining its original span. Double-quote
groups stay structural, including empty groups.

Redirections retain source order, optional descriptor, operator kind, target
word, and complete source span. Supported operators are `<`, `>`, `>>`, `<>`,
`>|`, `<&`, `>&`, `&>`, and `&>>`, limited to descriptors 0, 1, and 2. The engine
interprets descriptor duplication/moves/closing after expansion.

These rules follow the Bash references for
[quoting](https://www.gnu.org/software/bash/manual/html_node/Quoting.html),
[lists](https://www.gnu.org/software/bash/manual/html_node/Lists.html), and
[compound commands](https://www.gnu.org/software/bash/manual/html_node/Compound-Commands.html).
See [the language guide](shell.md#scripts-and-language) for the exact expansion
and execution subset.

## Incomplete input and limits

Unclosed quotes, trailing escapes/operators, missing closing expansion delimiters,
and unfinished compound commands return errors with `is_incomplete() == true`.
The input loop requests another line; complete file/command-string input reports
status 2. Missing redirection operands are syntax errors rather than continuation
requests. NUL bytes are rejected. Excessive compound/expansion nesting is rejected.

A single `&` terminates an asynchronous AND/OR list, including its pipelines.
The next command can begin on the same line. Quoting or escaping `&` keeps it
in a word. The operators `&&`, `|&`, `&>`, and `>&` keep their usual meanings.

Here-documents/strings, process substitution, `case`, `select`,
`[[ ... ]]`, `function` spelling, arrays, brace expansion, backticks, ANSI-C
quoting, and descriptors above 2 are outside the current grammar. Unsupported
features report errors where recognized; this remains a Bash subset.

The parser does not normalize CRLF: CR stays in word content and LF separates
commands. The CLI normalizes CRLF for file/stdin source and accepts a file's initial
UTF-8 BOM. `-c` and the parser API preserve their supplied source.

Windows paths use Bash quoting: `'C:\Program Files\winsh'` or `C:/Users/Carter`
preserve path characters; unquoted `C:\temp` becomes `C:temp`.

## Tests

~~~powershell
cargo test --locked --offline --workspace --all-targets
~~~

Tests check literal quoting, quote/escape structure, byte ranges, empty words,
continuations, nesting, unsupported/incomplete syntax, session behavior, scripts,
and native execution. Bounded exhaustive corpora check that short combinations
of syntax characters and Unicode do not panic and retain valid ranges.

Optional comparisons against a local Bash installation:

~~~powershell
$env:WINSH_TEST_BASH = 'C:\Program Files\Git\bin\bash.exe'
cargo test --test syntax literal_quoting_matches_bash -- --ignored --exact
cargo test --test shell redirections_and_pipelines_match_bash -- --ignored --exact
cargo test --test language scripts_and_language_match_bash -- --ignored --exact
~~~

These feed fixed fixtures to Bash and compare statuses, output streams, native
arguments, and redirection files. They are opt-in; running winsh needs no Bash.
The language comparison covers scalar/default/positional expansion, lists,
branches, loops, functions/local state, substitutions, arithmetic, grouped
pipelines, read, and exit behavior. On Windows the quoting comparison excludes
raw-CR fixtures because the Bash port discards them while reading source; the
Rust parser tests still cover them.
