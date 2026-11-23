# The working shell

The shell implements navigation, redirections, streaming pipelines, and native
foreground execution. It is a
small supported subset of Bash, with Windows paths and programs.

## Running commands

Run `cargo run` to start an interactive session, or `cargo run -- -c 'pwd'`
from PowerShell to execute one command or pipeline. A release build produces
`target/release/winsh.exe`.

The dedicated window is available with `cargo build --workspace` followed by
`cargo run -p winsh-terminal`; see [the terminal guide](terminal.md).

~~~sh
pwd
cd crates/winsh/src
cd ..
cd 'C:\Program Files'
cd -
cd ~
rustc --version
exit
~~~

Commands support spaces in quoted arguments, empty quoted arguments, escapes,
comments, and Unicode. Native programs receive arguments individually after
quote removal. For example, `program.exe 'hello world' ''` receives two
arguments, the second empty. Programs with custom Windows command-line parsing
may interpret their argument text differently from programs using the usual
Windows C runtime rules.

Unclosed quotes, trailing backslashes, and a trailing `|` or `|&` request another
input line using the `> ` prompt. On Windows, Ctrl+C cancels pending input or
interrupts foreground pipeline programs without closing winsh. A child can
define its own Ctrl+C behavior.
`exit` leaves the shell; Ctrl+Z provides console end-of-input.

Redirected standard input reads commands without prompts. This is a basic
sequence of command lines, not full script support. Native children inherit
stdin and can consume the bytes after their command line; the input reader
deliberately avoids reading those bytes ahead.

The input loop accepts UTF-8 redirected source and normalizes CRLF to LF.
Interactive Windows input is decoded from the console's UTF-16. Command strings
passed through `-c` are preserved as supplied.

## Built-ins and paths

| Command | Current behavior |
| --- | --- |
| `pwd` | Print the session's logical directory. |
| `pwd -P` | Print its physical, canonical path. |
| `cd directory` | Change the session directory; `-L` is the default. |
| `cd -P directory` | Resolve directory links before processing parent-directory steps. |
| `cd` | Go to HOME, falling back to USERPROFILE on Windows. |
| `cd -` | Switch to OLDPWD and print the new directory. |
| `cd -- -name` | Treat a leading dash as part of the directory name. |
| `exit` | Leave with the last command's status. |
| `exit number` | Leave with a signed integer reduced modulo 256. |

