# winsh architecture plan

winsh means **win + sh**, also **Windows shell**. Build a native Windows shell
in Rust with Bash-compatible navigation and, incrementally, scripting. Keep the
codebase small, with clear responsibilities and dependencies added for concrete
needs.

## Scope

winsh is a shell: it parses commands, manages session state, and executes programs.
It runs inside an existing Windows console or terminal, or in the dedicated
`winsh-terminal` window. That terminal hosts winsh through a Windows
pseudoconsole (ConPTY) without owning the shell's language or execution rules.

Bash syntax and behavior are the compatibility target. Windows filesystem,
process, and console semantics also affect script compatibility, so each
implemented feature needs a stated supported subset and tests. Native execution
does not by itself make every Linux script portable.

The eventual scripting goal is to run compatible shell scripts directly in
winsh, with utilities such as cat and grep implemented as needed. Bash is the
behavioral reference; it is not required to execute commands.

## Structure

The Cargo workspace has sibling `winsh` and `winsh-terminal` packages. `winsh`
contains an executable and an engine library. Its public interface is `syntax`,
`Shell`, `ShellIo`, and `Outcome`. Implementation modules are private. The shell
executable owns arguments, the input loop, console interaction, and the final
process exit; the library owns shell behavior. The terminal has no dependency
on the engine and launches the shell executable as a separate process.

| Module | Responsibility |
| --- | --- |
| `syntax` | Parsing, syntax trees, and source locations. No filesystem or process access. |
| `expand` | Quote-aware argument construction: currently literals and home-directory expansion. |
| `shell` | Working directory, inherited environment, last status, and evaluation. |
| `exec` | Executable lookup and foreground child lifecycle. |
| `builtins` | Commands that need session state: cd, pwd, and exit. |
| `platform` | Native paths, exit statuses, and process launch details. |
| `streams` | Explicit standard I/O handles shared by built-ins and children. |
| `console` | Executable-only console input and Windows Ctrl+C handling. |

The command flow is:

~~~text
CLI input (interactive, -c, or eventually a script)
    -> syntax: parse words and preserve quoting
    -> expand: construct arguments using session state
    -> builtins: operate on the session
       OR exec: launch a native program
    -> Outcome: continue or exit, with a status
~~~

Interactive input and `-c` share this flow; script input will use it too.
See [the shell walkthrough](shell.md#following-a-command-through-the-code).

`Shell` owns cwd, environment, and last status. `ShellIo` supplies explicit I/O
handles to built-ins and children. `Outcome` distinguishes a command's exit
status from a request to leave the session. Only the executable exits the host
process; the library returns results.

## Dedicated terminal

The dedicated terminal launches the shell executable through ConPTY and
exchanges terminal input/output with it. That boundary also lets it host other
shells and lets native programs use a real console. Terminal rendering and
window dependencies belong only to `winsh-terminal`.

The layout is:

~~~text
Cargo.toml                 Workspace manifest
crates/
  winsh/                   Shell executable and engine library
  winsh-terminal/          Dedicated terminal executable
docs/                      Shared architecture and compatibility notes
~~~

The terminal package owns the window, rendering, terminal screen state, input
events, resizing, and ConPTY session lifecycle. The shell owns parsing,
expansion, navigation, command execution, and eventually editing and history.

~~~text
Keyboard input -> terminal -> ConPTY -> winsh.exe -> native child
Display        <- terminal <- ConPTY <- shell and child output
~~~

The terminal interprets output into a cell grid, including cursor movement,
colors, and alternate screens. `windows-sys` supplies the native Win32 window,
GDI text renderer, and ConPTY APIs; `vt100` supplies parsing and screen state.
The first milestone provides one window, one session, keyboard and clipboard
input, resizing, scrollback, and shutdown. See [the terminal guide](terminal.md).

Input writes, output reads, and ConPTY control run on separate workers. Output
is parsed directly into bounded screen/scrollback state rather than queued as
an unbounded log. The window thread paints that state and never waits for pipe
I/O. A Windows job object owns the shell's process tree; closing the session
terminates its descendants while output continues draining during ConPTY
teardown. Explicit null startup handles prevent a launching process's redirected
streams from bypassing the pseudoconsole.

## Rules that keep the implementation small

- Preserve quoting and source locations during parsing. Expansion needs to
  distinguish literal text from syntax, even within a single word.
- Keep cwd and environment in explicit shell state. Pass them to child
  processes; never mutate the host process's cwd or environment in the engine.
- Use Rust's process APIs with individual native arguments. Batch files require
  an explicitly invoked interpreter until there is a tested batch policy.
- Use `PathBuf` and `OsString` at OS boundaries. Keep Bash escaping in the
  parser; document Windows syntax extensions deliberately.
- Give built-ins explicit state and I/O. Pass handles to children so large or
  binary output streams directly; avoid collecting it into strings.
- Stay synchronous for foreground execution. Add dependencies for concrete
  needs; avoid an async runtime, plugin framework, or speculative backend traits.
- Deny unsafe Rust by default. Necessary Windows calls live behind safe wrappers
  in the shell's `platform`/`console` and terminal's `session`/`window` modules,
  with safety comments.

The Windows-only `windows-sys` dependency supplies those API bindings. There is
no shell framework, async runtime, or GUI toolkit dependency.

## Implementation order

1. **Implemented:** parse simple command words with Bash quoting and escaping.
   Preserve syntax for later expansion. See [the syntax notes](syntax.md).
2. **Implemented:** shell state, cd/pwd/exit, literal and home-directory
   expansion, native foreground execution, and an interactive input loop.
   See [the shell guide](shell.md) for the exact supported subset.
3. **Implemented:** minimal dedicated terminal using ConPTY, with presentation
   and terminal-emulation code in its own package. See [terminal.md](terminal.md)
   for the supported subset and remaining terminal improvements.
4. Add redirections and pipelines with streaming I/O and correct process cleanup.
5. Add script-file input and language features incrementally: variables,
   expansions, command lists, control flow, and functions. Compare each feature
   against Bash and record intentional Windows differences.
6. Add compatible utilities such as cat and grep as scripts require them, with
   documented supported options and failure behavior.
7. Improve editing, history, completion, and job control as separate shell
   features. Basic input currently uses the host console's line editing.

Test behavior at the lowest useful level: pure parser tests, engine tests with
explicit state and I/O, and Windows integration tests for processes and
filesystem behavior. Bash is a development reference, not a runtime dependency.
