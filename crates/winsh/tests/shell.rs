use std::{
    env,
    ffi::OsString,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    },
};

use winsh::{Outcome, Shell, ShellIo};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let root = std::path::absolute(env!("CARGO_TARGET_TMPDIR"))
            .unwrap()
            .join("shell-tests");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let root = std::path::absolute(env!("CARGO_TARGET_TMPDIR"))
            .unwrap()
            .join("shell-tests");
        assert!(self.0.is_absolute() && self.0.parent() == Some(root.as_path()));
        // Only this uniquely created test directory may be recursively removed.
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    shell: Shell,
    io: ShellIo,
    output: PathBuf,
    error: PathBuf,
    root: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new();
        let output = root.0.join("stdout");
        let error = root.0.join("stderr");
        let shell = Shell::with_environment(root.0.clone(), env::vars_os()).unwrap();
        let io = ShellIo::inherited()
            .with_stdout(File::create(&output).unwrap())
            .with_stderr(File::create(&error).unwrap());
        Self {
            shell,
            io,
            output,
            error,
            root,
        }
    }

    fn run(&mut self, source: &str) -> Outcome {
        self.shell.run(source, &mut self.io).unwrap()
    }

    fn out(&self) -> String {
        fs::read_to_string(&self.output).unwrap()
    }

    fn err(&self) -> String {
        fs::read_to_string(&self.error).unwrap()
    }
}

fn quote(value: impl AsRef<std::ffi::OsStr>) -> String {
    format!(
        "'{}'",
        value.as_ref().to_str().unwrap().replace('\'', "'\\''")
    )
}

fn helper() -> &'static Path {
    static HELPER: OnceLock<PathBuf> = OnceLock::new();
    HELPER.get_or_init(|| {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("test-support");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "child-{}{}",
            std::process::id(),
            env::consts::EXE_SUFFIX
        ));
        let output = Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg("--edition=2024")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/child.rs"))
            .arg("-o")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        path
    })
}

fn cli(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_winsh"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn directory_changes_preserve_session_state_without_changing_the_host() {
    let host = env::current_dir().unwrap();
    let mut f = Fixture::new();
    let sub = f.root.0.join("space café 🦀");
    fs::create_dir(&sub).unwrap();
    assert_eq!(f.run("cd 'space café 🦀'"), Outcome::Continue(0));
    assert_eq!(f.shell.cwd(), sub);
    assert_eq!(env::current_dir().unwrap(), host);
    assert_eq!(f.run("pwd"), Outcome::Continue(0));
    assert_eq!(f.out().trim_end(), sub.to_str().unwrap());
    assert_eq!(f.run("cd -"), Outcome::Continue(0));
    assert_eq!(f.shell.cwd(), f.root.0);
    assert_eq!(f.run("cd -"), Outcome::Continue(0));
    assert_eq!(f.shell.cwd(), sub);
    assert_eq!(f.run("cd .."), Outcome::Continue(0));
    assert_eq!(f.shell.cwd(), f.root.0);
}

#[test]
fn bad_cd_preserves_cwd_and_oldpwd() {
    let mut f = Fixture::new();
    fs::create_dir(f.root.0.join("sub")).unwrap();
    f.run("cd sub");
    let before = f.shell.cwd().to_owned();
    for command in ["cd missing", "cd a b", "cd --bad"] {
        assert_ne!(f.run(command).status(), 0, "{command}");
        assert_eq!(f.shell.cwd(), before);
    }
    assert_eq!(f.run("cd -"), Outcome::Continue(0));
    assert_eq!(f.shell.cwd(), f.root.0);
}

#[test]
fn empty_cd_and_extra_pwd_operands_follow_bash() {
    let mut f = Fixture::new();
    assert_eq!(f.run("cd ''"), Outcome::Continue(0));
    assert_eq!(f.shell.cwd(), f.root.0);
    assert_eq!(f.run("pwd ignored"), Outcome::Continue(0));
    assert_eq!(f.out().trim_end(), f.root.0.to_str().unwrap());
}

#[test]
fn cd_validates_components_before_removing_dot_dot() {
    let mut f = Fixture::new();
    fs::write(f.root.0.join("file"), b"not a directory").unwrap();
    fs::create_dir(f.root.0.join("directory")).unwrap();
    for source in [
        "cd missing/..",
        "cd file/..",
        "cd -P missing/..",
        "cd -P file/..",
    ] {
        assert_eq!(f.run(source).status(), 1, "{source}");
        assert_eq!(f.shell.cwd(), f.root.0);
    }
    assert_eq!(f.run("cd directory/..").status(), 0);
    assert_eq!(f.shell.cwd(), f.root.0);
    assert_eq!(f.run("cd -P .").status(), 0);
    let physical = f.shell.cwd().to_owned();
    assert_eq!(f.run("cd missing/..").status(), 1);
    assert_eq!(f.shell.cwd(), physical);
}

