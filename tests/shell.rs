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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
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
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-support");
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
        "pwd | something",
        "pwd >file",
        "pwd; exit",
        "pwd $HOME",
        "pwd \"$HOME\"",
        "pwd *.rs",
        "pwd {a,b}",
        "pwd $(something)",
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
    for args in [
        vec!["-c"],
        vec!["-c", "pwd", "extra"],
        vec!["--unknown"],
        vec!["script.sh"],
    ] {
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
