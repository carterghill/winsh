# The working shell

This milestone implements navigation and native foreground execution. It is a
small supported subset of Bash, with Windows paths and programs.

## Running commands

Run `cargo run` to start an interactive session, or `cargo run -- -c 'pwd'`
from PowerShell to execute one command. A release build produces
`target/release/winsh.exe`.

~~~sh
pwd
cd src
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

Unclosed quotes and trailing backslashes request another input line using the
`> ` prompt. On Windows, Ctrl+C cancels pending input or interrupts a foreground
child without closing winsh. A child can define its own Ctrl+C behavior.
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
Their stdout and stderr stay separate, and output is streamed without buffering
the whole result in winsh. Native Windows exit codes retain all 32 bits.

Implicit PATHEXT and file-association dispatch are not implemented. Direct
`.bat`/`.cmd` execution is rejected; invoke `cmd.exe` explicitly when needed.
`dir` and similar commands supplied by cmd are not native executables.
winsh currently supplies only cd, pwd, and exit as built-ins.

| Status | Meaning |
| --- | --- |
| 0 | Successful built-in, or successful child. |
| 1 | Built-in operation failure, such as a missing directory. |
| 2 | Invalid syntax, unsupported expansion, or command usage. |
| 126 | A program was found but could not be executed. |
| 127 | Command not found. |
| 130 | Cancelled input or Windows STATUS_CONTROL_C_EXIT from a child. |

Other native statuses pass through unchanged. A nonzero command status does not
end the input loop. Blank lines and comments preserve the previous status.
Invalid numeric exit arguments terminate with 2; extra arguments after a valid
number report 1 and keep the input loop running. This last behavior currently
also applies to redirected input, unlike Bash's noninteractive exit handling.

## Current limits

Pipes, redirects, command lists, assignments, variables, substitutions,
wildcards, brace expansion, functions, and control flow are not implemented.
Unsupported constructs produce errors before execution. Detection is deliberately
conservative: unquoted `*`, `?`, `[`, `{`, and `}` are rejected even in some
strings Bash would leave literal. Quote or escape these characters when intended
as ordinary arguments.

There is no script-file argument, shebang dispatch, CDPATH lookup, history,
completion, configurable prompt, background jobs, or winsh implementation of
cat/grep/ls yet. Windows Ctrl+C support covers foreground execution; Unix signal
and job-control integration is deferred.

## Following a command through the code

For `cd 'C:\Program Files'`:

1. `main.rs` reads source and asks `syntax` to parse it. The parser returns words
   that still distinguish unquoted, quoted, and escaped pieces.
2. `Shell::execute` calls `expand`. Quote-aware construction produces two
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
- `ShellIo` owns optional File handles for stdin/stdout/stderr; omitted handles
  inherit the host's streams. The same handles serve built-ins and children.
- `Outcome` distinguishes `Continue(status)` from `Exit(status)`. Only the CLI
  calls `std::process::exit`, so executing exit cannot terminate a library host.

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
environment. Console handlers belong solely to the CLI; a future terminal will
host the executable through ConPTY.