#[test]
fn logical_and_physical_navigation_resolve_directory_links_in_order() {
    let mut f = Fixture::new();
    let actual = f.root.0.join("actual");
    let logical = f.root.0.join("logical");
    fs::create_dir_all(actual.join("nested")).unwrap();
    fs::create_dir_all(logical.join("destination")).unwrap();
    let link = logical.join("link");
    #[cfg(windows)]
    {
        // Junctions exercise real Windows directory links without requiring
        // administrator privileges or Developer Mode for symbolic links.
        let output = Command::new("powershell.exe")
            .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command",
                "New-Item -ItemType Junction -Path $env:WINSH_TEST_LINK -Target $env:WINSH_TEST_TARGET -ErrorAction Stop | Out-Null"])
            .env("WINSH_TEST_LINK", &link)
            .env("WINSH_TEST_TARGET", actual.join("nested"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(actual.join("nested"), &link).unwrap();
    assert_eq!(f.run("cd logical/link/../destination").status(), 0);
    assert_eq!(f.shell.cwd(), logical.join("destination"));
    assert_eq!(f.run("cd ../link").status(), 0);
    assert_eq!(f.shell.cwd(), link);
    assert_eq!(f.run("cd ..").status(), 0);
    assert_eq!(f.shell.cwd(), logical);
    assert_eq!(f.run("cd -P link/..").status(), 0);
    assert_eq!(f.shell.cwd(), fs::canonicalize(&actual).unwrap());
    // Repeat from a canonical Windows cwd, whose verbatim prefix otherwise
    // makes PathBuf::join erase dot-dot before the shell can interpret it.
    let source = format!("cd -P {}", quote(&f.root.0));
    assert_eq!(f.run(&source).status(), 0);
    assert_eq!(f.run("cd logical/link").status(), 0);
    assert_eq!(f.run("cd -P ..").status(), 0);
    assert_eq!(f.shell.cwd(), fs::canonicalize(actual).unwrap());
}

#[test]
fn directory_options_home_and_tilde_work() {
    let mut f = Fixture::new();
    let home = f.root.0.join("home with spaces");
    fs::create_dir_all(home.join("child")).unwrap();
    fs::create_dir(f.root.0.join("-named")).unwrap();
    f.shell = Shell::with_environment(
        f.root.0.clone(),
        [(OsString::from("HOME"), home.clone().into_os_string())],
    )
    .unwrap();
    assert_eq!(f.run("cd -- -named").status(), 0);
    assert_eq!(f.run("cd").status(), 0);
    assert_eq!(f.shell.cwd(), home);
    assert_eq!(f.run("cd ~/child").status(), 0);
    assert_eq!(f.shell.cwd(), home.join("child"));
    assert_eq!(f.run("cd ~").status(), 0);
    assert_eq!(f.shell.cwd(), home);
    assert_eq!(f.run("pwd -P").status(), 0);
    assert_eq!(
        f.out().trim_end(),
        fs::canonicalize(&home).unwrap().to_str().unwrap()
    );
}

#[test]
fn missing_home_and_previous_directory_report_failures() {
    let mut f = Fixture::new();
    f.shell = Shell::with_environment(f.root.0.clone(), []).unwrap();
    assert_eq!(f.run("cd").status(), 1);
    assert_eq!(f.run("cd -").status(), 1);
    assert_ne!(f.run("cd ~").status(), 0);
    assert!(f.err().contains("HOME"));
    assert!(f.err().contains("OLDPWD"));
}

#[cfg(windows)]
#[test]
fn windows_home_fallback_and_environment_case_are_respected() {
    let mut f = Fixture::new();
    let home = f.root.0.join("home");
    fs::create_dir(&home).unwrap();
    f.shell = Shell::with_environment(
        f.root.0.clone(),
        [(OsString::from("UserProfile"), home.clone().into_os_string())],
    )
    .unwrap();
    assert_eq!(f.run("cd").status(), 0);
    assert_eq!(f.shell.cwd(), home);
}

#[test]
fn exit_is_control_flow_with_bash_numeric_status_rules() {
    let mut f = Fixture::new();
    assert_eq!(f.run("cd missing"), Outcome::Continue(1));
    assert_eq!(f.run("# nothing"), Outcome::Continue(1));
    assert_eq!(f.run("exit"), Outcome::Exit(1));
    for (source, expected) in [
        ("exit 0", 0),
        ("exit 257", 1),
        ("exit -1", 255),
        ("exit -- 42", 42),
        ("exit '  +7  '", 7),
        ("exit bad", 2),
        ("exit 99999999999999999999999999", 2),
    ] {
        assert_eq!(f.run(source), Outcome::Exit(expected));
    }
    assert_eq!(f.run("exit 1 2"), Outcome::Continue(1));
}

#[test]
fn unsupported_syntax_and_expansion_never_execute_a_command() {
    let mut f = Fixture::new();
    for source in [
        "pwd <<EOF",
        "pwd; case x in",
        "pwd $[1+2]",
        "pwd `something`",
    ] {
        assert_eq!(f.run(source), Outcome::Continue(2), "{source}");
    }
    assert!(f.out().is_empty());
    assert!(!f.root.0.join("file").exists());
}

#[test]
fn child_receives_exact_arguments_including_empty_unicode_and_metacharacters() {
    let mut f = Fixture::new();
    let args = [
        "",
        "hello world",
        "日本語 🦀",
        "$HOME",
        "*.rs",
        "; & | <>",
        r"C:\a b\",
        "a\"b",
        "it's",
    ];
    let source = format!(
        "{} args {}",
        quote(helper()),
        args.iter().map(quote).collect::<Vec<_>>().join(" ")
    );
    assert_eq!(f.run(&source).status(), 0);
    let expected: Vec<u8> = args.iter().flat_map(|arg| arg.bytes().chain([0])).collect();
    assert_eq!(fs::read(&f.output).unwrap(), expected);
    assert!(f.err().is_empty());
}

#[test]
fn child_receives_cwd_environment_and_separate_output_handles() {
    let mut f = Fixture::new();
    let cwd = f.root.0.join("child cwd");
    fs::create_dir(&cwd).unwrap();
    f.shell = Shell::with_environment(
        cwd.clone(),
        [(
            OsString::from("WINSH_TEST_VALUE"),
            OsString::from("a value with spaces"),
        )],
    )
    .unwrap();
    assert_eq!(
        f.run(&format!("{} env WINSH_TEST_VALUE", quote(helper())))
            .status(),
        0
    );
    assert_eq!(f.out(), "a value with spaces");
    assert_eq!(f.run(&format!("{} both", quote(helper()))).status(), 0);
    assert_eq!(f.out(), "a value with spaceschild stdout");
    assert_eq!(f.err(), "child stderr");
    assert_eq!(f.run(&format!("{} cwd", quote(helper()))).status(), 0);
    assert!(f.out().ends_with(&format!("{}\n", cwd.display())));
}

#[test]
fn child_status_missing_program_and_invalid_executable_have_distinct_statuses() {
    let mut f = Fixture::new();
    assert_eq!(
        f.run(&format!("{} status 37", quote(helper()))),
        Outcome::Continue(37)
    );
    assert_eq!(f.run("exit"), Outcome::Exit(37));
    assert_eq!(
        f.run("winsh-nonexistent-command-7fbc"),
        Outcome::Continue(127)
    );
    let invalid = f.root.0.join("invalid.exe");
    fs::write(&invalid, "not a native executable").unwrap();
    assert_eq!(f.run(&quote(&invalid)), Outcome::Continue(126));
    assert_eq!(f.run("''"), Outcome::Continue(127));
}

#[test]
fn command_lookup_uses_session_path_and_relative_paths_after_cd() {
    let mut f = Fixture::new();
    let bin = f.root.0.join("bin");
    fs::create_dir(&bin).unwrap();
    let executable = bin.join(format!("probe{}", env::consts::EXE_SUFFIX));
    fs::copy(helper(), &executable).unwrap();
    f.shell = Shell::with_environment(
        f.root.0.clone(),
        [(OsString::from("PATH"), OsString::from("bin"))],
    )
    .unwrap();
    assert_eq!(f.run("probe status 17").status(), 17);
    assert_eq!(f.run("cd bin").status(), 0);
    assert_eq!(f.run("./probe status 23").status(), 23);
    // A bare name must not implicitly search the shell's cwd or parent PATH.
    assert_eq!(f.run("probe status 0").status(), 127);
}

#[cfg(windows)]
#[test]
fn dotted_executable_names_can_omit_exe_and_native_status_is_not_truncated() {
    let mut f = Fixture::new();
    fs::copy(helper(), f.root.0.join("my.tool.exe")).unwrap();
    assert_eq!(f.run("./my.tool status 1000").status(), 1000);
    assert_eq!(f.run("./my.tool status -1073741510").status(), 130);
    let source = format!("{} status 1000", quote(helper()));
    assert_eq!(cli(&["-c", &source], b"").status.code(), Some(1000));
}

#[test]
fn quoting_controls_tilde_and_expansion_markers() {
    let mut f = Fixture::new();
    let source = format!(
        r#"{} args '~' "~" \~ ~"" '$HOME' "\$HOME" \* "$." "#,
        quote(helper())
    );
    assert_eq!(f.run(&source).status(), 0);
    assert_eq!(
        fs::read(&f.output).unwrap(),
        b"~\0~\0~\0~\0$HOME\0$HOME\0*\0$.\0"
    );
}

#[test]
fn scripts_preserve_stdin_arguments_encoding_and_status() {
    let root = TempDir::new();
    let path = root.0.join("space script.sh");
    fs::write(&path,"\u{feff}#!/usr/bin/env winsh\r\nprintf '<%s>\\n' \"$0\" \"$@\"\r\nread -r line\r\necho \"$line\"\r\nexit 17\r\n").unwrap();
    let output = cli(
        &[path.to_str().unwrap(), "", "two words"],
        b"independent stdin\n",
    );
    assert_eq!(output.status.code(), Some(17));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("<{}>\n<>\n<two words>\nindependent stdin\n", path.display())
    );
    assert!(output.stderr.is_empty());
    fs::write(&path, "echo bad\nif true; then echo incomplete\n").unwrap();
    let output = cli(&[path.to_str().unwrap()], b"");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(
        cli(&["missing-winsh-script.sh"], b"").status.code(),
        Some(127)
    );
}

