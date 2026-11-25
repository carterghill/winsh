# The working shell

The shell implements scripts, navigation, redirections, streaming pipelines,
background jobs, and foreground job control. It is a supported subset of Bash, with Windows
paths and programs.

## Running commands

Run `cargo run` to start an interactive session, or `cargo run -- -c 'pwd'`
from PowerShell to execute a command string. Run a script with
`cargo run -- examples/greet.sh Carter`. A release build produces
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

Unclosed quotes, trailing operators, and unfinished compound commands request another
input line using the `> ` prompt. On Windows, Ctrl+C cancels pending input or
interrupts the foreground job. At the prompt, `exit` leaves the shell and
Ctrl+Z provides console end-of-input.

Redirected standard input reads commands without prompts. Native children inherit
stdin and can consume the bytes after their command line; the input reader
deliberately avoids reading those bytes ahead.

The input loop accepts UTF-8 redirected source and normalizes CRLF to LF.
Interactive Windows input is decoded from the console's UTF-16. Command strings
passed through `-c` are preserved as supplied.

## Interactive history

At the prompt, press ↑ to recall a command, edit it, and press Enter to run it.
Press ↓ to move forward through history and return to the line you were typing.
Multiline commands are stored as single entries. Winsh skips blank commands,
consecutive duplicates, and input canceled with Ctrl+C.

Winsh saves each submitted command before running it, so closing the terminal
or restarting it with `dev.cmd` keeps the last command. New sessions load up to
1,000 entries from `%LOCALAPPDATA%\winsh\history` on Windows. Set
`WINSH_HISTORY_FILE` in the launching environment to use a different file.
An empty value keeps history in memory for that session. Scripts, `-c`, and
piped input do not add entries. Winsh uses basic console input when output is
redirected or the terminal advertises limited editing support (such as `TERM=dumb`).
History navigation requires the interactive editor.

## Tab completion

Press Tab to complete a command or path at the cursor. If several names match,
the first Tab inserts their shared prefix. Press Tab again to list the choices.
For example, from the repository root, type `cd crat` and press Tab to get
`cd crates/`. Keep typing to complete a path inside that directory.

At the start of a command or pipeline stage, Tab finds built-ins, bundled
utilities, shell functions, and executables on the session's PATH. On Windows
it offers `.exe`, `.com`, and `.sh` files. Use `./` to complete a program in
the current directory. Arguments and file redirections complete paths relative
to the shell's current directory. After `cd`, Tab offers directories.

Completion handles literal paths, including `~/`, quoted names, and spaces.
It leaves variable and command substitutions for execution when you press Enter.

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
The script built-ins are described below.

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

## Background jobs and job control

End a command, pipeline, or `&&`/`||` list with `&` to start it in the background.
The prompt prints the job number and worker PID. For example, try this in winsh
and press Ctrl+Z after `fg` to stop the job and get the prompt back.

~~~sh
powershell.exe -NoProfile -Command 'Start-Sleep 60' &
jobs
fg %1
~~~

| Command or key | Effect |
| --- | --- |
| `jobs` | List job numbers, PIDs, commands, and running, stopped, or completed status. |
| Ctrl+Z in the dedicated terminal | Stop the foreground job, including native pipeline stages and descendants. At the shell prompt this key still signals EOF. |
| `stop %1` | Pause a particular job. |
| `bg %1` | Resume it in the background. |
| `fg %1` | Give it the foreground and wait for it. A stopped job resumes. |
| Ctrl+C during `fg` | Interrupt the foreground job tree and return status 130. |
| `kill %1` | Terminate the job tree. The default Windows exit status is 143. |
| `wait %1` or `wait "$!"` | Collect a job's exit status. With no arguments, wait for the running jobs. |

`fg`, `bg`, and `stop` default to the current job. Use `%+` or `%%` for the
current job, `%-` for the previous one, and `%N` for a numbered job. A bare
number selects a worker PID owned by this shell. `$!` holds the most recent
background PID. `kill` also accepts `-STOP`, `-CONT`, `-INT`, `-TERM`, and
`-KILL`. Windows applies these operations to the whole job object. In other
Windows console hosts, start the command with `&` and use `stop %1` to pause it.

An asynchronous list starts with status 0 and runs with a copy of the launching
shell's directory, variables, functions, and positional arguments. Its changes
stay in that job. `wait` and `fg` return the pipeline or list's final status.
A stop returns 148. The shell reports completion before the next prompt and
retains recent exit statuses until collected. Exiting winsh cleans up its jobs.