Both directory built-ins accept `-L`, `-P`, and `--`; the last L/P option wins.
Successful cd updates PWD and OLDPWD in the session environment. Failed cd
preserves them. An empty quoted cd operand leaves the directory unchanged;
extra pwd operands are ignored. Directory-link ordering follows the
[Bash cd reference](https://www.gnu.org/s/bash/manual/html_node/Bourne-Shell-Builtins.html).
Physical Windows paths may include the native `\\?\` prefix.

Bare `~` and leading `~/` expand to the home directory. Quoted or escaped tildes
are literal. Named-user forms such as `~carter` are not supported.

Use forward slashes or single-quote backslashes in Windows paths. Unquoted
`C:\temp` follows Bash escaping and becomes `C:temp`. Drive-relative paths such
as `C:folder` are unsupported; use `C:/folder`. There is no Unix mount-path
translation such as mapping `/c` to `C:/`.

## Native programs and I/O

Bare program names are searched in the session's PATH. Relative PATH entries
are relative to the session's current directory. The current directory is
searched only when PATH includes it; otherwise use `./program.exe`. An omitted
`.exe` suffix is supported on Windows, including `./my.tool` for `my.tool.exe`.

Programs inherit the session environment, directory, and standard I/O handles.
Their stdout and stderr stay separate unless explicitly merged, and output is streamed without buffering
the whole result in winsh. Native Windows exit codes retain all 32 bits.

Implicit PATHEXT and file-association dispatch are not implemented. Direct
`.bat`/`.cmd` execution is rejected; invoke `cmd.exe` explicitly when needed.
`dir` and similar commands supplied by cmd are not native executables.
winsh currently supplies only cd, pwd, and exit as built-ins.

| Status | Meaning |
| --- | --- |
| 0 | Successful built-in, or successful child. |
| 1 | Built-in operation, output write, or redirection failure. |
| 2 | Invalid syntax, unsupported expansion, or command usage. |
| 126 | A program was found but could not be executed. |
| 127 | Command not found. |
| 130 | Cancelled input or Windows STATUS_CONTROL_C_EXIT from a child. |

Other native statuses pass through unchanged. A nonzero command status does not
end the input loop. Blank lines and comments preserve the previous status.
Invalid numeric exit arguments terminate with 2; extra arguments after a valid
number report 1 and keep the input loop running. This last behavior currently
also applies to redirected input, unlike Bash's noninteractive exit handling.

## Redirections and pipelines

Redirections apply to built-ins, native programs, and commands inside pipelines.
They are local to that command; the next command gets the session's original
streams. File operands use the same quoting and home-directory expansion as
arguments. Relative paths resolve against the shell's cwd. Quote filenames
containing spaces, and quote or escape literal operator characters.

| Syntax | Behavior |
| --- | --- |
| `<file`, `0<file` | Read stdin from an existing file. |
| `>file`, `1>file` | Create or truncate stdout's file. |
| `>>file`, `1>>file` | Create or append to stdout's file. |
| `2>file`, `2>>file` | Redirect or append stderr. |
| `&>file`, `&>>file` | Redirect or append both output streams. |
| `>&file` | Bash's combined-output shorthand when the operand is not a descriptor. |
| `>\|file` | Overwrite a file; currently equivalent to `>` since noclobber is not implemented. |
| `<>file` | Open stdin for reading and writing, creating the file without truncation. |
| `n>&m`, `n<&m` | Duplicate the current stream `m` onto `n`. |
| `n>&-`, `n<&-` | Close stream `n` for this command. |
| `n>&m-`, `n<&m-` | Duplicate stream `m` onto `n`, then close `m`. |
| `command1 \| command2` | Stream stdout into the next command's stdin. |
| `command1 \|& command2` | Also merge stderr into that output stream. |

Only standard descriptors 0, 1, and 2 are supported. An adjacent unquoted number
selects a descriptor: `2>errors` redirects stderr, while `2 >output` passes `2`
as an argument and redirects stdout. Redirect-only commands such as `>file`
create/truncate files and return 0 on success.

Redirections run from left to right. `program >all 2>&1` puts both outputs in
`all`; `program 2>&1 >out` preserves stderr on the original stdout and sends only
stdout to `out`. A failed open or duplication returns 1 and prevents that command
from running. Earlier file opens can already have created or truncated files.
Earlier stderr redirections also govern subsequent redirection diagnostics.
Closing stderr suppresses diagnostics while preserving the command's status.
For native programs, closed streams are represented by inaccessible handles;
reads or writes fail rather than silently reading from or writing to NUL.

Pipeline connections are installed before explicit redirections. Consequently,
`producer >file | consumer` leaves the consumer at EOF, and `producer | consumer
<file` makes the consumer read its file. `|&` duplicates stderr onto the stage's
resulting stdout after all its explicit redirections, following Bash's ordering.

All pipeline stages run concurrently with OS pipes, and winsh waits for every
stage. Bytes stream directly between processes, including binary data. Every
built-in in a multi-stage pipeline uses an independent copy of the shell session:
`cd subdir | pwd` prints the parent's cwd, and `exit 7 | pwd` leaves the parent
session running. A standalone redirected `cd` or `exit` still affects the parent
session normally.

The pipeline status is the last stage's status, including 126/127 for a failed
last executable or 1 for a last-stage redirection failure. An earlier stage's
failure does not override a successful final stage. Pipefail, pipeline negation
(`!`), and asynchronous pipelines are not implemented. Earlier stages may fail
with a broken pipe if a downstream command exits early; the shell still waits
for every stage. If an engine I/O failure aborts startup or waiting, guards kill
and reap the native stages already started.

A newline after a pipe may be followed by comments and then another command.
A missing redirection operand is a syntax error, not a request for another input
line. The entire pipeline's supported expansions are validated before files are
opened or programs are started, so unsupported expansion syntax cannot cause
partial execution.

These rules follow the Bash references for [redirections](https://www.gnu.org/software/bash/manual/html_node/Redirections.html)
and [pipelines](https://www.gnu.org/software/bash/manual/html_node/Pipelines.html).

## Current limits

Command lists, assignments, variables, substitutions,
wildcards, brace expansion, functions, and control flow are not implemented.
Unsupported constructs produce errors before execution. Detection is deliberately
conservative: unquoted `*`, `?`, `[`, `{`, and `}` are rejected even in some
strings Bash would leave literal. Quote or escape these characters when intended
as ordinary arguments.

Here-documents (`<<`, `<<-`), here-strings (`<<<`), process substitution, named
descriptors (`{fd}>file`), and descriptors above 2 remain unsupported. Here-documents
and here-strings are deferred with scripting and expansion work. There is no
special `/dev/null` or `/dev/fd` path translation; use Windows native paths/devices
such as `NUL` where appropriate.

There is no script-file argument, shebang dispatch, CDPATH lookup, history,
completion, configurable prompt, background jobs, or winsh implementation of
cat/grep/ls yet. Windows Ctrl+C support covers foreground execution; Unix signal
and job-control integration is deferred.

## Following a command through the code

For `cd 'C:\Program Files'`:

1. `main.rs` reads source and asks `syntax` to parse it. The parser returns words
   that still distinguish unquoted, quoted, and escaped pieces.
2. `Shell::execute_pipeline` calls `expand`. Quote-aware construction produces two
   `OsString` values: the command name and one complete path argument.
3. `builtins` recognizes cd. It validates and resolves the path through
   `platform`, then updates the session's directory and environment.
4. The built-in returns `Outcome::Continue(0)`. The shell records that status and
   the CLI displays the next prompt.

For `rustc --version`, step 3 instead goes through `exec`: resolve the executable,
configure a `std::process::Command` with the session state and I/O, then spawn and
wait. Neither path sends the source text through PowerShell or cmd.

The public engine interface consists of:

- `Shell` owns one session's cwd, environment, and last status.
- `ShellIo` owns stdin/stdout/stderr as inherited streams, files, pipe ends, or
  closed streams. `with_stdin`/`with_stdout`/`with_stderr` still accept File handles
  for embedding. The same stream model serves built-ins and children.
- `Outcome` distinguishes `Continue(status)` from `Exit(status)`. Only the CLI
  calls `std::process::exit`, so executing exit cannot terminate a library host.

`Shell::run` parses a pipeline, `execute_pipeline` evaluates an already parsed
pipeline, and `execute` remains available for one `SimpleCommand`. `Shell::clone`
copies session state for isolated evaluation; it does not change host process
state or share mutable session variables.

An engine caller can run:

~~~rust
use winsh::{Outcome, Shell, ShellIo};

fn main() -> std::io::Result<()> {
    let mut shell = Shell::new()?;
    let mut streams = ShellIo::inherited();
    let outcome = shell.run("pwd", &mut streams)?;
    if let Outcome::Exit(status) = outcome {
        println!("The session requested exit with status {status}");
    }
    Ok(())
}
~~~

In Rust terms, `&mut Shell` grants temporary exclusive access to session state.
`Result<Outcome, io::Error>` distinguishes a completed shell command from a host
I/O failure. A missing command is a normal shell outcome with status 127 and a
diagnostic; it is not an engine error that must terminate the host.

`Shell::with_environment` accepts an explicit absolute directory and environment
for embedding and isolated tests. The engine never changes process-wide cwd or
environment. Console handlers belong solely to the CLI; the dedicated terminal
hosts the executable through ConPTY. The shell's source lives in
`crates/winsh/src` and its tests in `crates/winsh/tests`.