#[test]
fn variables_export_temporary_assignments_and_native_lookup_use_session_state() {
    let mut f = Fixture::new();
    let child = quote(helper());
    let name = "WINSH_SCRIPT_VARIABLE";
    assert_eq!(
        f.run(&format!("{name}='two words'; {child} env-optional {name}"))
            .status(),
        0
    );
    assert_eq!(f.out(), "");
    assert_eq!(
        f.run(&format!("export {name}; {child} env {name}"))
            .status(),
        0
    );
    assert_eq!(f.out(), "two words");
    assert_eq!(
        f.run(&format!(
            "{name}=temporary {child} env {name}; printf '<%s>' \"${name}\""
        ))
        .status(),
        0
    );
    assert_eq!(f.out(), "two wordstemporary<two words>");
    assert_eq!(f.run(&format!("unset {name}; {name}=first {name}+=second {child} env {name}; printf '<%s>' \"${{{name}:-gone}}\"")).status(),0);
    assert_eq!(f.out(), "two wordstemporary<two words>firstsecond<gone>");
    fs::copy(helper(), f.root.0.join("script-helper.exe")).unwrap();
    assert_eq!(f.run("PATH=.; script-helper status 19").status(), 19);
    assert!(f.err().is_empty());
}

#[test]
fn globbing_and_redirect_expansions_respect_quotes_and_working_directory() {
    let mut f = Fixture::new();
    for path in ["a.txt", "b.txt", "c.rs", ".hidden.txt", "bracket[1].txt"] {
        fs::write(f.root.0.join(path), "").unwrap();
    }
    assert_eq!(
        f.run("printf '<%s>\\n' [ab].txt '*.txt' no-match*.x")
            .status(),
        0
    );
    assert_eq!(f.out(), "<a.txt>\n<b.txt>\n<*.txt>\n<no-match*.x>\n");
    assert_eq!(f.run("name='space file'; echo data >\"$name\"").status(), 0);
    assert_eq!(fs::read(f.root.0.join("space file")).unwrap(), b"data\n");
    assert_eq!(f.run("echo bad >$name").status(), 2);
    assert_eq!(f.run("echo bad >[ab].txt").status(), 2);
    assert_eq!(fs::read(f.root.0.join("a.txt")).unwrap(), b"");
}