On Windows, a job started with `&` at the prompt gets a private input pipe.
The bundled `cat` and `grep` commands also use this pipe when launched directly
to read keyboard input. The shell forwards lines to the pipe while the job is
in the foreground. Ctrl+D on an empty line closes it. In scripts, background
input defaults to EOF. Explicit file and pipe redirections take precedence.
Native Windows programs launched in the foreground can read the console
directly. Resume those keyboard readers with `fg` because they share the
console input buffer with the prompt.

## File utilities

`ls`, `cat`, and `grep` are bundled with winsh. A shell function of the same name
takes precedence. Use an explicit path to run another version. File operands
resolve against the shell's working directory. `cat` and `grep` read stdin when
given no file operands or when an operand is `-`. Put options before operands
and use `--` to end option parsing.

~~~sh
cat -- 'part one.txt' - 'part three.txt' <middle.txt >combined.txt
grep -inE '^(warn|error)' combined.txt
if grep -Fq 'build failed' combined.txt; then echo 'check the build log'; fi
~~~

| Command | Supported behavior |
| --- | --- |
| `ls [-1aAd] [PATH ...]` | List the current directory by default. Print sorted names, one per line. `-A` includes dotfiles and `-a` also includes `.` and `..`. With `-d`, print directory operands themselves. |
| `cat [-bnu] [FILE ...]` | Copy bytes in order, including NULs, CRLF and a partial final line. `-n` numbers all lines, `-b` numbers nonempty lines and overrides `-n`. Numbering continues across files. `-u` requests immediate output, which is already the default. |
| `grep [OPTIONS] PATTERN [FILE ...]` | Select matching lines. The default supports common basic regular expressions, including anchors, dot, bracket classes, star and escaped groups or repetitions. `-E` enables extended operators such as `(a|b)+`. `-F` treats the pattern as literal text. |
| `grep -e PATTERN` | Supply a pattern beginning with a dash or repeat `-e` for alternatives. Newline-separated patterns also select any match. |
| `grep -i -v -n` | Ignore case, invert selection, or prefix each selected line with its 1-based line number. Short flags can be combined. |
| `grep -c -q -H -h` | `-c` prints a selected-line count per file. `-q` stops at the first selected line and suppresses normal output. Multiple files get filename prefixes by default. `-H` forces prefixes and `-h` suppresses them. |

`ls` exits with 0 on success, 1 for an error reading a directory entry, and 2 for an invalid option, inaccessible operand, or output error.
`cat` exits with 0 on success and 1 on error. `grep` exits with 0 if it selects
any line, 1 for no selected lines, and 2 for invalid patterns, options or I/O
errors. Both report unreadable files on stderr and continue with later files.
With `grep -q`, finding a match returns 0 even after an earlier file error.
Streaming a regular file back into itself, including through a hard link,
produces an error. Shell redirections take effect before the utility starts.

`grep` processes one line at a time, treats LF and CRLF as line endings for
matching, preserves selected bytes, and appends LF to a selected unterminated
last line. Patterns are UTF-8 and `-i` uses Unicode case folding. Binary data
uses the same line rules. Backreferences, locale-specific collation, recursion
through directories, and GNU long options are outside this subset.

The interactive Windows shell runs a utility as a foreground child so Ctrl+C
returns to the prompt. PowerShell can invoke the same implementation directly
with `winsh --utility cat FILE` or `winsh --utility grep PATTERN FILE`.

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
If a command closes stderr or redirects it to a read-only file, a failed
diagnostic write preserves the command's status and the session can continue.
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
failure does not override a successful final stage. `! pipeline` negates its
status. Pipefail and asynchronous pipelines are not implemented. Earlier stages may fail
with a broken pipe if a downstream command exits early; the shell still waits
for every stage. If an engine I/O failure aborts startup or waiting, guards kill
and reap the native stages already started.

A newline after a pipe may be followed by comments and then another command.
A missing redirection operand is a syntax error, not a request for another input
line. A complete script is parsed before execution. Expansions occur when each
command is reached, so skipped branches and short-circuited lists have no
expansion side effects. Expansion failures stop the current script evaluation
with status 2; the interactive session can accept another command. Earlier
commands or pipeline stages may already have run.

