# Architecture

## Scope

winsh is a shell: it parses commands, manages shell state, and executes programs.
It initially runs inside an existing Windows console or terminal. A future
terminal UI should host the shell without owning its language or execution rules.

Bash syntax and behavior are the compatibility target. Windows filesystem,
process, and console semantics also affect script compatibility, so every
implemented feature needs a stated supported subset and tests. Native execution
does not by itself make every Linux script portable.

## Structure

Start with one Cargo package and one executable. As shell behavior arrives, add
`src/lib.rs` in the same package to hold the engine; keep `main.rs` responsible for
arguments, input, diagnostics, and the final process exit code. This lets a later
terminal reuse the engine without introducing a workspace today.

Create modules when their first behavior is implemented, using these boundaries:

| Module | Responsibility |
| --- | --- |
| `syntax` | Tokenization, parsing, syntax trees, and source locations. No filesystem or process access. |
| `expand` | Shell word expansion using quoting information and shell state; filesystem access where globbing requires it. |
| `shell` | Evaluation, working directory, variables, exported environment, and last exit status. |
| `exec` | Command dispatch, redirections, pipelines, and child process lifecycle. |
| `builtins` | Stateful commands such as `cd`, plus compatible utilities as needed. |
| `platform` | Windows-specific process, path, and console behavior that portable Rust cannot express. |

The intended flow is input -> syntax tree -> evaluation and expansion -> built-in
or native process. Interactive input, `-c`, and script files must share this flow.
Keep modules private by default and expose only the engine interface that callers
actually need.

## Rules that keep the implementation small

- Preserve quoting and source locations during parsing. Splitting on whitespace
  or prematurely converting words to strings loses information needed by Bash
  expansion and diagnostics.
- Keep working directory and environment in explicit shell state. Pass them to
  child processes instead of changing process-wide state throughout the engine.
- Launch native programs with Rust's process APIs. Keep any required interpreter
  handling for Windows batch files in the platform boundary; do not translate
  every command into PowerShell or `cmd.exe` text.
- Preserve native paths and arguments with `PathBuf` and `OsString` at OS
  boundaries. Keep Bash escaping rules in the parser; document Windows syntax
  extensions deliberately instead of silently rewriting input.
- Give built-ins explicit access to shell state and input/output streams. Stream
  utility data as bytes so pipelines can handle large files and non-UTF-8 data.
- Use synchronous execution first. Add dependencies for concrete needs, with
  focused APIs; avoid an async runtime, plugin framework, or generic backend
  traits before there is a demonstrated need.
- Unsafe Rust is denied by default. If Windows integration needs it, isolate and
  document a narrowly scoped allowance in `platform` behind a safe interface.

## Implementation order

1. Parse simple commands with Bash quoting and escaping; preserve syntax needed
   for later expansion. Add focused parser tests.
2. Add shell state, `pwd`, `cd`, `exit`, and foreground native process execution.
   Test paths containing spaces, arguments, environment, and exit statuses.
3. Add redirections and pipelines with streaming I/O and correct process cleanup.
4. Add script input and language features incrementally: variables, expansions,
   command lists, control flow, and functions. Use Bash comparison fixtures for
   each supported feature, recording intentional Windows differences.
5. Add compatible utility behavior such as `cat` and `grep` as scripts require
   it, with documented supported options and failure behavior.
6. Improve interactive editing, history, completion, and job control. Consider a
   separate terminal frontend when the shell interface has proved useful.

For each milestone, test behavior at the lowest useful level: pure parser tests,
engine tests with explicit state and I/O, and Windows integration tests for
processes and filesystem behavior. Bash is a development reference, not a runtime
dependency.