#[test]
fn sourcing_shares_state_and_restores_function_arguments_and_locals() {
    let mut f = Fixture::new();
    fs::write(
        f.root.0.join("library.sh"),
        "value=shared; echo \"source:$1:$0\"; return 6; echo bad\n",
    )
    .unwrap();
    assert_eq!(f.run("set -- parent; f() { local value=local; source ./library.sh child; echo \"$?:$value:$1\"; }; value=global; f caller; echo \"$value:$1\"").status(),0);
    assert_eq!(
        f.out(),
        "source:child:winsh\n6:shared:caller\nglobal:parent\n"
    );
}

#[test]
fn direct_scripts_are_isolated_and_can_be_pipeline_stages() {
    let mut f = Fixture::new();
    let path = f.root.0.join("worker.sh");
    fs::create_dir(f.root.0.join("sub")).unwrap();
    fs::write(&path,"#!/usr/bin/env winsh\necho \"$0|$1|${WINSH_PRIVATE:-missing}|$WINSH_PUBLIC\"; cd sub; exit 4\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(f.run("WINSH_PRIVATE=private; export WINSH_PUBLIC=exported; ./worker.sh 'two words'; echo \"$?:$WINSH_PRIVATE\"").status(),0);
    assert_eq!(
        f.out(),
        "./worker.sh|two words|missing|exported\n4:private\n"
    );
    assert_eq!(f.shell.cwd(), f.root.0);
    assert_eq!(f.run("PATH=.; worker.sh child | read line").status(), 0);
}

#[test]
fn function_redirections_are_deferred_until_the_function_is_called() {
    let mut f = Fixture::new();
    assert_eq!(f.run("f() { echo \"$1\"; } >deferred").status(), 0);
    assert!(!f.root.0.join("deferred").exists());
    assert_eq!(f.run("f value").status(), 0);
    assert_eq!(fs::read(f.root.0.join("deferred")).unwrap(), b"value\n");
}

#[test]
fn large_command_substitutions_and_compound_pipelines_do_not_deadlock() {
    let child = quote(helper());
    let output = cli_bounded(&format!(
        "value=$({child} generate-text 1048576); echo \"${{#value}}\""
    ));
    assert!(output.status.success());
    assert_eq!(output.stdout, b"1048576\n");
    let output = cli_bounded(&format!(
        "value=$({child} generate 1048576); echo \"${{#value}}\""
    ));
    // The binary generator includes NUL, which a shell variable cannot contain.
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("NUL"));
    let output = cli_bounded(&format!(
        "{{ {child} generate 1048576; }} | {child} copy | {child} head 123"
    ));
    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 123);
}

#[cfg(windows)]
#[test]
fn batch_files_require_explicit_interpreter_invocation() {
    let mut f = Fixture::new();
    fs::write(f.root.0.join("script.cmd"), "@echo unexpected").unwrap();
    assert_eq!(f.run("./script.cmd").status(), 126);
    assert!(f.out().is_empty());
}

#[test]
fn cli_command_mode_reports_usage_errors_and_preserves_status() {
    assert_eq!(cli(&["-c", "exit 42"], b"").status.code(), Some(42));
    for args in [vec!["-c"], vec!["--unknown"]] {
        assert_eq!(cli(&args, b"").status.code(), Some(2));
    }
    assert_eq!(cli(&["--version"], b"").status.code(), Some(0));
}

#[test]
fn cli_session_preserves_state_and_stops_at_exit() {
    let root = TempDir::new();
    fs::create_dir(root.0.join("space dir")).unwrap();
    let source = format!(
        "cd {}\r\ncd 'space dir'\r\npwd\r\ncd missing\r\nexit\r\npwd\r\n",
        quote(&root.0)
    );
    let output = cli(&[], source.as_bytes());
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim_end(),
        root.0.join("space dir").to_str().unwrap()
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("cd:"));
    assert!(!stderr.contains(" $ "));
}