These rules follow the Bash references for [redirections](https://www.gnu.org/software/bash/manual/html_node/Redirections.html)
and [pipelines](https://www.gnu.org/software/bash/manual/html_node/Pipelines.html).

## Scripts and language

From PowerShell:

~~~powershell
cargo run -- examples/greet.sh Carter 'another name'
cargo run -- -c 'printf "%s\n" "$0" "$@"' command-name first second
~~~

`winsh [--] script [arguments...]` reads a UTF-8 file, accepts an initial BOM,
and normalizes CRLF to LF. A shebang line is a comment; it does not select another
interpreter. `$0` is the supplied filename and `$1` onward are its arguments.
The file is read independently of stdin, so `read` and native children can read
the caller's input. File input and `-c` are parsed completely before execution;
stdin input is parsed one complete command list or compound command at a time.
For `-c source [name [arguments...]]`, name supplies `$0`, defaulting to `winsh`.

Inside winsh, `./script.sh arguments...` and named `.sh` files on PATH run in
an isolated shell initialized from exported environment variables. Their exit,
directory changes, variables, and functions do not modify the caller. Source
files explicitly with `. ./library.sh [arguments...]` or `source ./library.sh`
to share session state. Source filenames resolve against cwd; source does not
search PATH. Supplied source arguments temporarily replace positional arguments;
without arguments it shares the caller's arguments. `return` ends a sourced file.

~~~sh
greet() {
    local name=${1:-world}
    printf 'Hello, %s!\n' "$name"
}
for name in "$@"; do
    greet "$name"
done
~~~

| Feature | Supported forms |
| --- | --- |
| Scalar assignments | `name=value`, `name+=value`, assignment-only commands, and temporary `name=value command` prefixes. Values avoid splitting/globbing. |
| Exported environment | Inherited environment entries are exported; `export name[=value]` exports a scalar, and `unset name` removes it. Unexported variables stay in the shell. |
| Parameters | `$name`, `${name}`, `$0`…`$9`, `${10}` onward, `$?`, `$#`, `$$`, `$@`, and `$*`. |
| Parameter operators | `${name-word}`, `${name:-word}`, `+`, `:+`, `=`, `:=`, `?`, `:?`, and `${#name}`. Default words expand lazily. Quote the entire expansion when preserving spaces. |
| Command substitution | `$(commands)` runs in an isolated session, captures UTF-8 stdout, and removes trailing LF bytes. Stderr stays on the current stderr. An assignment-only command returns the last substitution's status. |
| Integer expansion | `$((expression))` with decimal/octal/hex integers, integer-valued scalar names, parentheses, unary `+ - ! ~`, arithmetic `+ - * / %`, shifts, comparisons, bitwise operators, and short-circuit `&&` / `||`. Arithmetic wraps at 64 bits; division errors return 2. |
| Field splitting | Unquoted parameter/substitution results split using IFS (default space/tab/LF); quoted results preserve empty arguments and spaces. `"$@"` preserves separate positional arguments. |
| Pathname patterns | Unquoted `*`, `?`, and bracket/range/negated classes. Matches are sorted, exclude leading-dot names unless explicitly requested, and leave unmatched patterns literal. Use slash path separators. |
| Lists | Newlines, `;`, `&&`, `||`, and pipeline negation `!`. AND/OR lists evaluate left to right and skip expansions in commands they skip. |
| Branches | `if commands; then commands; elif commands; then commands; else commands; fi`. |
| Loops | `for name in words; do commands; done`, `for name; do commands; done` over positional arguments, and `while` / `until commands; do commands; done`. |
| Loop control | `break [positive-count]`, `continue [positive-count]`; counts can unwind nested loops. |
| Groups | `{ commands; }` shares state; `(commands)` isolates state. Groups and compound commands can have redirections or be pipeline stages. |
| Functions | `name() { commands; }`, arguments, recursion, `local name[=value]`, and `return [status]`. Functions share session state, with temporary arguments and dynamically scoped local variables. |

Pipeline functions and compound commands use isolated session copies, like
pipeline built-ins. Function definitions take effect when evaluated; their
redirects apply when called. Parsing/recursion limits reject excessive nesting
instead of allowing unbounded recursive evaluation.

| Additional built-in | Supported behavior |
| --- | --- |
| `:` / `true` / `false` | Return success, success, or failure. |
| `echo [-n] [words...]` | Join words with spaces; `-n` suppresses the newline. Use printf for escapes. |
| `printf format [values...]` | Repeating format with `%s`, `%b`, `%d`, `%i`, `%u`, `%x`, `%X`, `%o`, `%%`; basic backslash escapes including `\n`, `\t`, and `\\`. Width/precision and other formats are unsupported. |
| `test` / `[` | Empty/nonempty strings; `-n`, `-z`, `-e`, `-f`, `-d`, `-s`; string `=`, `==`, `!=`, `<`, `>`; integer `-eq`, `-ne`, `-lt`, `-le`, `-gt`, `-ge`; leading `!`. `[` requires `]`. Combine tests with shell `&&` / `||`. Quote `<` and `>`. |
| `read [-r] [names...]` | Read one UTF-8 LF-terminated line without reading ahead. With no names, set REPLY; otherwise split using IFS, with the final name receiving remaining fields. `-r` preserves backslashes. EOF returns 1, including a final partial line. |
| `set -- [arguments...]` | Replace positional arguments. |
| `shift [count]` | Drop positional arguments; default one. |
| `unset [-v|-f] names...` | Remove variables or functions. |

Command substitutions collect their text result while concurrently draining the
pipe, so output larger than a pipe buffer does not deadlock. Unlike binary
pipelines, substitution output must be valid UTF-8 without NUL bytes; invalid
text is an expansion error. Bash may instead retain arbitrary bytes or discard
NUL bytes. Script source and read input are also UTF-8.

The comparison fixtures use the [Bash reference manual](https://www.gnu.org/software/bash/manual/bash.html)
and a locally installed Bash as the behavioral reference, with no Bash runtime
dependency. Windows exported names are case-insensitive, while unexported scalar
names are case-sensitive. Native exit codes retain 32 bits. Filename matches use
case-sensitive pattern comparisons and Rust string ordering, rather than Bash
locale collation. These differences can affect cross-platform scripts.

## Current limits

This is not a full Bash implementation. Arrays, associative arrays, `case`,
`select`, `[[ ... ]]`, arithmetic commands/for loops, arithmetic assignment,
increment/decrement, exponentiation, ternary/comma operators, recursive variable
expressions in arithmetic, pattern-removal/replacement/substring parameter
operators, indirect expansion, brace expansion, ANSI-C quoting, and backticks
remain pending. The `function name` spelling is unsupported; use `name() { ...; }`.
Advanced quoting within parameter default words, combined whitespace/nonwhitespace
IFS edge cases, and escaped IFS characters in non-raw read are not fully Bash-compatible.
`return` accepts a nonnegative status; shell options (`set -e`, `-u`, pipefail,
shopt), readonly variables, aliases, traps, and POSIX special-built-in error rules
are not implemented. Unsupported built-in flags/forms report errors.

Here-documents (`<<`, `<<-`), here-strings (`<<<`), process substitution, named
descriptors (`{fd}>file`), and descriptors above 2 remain unsupported. There is no
special `/dev/null` or `/dev/fd` path translation; use Windows native paths/devices
such as `NUL` where appropriate.

Arbitrary shebang dispatch, CDPATH lookup, configurable prompts, `disown`, and
shell signal traps remain outside the supported subset.

## Following a command through the code

For `cd 'C:\Program Files'`:

1. `main.rs` reads source and asks `syntax` to parse it. The parser returns words
   that still distinguish unquoted, quoted, and escaped pieces.
2. `Shell::execute_script` evaluates lists and calls `expand`. Quote-aware construction produces two
   `OsString` values: the command name and one complete path argument.
3. `builtins` recognizes cd. It validates and resolves the path through
   `platform`, then updates the session's directory and environment.
4. The built-in returns `Outcome::Continue(0)`. The shell records that status and
   the CLI displays the next prompt.

For `rustc --version`, step 3 instead goes through `exec`: resolve the executable,
configure a `std::process::Command` with the session state and I/O, then spawn and
wait. Neither path sends the source text through PowerShell or cmd.

The public engine interface consists of:

- `Shell` owns one session's cwd, variables, exported environment, arguments,
  functions, and last status.
- `ShellIo` owns stdin/stdout/stderr as inherited streams, files, pipe ends, or
  closed streams. `with_stdin`/`with_stdout`/`with_stderr` still accept File handles
  for embedding. The same stream model serves built-ins and children.
- `Outcome` distinguishes `Continue(status)` from `Exit(status)`. Only the CLI
  calls `std::process::exit`, so executing exit cannot terminate a library host.
- `Utility` runs bundled file utilities with explicit session paths and I/O.

`Shell::run` parses a script, `execute_script` evaluates an already parsed script,
`execute_pipeline` accepts the earlier simple-pipeline AST, and `execute` remains
available for one `SimpleCommand`. `set_arguments` supplies `$0` and positional
arguments to embedded sessions. `Shell::clone`
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
