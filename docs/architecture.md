# Architecture

## Scope

winsh is a shell: it parses commands, manages session state, and executes programs.
It currently runs inside an existing Windows console or terminal. A dedicated
terminal is the next milestone; it will host winsh through a Windows
pseudoconsole (ConPTY) without owning the shell's language or execution rules.

Bash syntax and behavior are the compatibility target. Windows filesystem,
process, and console semantics also affect script compatibility, so each
implemented feature needs a stated supported subset and tests. Native execution
does not by itself make every Linux script portable.

## Structure

There is one Cargo package with an executable and an engine library. The public
interface is `syntax`, `Shell`, `ShellIo`, and `Outcome`. Implementation modules
are private. The executable owns arguments, the input loop, console interaction,
and the final process exit; the library owns shell behavior.

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

The flow is input -> syntax tree -> expansion -> built-in or native process.
Interactive input and `-c` share this flow; script input will use it too.
See [the shell walkthrough](shell.md#following-a-command-through-the-code).

When adding the dedicated terminal, turn the repository into a small Cargo
workspace with sibling shell and terminal packages. The terminal should launch
the shell executable through ConPTY and exchange terminal input/output with it.
That boundary also lets the terminal host other shells and lets native programs
use a real console. No terminal renderer or GUI dependencies belong in the shell
engine.

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
- Deny unsafe Rust by default. The few necessary Windows calls live behind safe
  wrappers in `platform` and the CLI's `console` module, with safety comments.

The Windows-only `windows-sys` dependency supplies those API bindings. There is
no shell framework or GUI dependency.

## Implementation order

1. **Implemented:** parse simple command words with Bash quoting and escaping.
   Preserve syntax for later expansion. See [the syntax notes](syntax.md).
2. **Implemented:** shell state, cd/pwd/exit, literal and home-directory
   expansion, native foreground execution, and an interactive input loop.
   See [the shell guide](shell.md) for the exact supported subset.
3. Build a minimal dedicated terminal using ConPTY. Keep its presentation and
   terminal-emulation code in its own package; use it to exercise winsh.
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