#[test]
fn cli_multiline_quotes_and_continuations_share_the_parser() {
    let source = format!(
        "{} args 'first\nsecond' one\\\ntwo\nexit\n",
        quote(helper())
    );
    let output = cli(&[], source.as_bytes());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"first\nsecond\0onetwo\0");
    assert!(output.stderr.is_empty());
    assert_eq!(cli(&[], b"pwd 'unfinished\n").status.code(), Some(2));
}

#[test]
fn native_children_receive_stdin_without_shell_read_ahead() {
    let command = format!("{} stdin", quote(helper()));
    let output = cli(&["-c", &command], b"child input\0binary");
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"child input\0binary");
    let source = format!("{command}\nthis belongs to the child\n");
    let output = cli(&[], source.as_bytes());
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"this belongs to the child\n");
}

#[test]
fn shell_io_files_apply_to_native_stdin_as_well_as_builtin_output() {
    let mut f = Fixture::new();
    let input = f.root.0.join("input");
    fs::write(&input, b"file input\0bytes").unwrap();
    f.io = ShellIo::inherited()
        .with_stdin(File::open(input).unwrap())
        .with_stdout(File::create(&f.output).unwrap())
        .with_stderr(File::create(&f.error).unwrap());
    assert_eq!(f.run(&format!("{} stdin", quote(helper()))).status(), 7);
    assert_eq!(fs::read(&f.output).unwrap(), b"file input\0bytes");
}

#[test]
fn output_redirection_append_clobber_and_redirect_only_commands_restore_streams() {
    let mut f = Fixture::new();
    let expected = format!("{}\n", f.root.0.display());
    assert_eq!(f.run("pwd >'space café 日本語'"), Outcome::Continue(0));
    assert_eq!(
        fs::read_to_string(f.root.0.join("space café 日本語")).unwrap(),
        expected
    );
    assert!(f.out().is_empty());
    assert_eq!(f.run("pwd >>'space café 日本語'"), Outcome::Continue(0));
    assert_eq!(
        fs::read_to_string(f.root.0.join("space café 日本語")).unwrap(),
        expected.repeat(2)
    );
    assert_eq!(f.run("pwd >|'space café 日本語'"), Outcome::Continue(0));
    assert_eq!(
        fs::read_to_string(f.root.0.join("space café 日本語")).unwrap(),
        expected
    );
    assert_eq!(f.run(">empty"), Outcome::Continue(0));
    assert_eq!(fs::read(f.root.0.join("empty")).unwrap(), b"");
    assert_eq!(f.run("pwd"), Outcome::Continue(0));
    assert_eq!(f.out(), expected);
}

#[test]
fn native_stdin_stdout_and_stderr_can_be_redirected_independently() {
    let mut f = Fixture::new();
    let bytes = b"input\0binary\xff\xfe\n";
    fs::write(f.root.0.join("input"), bytes).unwrap();
    let child = quote(helper());
    assert_eq!(
        f.run(&format!("{child} copy <input >output")),
        Outcome::Continue(0)
    );
    assert_eq!(fs::read(f.root.0.join("output")).unwrap(), bytes);
    assert_eq!(
        f.run(&format!("{child} both >output 2>errors")),
        Outcome::Continue(0)
    );
    assert_eq!(fs::read(f.root.0.join("output")).unwrap(), b"child stdout");
    assert_eq!(fs::read(f.root.0.join("errors")).unwrap(), b"child stderr");
    assert_eq!(
        f.run(&format!("{child} both >output 2>>errors")),
        Outcome::Continue(0)
    );
    assert_eq!(
        fs::read(f.root.0.join("errors")).unwrap(),
        b"child stderrchild stderr"
    );
    assert!(f.out().is_empty());
    assert!(f.err().is_empty());
}

#[test]
fn descriptor_duplication_snapshots_the_current_stream_in_source_order() {
    let child = quote(helper());
    let mut f = Fixture::new();
    assert_eq!(f.run(&format!("{child} both >merged 2>&1")).status(), 0);
    assert_eq!(
        fs::read(f.root.0.join("merged")).unwrap(),
        b"child stdoutchild stderr"
    );
    assert!(f.out().is_empty());
    let mut f = Fixture::new();
    assert_eq!(f.run(&format!("{child} both 2>&1 >output")).status(), 0);
    assert_eq!(fs::read(f.root.0.join("output")).unwrap(), b"child stdout");
    assert_eq!(f.out(), "child stderr");
    assert!(f.err().is_empty());
    let mut f = Fixture::new();
    assert_eq!(f.run(&format!("{child} both 1>&2")).status(), 0);
    assert_eq!(f.err(), "child stdoutchild stderr");
    assert!(f.out().is_empty());
    // Also exercise inherited streams rather than File-backed engine fixtures.
    let output = cli_bounded(&format!(
        "{child} both 2>&1 >{}",
        quote(f.root.0.join("native"))
    ));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"child stderr");
    assert!(output.stderr.is_empty());
}

#[test]
fn combined_output_redirections_and_input_duplication_work() {
    let child = quote(helper());
    for operator in ["&>", ">&"] {
        let mut f = Fixture::new();
        assert_eq!(
            f.run(&format!("{child} both {operator}combined")).status(),
            0
        );
        assert_eq!(
            fs::read(f.root.0.join("combined")).unwrap(),
            b"child stdoutchild stderr"
        );
        assert_eq!(f.run(&format!("{child} both &>>combined")).status(), 0);
        assert_eq!(
            fs::read(f.root.0.join("combined")).unwrap(),
            b"child stdoutchild stderrchild stdoutchild stderr"
        );
    }
    let mut f = Fixture::new();
    fs::write(f.root.0.join("input"), b"preserved").unwrap();
    assert_eq!(
        f.run(&format!("{child} copy 2<input 0<&2 >output"))
            .status(),
        0
    );
    assert_eq!(fs::read(f.root.0.join("output")).unwrap(), b"preserved");
    assert_eq!(f.run(&format!("{child} copy <>input >output")).status(), 0);
    assert_eq!(fs::read(f.root.0.join("input")).unwrap(), b"preserved");
    assert_eq!(fs::read(f.root.0.join("output")).unwrap(), b"preserved");
    assert_eq!(f.run("<>created").status(), 0);
    assert!(f.root.0.join("created").is_file());
}

#[test]
fn redirection_paths_follow_shell_cwd_and_quote_aware_home_expansion() {
    let mut f = Fixture::new();
    let home = f.root.0.join("home");
    fs::create_dir(&home).unwrap();
    fs::create_dir(f.root.0.join("sub")).unwrap();
    f.shell = Shell::with_environment(
        f.root.0.clone(),
        [(OsString::from("HOME"), home.clone().into_os_string())],
    )
    .unwrap();
    assert_eq!(f.run("pwd >~/location").status(), 0);
    assert_eq!(
        fs::read_to_string(home.join("location")).unwrap(),
        format!("{}\n", f.root.0.display())
    );
    assert_eq!(f.run("cd sub >created").status(), 0);
    assert!(f.root.0.join("created").is_file());
    assert_eq!(f.run("pwd >location").status(), 0);
    assert!(f.root.0.join("sub/location").is_file());
    assert_eq!(f.run("pwd >'~literal'").status(), 0);
    assert!(f.root.0.join("sub/~literal").is_file());
}

#[test]
fn failed_redirections_prevent_builtins_and_use_previously_redirected_stderr() {
    let mut f = Fixture::new();
    fs::create_dir(f.root.0.join("sub")).unwrap();
    assert_eq!(
        f.run("cd sub 2>errors >missing/output"),
        Outcome::Continue(1)
    );
    assert_eq!(f.shell.cwd(), f.root.0);
    assert!(
        fs::read_to_string(f.root.0.join("errors"))
            .unwrap()
            .contains("missing/output")
    );
    assert!(f.err().is_empty());
    assert_eq!(f.run("exit 42 >missing/output"), Outcome::Continue(1));
    assert_eq!(f.run("pwd").status(), 0);
    assert_eq!(f.out(), format!("{}\n", f.root.0.display()));
    assert_eq!(f.run("pwd >first >second").status(), 0);
    assert_eq!(fs::read(f.root.0.join("first")).unwrap(), b"");
    assert!(!fs::read(f.root.0.join("second")).unwrap().is_empty());
}

#[test]
fn unwritable_redirected_stderr_preserves_command_and_pipeline_status() {
    let mut f = Fixture::new();
    fs::write(f.root.0.join("input"), b"preserved").unwrap();
    assert_eq!(
        f.run("exit 42 2<input >missing/output"),
        Outcome::Continue(1)
    );
    assert_eq!(
        f.run("pwd 2<input >missing/output | exit 7"),
        Outcome::Continue(7)
    );
    assert_eq!(
        f.run("missing-winsh-command 2<input"),
        Outcome::Continue(127)
    );
    assert_eq!(f.run("exit bad 2<input"), Outcome::Exit(2));
    assert_eq!(fs::read(f.root.0.join("input")).unwrap(), b"preserved");
    assert!(f.err().is_empty());
}

#[test]
fn syntax_errors_are_validated_before_pipeline_side_effects() {
    let mut f = Fixture::new();
    for source in [
        "pwd >created $[1+2]",
        "pwd >created | pwd `foo`",
        "pwd >created | case x in",
    ] {
        assert_eq!(f.run(source), Outcome::Continue(2), "{source}");
        assert!(!f.root.0.join("created").exists());
        assert!(f.out().is_empty());
    }
    for source in ["pwd 2>&9", "pwd 1>&nonsense"] {
        assert_eq!(f.run(source), Outcome::Continue(1), "{source}");
    }
    assert!(!f.root.0.join("-").exists());
}

#[test]
fn builtin_output_write_errors_fail_the_command_and_keep_the_session_alive() {
    let mut f = Fixture::new();
    fs::write(f.root.0.join("readonly"), b"preserved").unwrap();
    assert_eq!(f.run("pwd 1<readonly"), Outcome::Continue(1));
    assert_eq!(fs::read(f.root.0.join("readonly")).unwrap(), b"preserved");
    assert_eq!(f.run("pwd"), Outcome::Continue(0));
}

#[test]
fn pipelines_stream_large_binary_output_through_all_stages() {
    let f = Fixture::new();
    let child = quote(helper());
    let output_file = f.root.0.join("payload");
    let output = cli_bounded(&format!(
        "{child} generate 4194304 | {child} copy | {child} copy >{}",
        quote(&output_file)
    ));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let bytes = fs::read(output_file).unwrap();
    assert_eq!(bytes.len(), 4 * 1024 * 1024);
    assert!(
        bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| *byte == (index % 256) as u8)
    );
}

#[test]
fn early_consumers_and_failed_middle_stages_close_all_unused_pipe_ends() {
    let f = Fixture::new();
    let child = quote(helper());
    let prefix = f.root.0.join("prefix");
    let output = cli_bounded(&format!(
        "{child} generate 16777216 | {child} head 17 >{}",
        quote(&prefix)
    ));
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(fs::read(prefix).unwrap(), (0..17).collect::<Vec<u8>>());
    let output = cli_bounded(&format!(
        "{child} generate 16777216 | winsh-command-that-does-not-exist | {child} copy"
    ));
    assert_eq!(output.status.code(), Some(0));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("winsh-command-that-does-not-exist"));
}

#[test]
fn explicit_redirections_override_pipes_and_pipe_stderr_merges_after_them() {
    let child = quote(helper());
    let mut f = Fixture::new();
    assert_eq!(
        f.run(&format!("{child} both 2>unused |& {child} copy >merged"))
            .status(),
        0
    );
    assert_eq!(fs::read(f.root.0.join("unused")).unwrap(), b"");
    assert_eq!(
        fs::read(f.root.0.join("merged")).unwrap(),
        b"child stdoutchild stderr"
    );
    assert_eq!(
        f.run(&format!("{child} both >diverted |& {child} copy >empty"))
            .status(),
        0
    );
    assert_eq!(
        fs::read(f.root.0.join("diverted")).unwrap(),
        b"child stdoutchild stderr"
    );
    assert_eq!(fs::read(f.root.0.join("empty")).unwrap(), b"");
    fs::write(f.root.0.join("input"), b"replacement input").unwrap();
    assert_eq!(
        f.run(&format!(
            "{child} generate 1024 | {child} copy <input >output"
        ))
        .status(),
        0
    );
    assert_eq!(
        fs::read(f.root.0.join("output")).unwrap(),
        b"replacement input"
    );
    assert_eq!(
        f.run(&format!("pwd 1>&- |& {child} copy >empty")).status(),
        0
    );
    assert_eq!(fs::read(f.root.0.join("empty")).unwrap(), b"");
}

#[test]
fn pipeline_builtins_use_isolated_sessions_and_exit_does_not_leave_the_parent() {
    let child = quote(helper());
    let mut f = Fixture::new();
    fs::create_dir(f.root.0.join("sub")).unwrap();
    assert_eq!(f.run(&format!("pwd | {child} copy >output")).status(), 0);
    assert_eq!(
        fs::read_to_string(f.root.0.join("output")).unwrap(),
        format!("{}\n", f.root.0.display())
    );
    assert_eq!(f.run("cd sub | pwd"), Outcome::Continue(0));
    assert_eq!(f.shell.cwd(), f.root.0);
    assert_eq!(f.out(), format!("{}\n", f.root.0.display()));
    assert_eq!(f.run("pwd | cd sub"), Outcome::Continue(0));
    assert_eq!(f.shell.cwd(), f.root.0);
    assert_eq!(
        f.run(&format!("exit 42 | {child} status 7")),
        Outcome::Continue(7)
    );
    assert_eq!(f.run("pwd | exit 9"), Outcome::Continue(9));
    assert_eq!(f.run("pwd"), Outcome::Continue(0));
}

#[test]
fn pipeline_status_comes_from_the_last_stage_and_all_children_are_waited() {
    let child = quote(helper());
    let mut f = Fixture::new();
    assert_eq!(
        f.run(&format!("{child} status 37 | {child} status 5")),
        Outcome::Continue(5)
    );
    assert_eq!(f.shell.last_status(), 5);
    assert_eq!(
        f.run(&format!("{child} status 0 | missing-winsh-command")),
        Outcome::Continue(127)
    );
    assert_eq!(
        f.run(&format!("{child} copy <missing | {child} copy")),
        Outcome::Continue(0)
    );
    assert_eq!(
        f.run(&format!(
            "{child} generate 32 | {child} copy >missing/output"
        )),
        Outcome::Continue(1)
    );
    assert_eq!(
        f.run(&format!("{child} delayed-mark marker | {child} status 0")),
        Outcome::Continue(0)
    );
    assert!(
        f.root.0.join("marker").is_file(),
        "upstream must finish before returning"
    );
}

#[test]
fn host_io_failure_during_pipeline_start_kills_and_reaps_started_children() {
    let mut f = Fixture::new();
    let child = quote(helper());
    f.io = ShellIo::inherited()
        .with_stdout(File::create(&f.output).unwrap())
        .with_stderr(File::open(&f.error).unwrap());
    let source = format!("{child} delayed-mark leaked | missing-winsh-command");
    assert!(f.shell.run(&source, &mut f.io).is_err());
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert!(
        !f.root.0.join("leaked").exists(),
        "started child was abandoned"
    );
}

#[test]
fn cli_accepts_multiline_pipelines_and_does_not_read_ahead_of_native_stdin() {
    let child = quote(helper());
    let source = format!("{child} args one |\n# continuation\n{child} copy\nexit\n");
    let output = cli(&[], source.as_bytes());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"one\0");
    let source = format!("{child} copy | {child} copy\npayload\0binary\n");
    let output = cli(&[], source.as_bytes());
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(output.stdout, b"payload\0binary\n");
    assert_eq!(cli(&["-c", "pwd |"], b"").status.code(), Some(2));
    assert_eq!(cli(&[], b"pwd |\n").status.code(), Some(2));
}

#[test]
fn descriptor_closing_and_moving_fail_native_io_and_restore_parent_streams() {
    let child = quote(helper());
    let mut f = Fixture::new();
    assert_eq!(f.run("pwd >&-"), Outcome::Continue(1));
    assert_eq!(f.run("pwd 1>&- 2>&1"), Outcome::Continue(1));
    assert_eq!(f.run("missing-winsh-command 2>&-"), Outcome::Continue(127));
    assert_eq!(f.run("exit bad 2>&-"), Outcome::Exit(2));
    assert_eq!(f.run("pwd 2>&- >missing/output"), Outcome::Continue(1));
    assert_eq!(f.run("pwd"), Outcome::Continue(0));
    assert_eq!(f.out(), format!("{}\n", f.root.0.display()));
    assert_eq!(f.run("cd missing 1>moved 2>&1-"), Outcome::Continue(1));
    assert!(
        fs::read_to_string(f.root.0.join("moved"))
            .unwrap()
            .contains("cd:")
    );
    fs::write(f.root.0.join("input"), b"moved input").unwrap();
    assert_eq!(
        f.run(&format!("{child} copy 2<input 0<&2- >output"))
            .status(),
        0
    );
    assert_eq!(fs::read(f.root.0.join("output")).unwrap(), b"moved input");
    assert_eq!(f.run(&format!("{child} status 0 1>&- 2>&-")).status(), 0);
    let output = cli_bounded(&format!("{child} raw-stdin 0<&-"));
    assert_ne!(
        output.status.code(),
        Some(0),
        "reading a closed input must fail"
    );
    let output = cli_bounded(&format!("{child} raw-stdout 1>&-"));
    assert_ne!(
        output.status.code(),
        Some(0),
        "writing a closed output must fail"
    );
    assert!(output.stdout.is_empty());
}

#[test]
#[ignore = "requires Bash; set WINSH_TEST_BASH to its executable path"]
fn redirections_and_pipelines_match_bash() {
    let bash = env::var_os("WINSH_TEST_BASH").expect("set WINSH_TEST_BASH");
    let child = quote(helper().to_string_lossy().replace('\\', "/"));
    let cases = [
        format!("{child} both >a 2>&1"),
        format!("{child} both 2>&1 >a"),
        format!("{child} both 1>&2 2>a"),
        format!("{child} both &>a"),
        format!("{child} both >&a"),
        format!("{child} both &>>a"),
        format!("{child} both >a >b"),
        format!("{child} both 2>a |& {child} copy >b"),
        format!("{child} both >a |& {child} copy >b"),
        format!("{child} both 2>&1 | {child} copy >a"),
        format!("{child} copy <input | {child} copy >a"),
        format!("{child} copy <>input >a"),
        format!("{child} copy 2<input 0<&2- >a"),
        format!("{child} status 7 | {child} status 3"),
        format!("{child} status 0 0<&1"),
        format!("{child} status 0 1>&0"),
        format!("{child} status 0 1>&- 2>&-"),
        format!("exit 7 | {child} status 0"),
        "cd missing 1>a 2>&1-".to_owned(),
        ">a".to_owned(),
    ];
    for source in cases {
        let mut f = Fixture::new();
        let reference = TempDir::new();
        fs::write(f.root.0.join("input"), b"input\0bytes\xff").unwrap();
        fs::write(reference.0.join("input"), b"input\0bytes\xff").unwrap();
        fs::write(f.root.0.join("a"), b"existing").unwrap();
        fs::write(reference.0.join("a"), b"existing").unwrap();
        let mut process = Command::new(&bash)
            .args(["--noprofile", "--norc", "-s"])
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .env("MSYS2_ARG_CONV_EXCL", "*")
            .current_dir(&reference.0)
            .stdin(Stdio::piped())
            .stdout(File::create(reference.0.join("stdout")).unwrap())
            .stderr(File::create(reference.0.join("stderr")).unwrap())
            .spawn()
            .unwrap();
        process
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{source}\n").as_bytes())
            .unwrap();
        let expected = process.wait().unwrap();
        let actual = f.run(&source);
        assert_eq!(actual.status(), expected.code().unwrap(), "{source}");
        assert_eq!(
            fs::read(&f.output).unwrap(),
            fs::read(reference.0.join("stdout")).unwrap(),
            "stdout: {source}"
        );
        // Built-in diagnostics use the winsh prefix and native Windows errors.
        if !source.starts_with("cd ") {
            assert_eq!(
                fs::read(&f.error).unwrap(),
                fs::read(reference.0.join("stderr")).unwrap(),
                "stderr: {source}"
            );
        }
        for name in ["input", "a", "b"] {
            if source.starts_with("cd ") && name == "a" {
                continue;
            }
            assert_eq!(
                fs::read(f.root.0.join(name)).ok(),
                fs::read(reference.0.join(name)).ok(),
                "file {name}: {source}"
            );
        }
    }
}

/// Bound regression tests that would otherwise hang forever on a leaked pipe
/// endpoint or a producer being waited before its consumer has been started.
fn cli_bounded(source: &str) -> Output {
    use std::{
        io::Read,
        thread,
        time::{Duration, Instant},
    };
    let mut child = Command::new(env!("CARGO_BIN_EXE_winsh"))
        .args(["-c", source])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    thread::scope(|scope| {
        let out = scope.spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let err = scope.spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("pipeline did not finish: {source}");
            }
            thread::sleep(Duration::from_millis(10));
        };
        Output {
            status,
            stdout: out.join().unwrap(),
            stderr: err.join().unwrap(),
        }
    })
}
