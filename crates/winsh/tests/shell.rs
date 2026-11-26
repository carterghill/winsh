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

#[test]
fn job_control_stops_native_work_and_retains_pipeline_status() {
    let mut f = Fixture::new();
    f.shell
        .set_job_executable(env!("CARGO_BIN_EXE_winsh").into());
    let child = quote(helper());
    let command = format!("{child} gated-mark | cat && exit 37");
    assert_eq!(
        f.run(&format!("{command} & pid=$!; jobs")),
        Outcome::Continue(0)
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !f.root.0.join("ready").is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "worker did not start: {}",
            f.err()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(f.run("kill -STOP %1; jobs"), Outcome::Continue(0));
    fs::write(f.root.0.join("release"), b"go").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(120));
    assert!(
        !f.root.0.join("passed").exists(),
        "a stopped native stage kept running"
    );
    assert_eq!(f.run("bg %1; wait \"$pid\""), Outcome::Continue(37));
    assert!(f.root.0.join("passed").is_file());
    assert!(f.out().contains("[1] Running") && f.out().contains("[1] Stopped"));
    assert_eq!(f.run("wait %1"), Outcome::Continue(127));
}

#[test]
fn background_lists_inherit_shell_state_and_isolate_changes() {
    let mut f = Fixture::new();
    f.shell
        .set_job_executable(env!("CARGO_BIN_EXE_winsh").into());
    f.shell
        .set_arguments("script".into(), vec!["positional".into()]);
    fs::write(f.root.0.join("input"), "from-file\n").unwrap();
    assert_eq!(
        f.run(
            r#"
        private=value
        task() { read -r line; echo "$private $line $1"; private=changed; exit 9; }
        false || task "$1" <input >result & id=$!
        wait "$id"; echo "$? $private"
        true & false & wait
    "#
        ),
        Outcome::Continue(0)
    );
    assert_eq!(
        fs::read_to_string(f.root.0.join("result")).unwrap(),
        "value from-file positional\n"
    );
    assert_eq!(f.out(), "9 value\n");
    let nested = format!("{}true{} & wait %4", "{ ".repeat(24), "; }".repeat(24));
    assert_eq!(f.run(&nested), Outcome::Continue(0));
}

#[cfg(windows)]
#[test]
#[allow(unsafe_code)]
fn shell_shutdown_reaps_a_stopped_jobs_native_descendants() {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0,
        System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    };
    let mut f = Fixture::new();
    f.shell
        .set_job_executable(env!("CARGO_BIN_EXE_winsh").into());
    assert_eq!(
        f.run(&format!("{} spawn-gated &", quote(helper()))),
        Outcome::Continue(0)
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let pid = loop {
        if let Some(pid) = fs::read_to_string(f.root.0.join("ready"))
            .ok()
            .and_then(|s| s.parse().ok())
        {
            break pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "grandchild failed to start: {}",
            f.err()
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    // SAFETY: request wait access only, and own the returned handle until the check ends.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    assert!(!handle.is_null());
    let grandchild = unsafe { OwnedHandle::from_raw_handle(handle) };
    assert_eq!(f.run("stop %1"), Outcome::Continue(0));
    drop(f.shell);
    assert_eq!(
        unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 3_000) },
        WAIT_OBJECT_0
    );
}

#[test]
fn completion_uses_session_lookup_and_shell_word_boundaries() {
    let mut fixture = Fixture::new();
    let work = fixture.root.0.join("work");
    let bin = work.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join("native-tool.sh");
    fs::write(&script, "echo from-script").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(bin.join("native-note.txt"), "").unwrap();
    fs::write(work.join("café's $note[1].txt"), "contents\n").unwrap();
    fs::write(work.join("bind.txt"), "").unwrap();
    assert_eq!(
        fixture
            .run("cd work; export PATH=bin HOME=\"$PWD\"; notify() { :; }")
            .status(),
        0
    );
    let ctx = fixture.shell.completion_context();
    let additions = |before, after| {
        ctx.candidates(before, after)
            .into_iter()
            .map(|(_, insert)| insert)
            .collect::<Vec<_>>()
    };
    for before in [
        "nat",
        "./bin/nat",
        "pwd && nat",
        "cd . | nat",
        "X='a b' 2>out nat",
        "if true; then nat",
    ] {
        assert_eq!(additions(before, ""), ["ive-tool.sh "], "{before}");
    }
    assert_eq!(additions("not", ""), ["ify "]);
    assert_eq!(additions("gre", ""), ["p "]);
    assert_eq!(additions("mkd", ""), ["ir "]);
    assert_eq!(additions("pri", "tf x"), ["n"]);
    assert_eq!(additions("cd bi", ""), ["n/"]);
    assert_eq!(additions("cat bi", ""), ["n/", "nd.txt "]);
    assert_eq!(additions("echo a#b >bi", ""), ["n/", "nd.txt "]);
    for before in ["cat 2>&", "cat # bi", "cat $(echo caf", "echo >$(echo x)/"] {
        assert!(additions(before, "").is_empty(), "{before}");
    }
    for before in ["cat caf", "cat 'caf", "cat \"caf", "cat ~/caf"] {
        let matches = additions(before, "");
        assert_eq!(matches.len(), 1, "{before}");
        assert_eq!(
            fixture.run(&format!("{before}{}", matches[0])).status(),
            0,
            "{before}"
        );
    }
    assert_eq!(fixture.out(), "contents\n".repeat(4));
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

#[cfg(windows)]
#[test]
fn ip_reports_local_addresses_independently_of_public_lookup() {
    let mut f = Fixture::new();
    f.run("export PATH=.");
    assert_eq!(f.run("command -v ip; ip --help").status(), 0);
    assert_eq!(f.run("ip --unknown").status(), 2);
    assert!(!f.err().contains("curl.exe"));

    let local_status = f.run("ip --local >local").status();
    let local = fs::read_to_string(f.root.0.join("local")).unwrap();
    if local.is_empty() {
        assert_eq!(local_status, 1);
        assert!(f.err().contains("no active local addresses"));
    } else {
        assert_eq!(local_status, 0);
        assert_eq!(local.lines().count(), 1);
        for word in local.split_whitespace() {
            let address: std::net::IpAddr = word.parse().unwrap();
            assert!(!address.is_loopback() && !address.is_unspecified());
        }
    }
    assert!(!f.err().contains("curl.exe"));
    fs::copy(helper(), f.root.0.join("curl.exe")).unwrap();
    let labeled_local: String = local
        .split_whitespace()
        .map(|address| {
            let family = if address.contains(':') {
                "IPv6"
            } else {
                "IPv4"
            };
            format!("Local {family}: {address}\n")
        })
        .collect();
    for (family, address) in [("IPv4", "203.0.113.7"), ("IPv6", "2001:db8::7")] {
        assert_eq!(
            f.run(&format!("WINSH_TEST_IP_RESPONSE={address} ip >both"))
                .status(),
            local_status
        );
        let both = fs::read_to_string(f.root.0.join("both")).unwrap();
        assert_eq!(both, format!("{labeled_local}Public {family}: {address}\n"));
    }
    for command in ["ip >partial", "WINSH_TEST_IP_RESPONSE='<html>' ip >partial"] {
        assert_eq!(f.run(command).status(), 1);
        let partial = fs::read_to_string(f.root.0.join("partial")).unwrap();
        assert_eq!(partial, labeled_local);
    }
    assert!(f.err().contains("curl.exe exited with status 28"));
    assert!(f.err().contains("invalid IP address"));
}

#[cfg(windows)]
#[test]
fn pkg_installs_python_runtime_and_reuses_its_manager() {
    let mut f = Fixture::new();
    f.run("export PATH=.");
    fs::copy(helper(), f.root.0.join("winget.exe")).unwrap();
    fs::copy(helper(), f.root.0.join("python3.exe")).unwrap();
    fs::copy(helper(), f.root.0.join("pending.exe")).unwrap();

    assert_eq!(f.run("pkg install python3").status(), 0);
    assert_eq!(f.run("pkg install python3").status(), 0);
    assert_eq!(
        f.out(),
        "winget install\n[--exact]\n[--id]\n[9NQ7512CXL7T]\n[--source]\n[msstore]\npymanager install\n[3]\npymanager install\n[3]\n"
    );
    assert_eq!(
        f.run("WINSH_TEST_INSTALL_STATUS=19 pkg install python3")
            .status(),
        19
    );
}

#[cfg(windows)]
#[test]
fn pkg_passes_names_and_ids_as_queries_and_preserves_install_failures() {
    let mut f = Fixture::new();
    f.run("export PATH=.");
    for source in [
        "pkg",
        "pkg remove Git.Git",
        "pkg install ''",
        "pkg install --help",
        "pkg install A B",
    ] {
        assert_eq!(f.run(source).status(), 2, "{source}");
    }
    assert_eq!(f.run("pkg install Git.Git").status(), 127);
    assert!(f.err().contains("App Installer"));
    fs::copy(helper(), f.root.0.join("winget.exe")).unwrap();
    for query in ["lazygit", "fzf", "Git.Git", "one & two"] {
        let command = format!("command pkg install '{query}' >args");
        assert_eq!(f.run(&command).status(), 0);
        assert_eq!(
            fs::read_to_string(f.root.0.join("args")).unwrap(),
            format!("winget install\n[--query]\n[{query}]\n")
        );
    }
    assert_eq!(
        f.run("WINSH_TEST_INSTALL_STATUS=17 pkg install fzf")
            .status(),
        17
    );
    assert_eq!(
        f.run("WINSH_TEST_INSTALL_STATUS=23 pkg install python3")
            .status(),
        23
    );
    assert_eq!(f.run("pkg install python3").status(), 127);
    assert!(f.err().contains("Manage app execution aliases"));
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
    assert_eq!(
        f.run(&format!("export {name}; {child} env-is-set {name}"))
            .status(),
        1
    );
    assert_eq!(
        f.run(&format!("{name}=; {child} env-is-set {name}"))
            .status(),
        0
    );
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
    assert_eq!(f.run("echo bad >$name").status(), 1);
    assert_eq!(f.run("echo bad >[ab].txt").status(), 1);
    assert_eq!(fs::read(f.root.0.join("a.txt")).unwrap(), b"");
}

#[test]
fn assignment_redirects_and_file_substitutions_preserve_contents_and_status() {
    let mut f = Fixture::new();
    fs::write(f.root.0.join("a"), "keep\n\n").unwrap();
    fs::write(f.root.0.join("b"), "truncate").unwrap();
    assert_eq!(
        f.run("out=a; out=b >\"$out\"; printf '<%s>' \"$(<a)\"")
            .status(),
        0
    );
    assert_eq!(fs::read(f.root.0.join("a")).unwrap(), b"keep\n\n");
    assert_eq!(fs::read(f.root.0.join("b")).unwrap(), b"");
    assert_eq!(f.out(), "<keep>");
    assert_eq!(f.run("v=$(<missing) || echo read-failed; dest='a b'; echo bad >$dest || echo recovered; echo done").status(), 0);
    assert_eq!(f.out(), "<keep>read-failed\nrecovered\ndone\n");
    assert!(f.err().contains("ambiguous redirect"));
    assert!(!f.root.0.join("a b").exists());
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
fn source_searches_session_path_before_cwd_and_accepts_nonexecutables() {
    let mut f = Fixture::new();
    fs::create_dir(f.root.0.join("lib")).unwrap();
    fs::write(f.root.0.join("settings"), "value=cwd").unwrap();
    fs::write(f.root.0.join("lib/settings"), "value=$1; return 6").unwrap();
    fs::write(f.root.0.join("fallback"), "value=fallback").unwrap();
    assert_eq!(f.run("PATH=lib; . settings path; echo \"$?:$value\"; source -- ./settings; echo $value; . fallback; echo $value").status(), 0);
    assert_eq!(f.out(), "6:path\ncwd\nfallback\n");
}

#[test]
fn exec_commits_descriptors_and_finishes_only_the_current_session() {
    let mut f = Fixture::new();
    let child = quote(helper());
    f.run("trap 'echo cleanup:$?' EXIT; exec >log 2>&1");
    f.run("f(){ exec >other; echo inner; }; f >temporary; (exec >sub; echo sub); echo after >&2");
    let outcome = f.run(&format!("exec {child} status 127; echo bad"));
    assert_eq!(outcome, Outcome::Exit(127));
    assert_eq!(f.shell.finish(outcome.status(), &mut f.io).unwrap(), 127);
    assert_eq!(fs::read(f.root.0.join("log")).unwrap(), b"after\n");
    assert_eq!(fs::read(f.root.0.join("other")).unwrap(), b"inner\n");
    assert_eq!(fs::read(f.root.0.join("sub")).unwrap(), b"sub\n");
    assert_eq!(fs::read(f.root.0.join("temporary")).unwrap(), b"");
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
fn here_documents_feed_native_children_and_background_jobs() {
    let mut f = Fixture::new();
    f.shell
        .set_job_executable(env!("CARGO_BIN_EXE_winsh").into());
    let child = quote(helper());
    let payload = "large payload 🦀 ".repeat(18_000);
    let script = format!(
        "case ok in ok) {child} copy <<'DOC' >copied;; esac &\n{payload}\nDOC\nwait \"$!\""
    );
    assert_eq!(f.run(&script).status(), 0, "{}", f.err());
    assert_eq!(
        fs::read_to_string(f.root.0.join("copied")).unwrap(),
        format!("{payload}\n")
    );
    assert_eq!(f.run("cat <<DOC 2>error <missing\nbody\nDOC").status(), 1);
    assert!(
        fs::read_to_string(f.root.0.join("error"))
            .unwrap()
            .contains("missing")
    );
    assert_eq!(
        f.run("cat <missing <<DOC\n$(echo ran >marker)\nDOC")
            .status(),
        1
    );
    assert!(!f.root.0.join("marker").exists());
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
fn aliases_preserve_words_self_references_and_session_scope() {
    let mut f = Fixture::new();
    assert_eq!(
        f.run(r#"alias echo='echo prefix' two='echo "two words"' say='two'"#)
            .status(),
        0
    );
    f.run(r#"say 'extra arg' | cat; \echo plain; 'echo' plain; name=echo; "$name" plain"#);
    assert_eq!(f.out(), "prefix two words extra arg\nplain\nplain\nplain\n");
    f.run("alias say; (unalias echo; say); say");
    assert!(
        f.out()
            .ends_with("alias say='two'\ntwo words\nprefix two words\n")
    );
    f.run(r#"alias value='printf "<%s>\n" "$later"'; later='first run'; value; later=next; value"#);
    assert!(f.out().ends_with("<first run>\n<next>\n"));
    f.run("alias a=b b=a; a() { echo cycle-ended; }; a; unalias -a; echo done");
    assert!(f.out().ends_with("prefix cycle-ended\ndone\n"));
    assert_eq!(f.run("alias say").status(), 1);
}

#[test]
fn simultaneous_first_sessions_share_a_complete_starter_profile() {
    let root = TempDir::new();
    let config = if cfg!(windows) {
        "APPDATA"
    } else {
        "XDG_CONFIG_HOME"
    };
    let profile = root.0.join("winsh/winshrc");
    let result = Command::new(env!("CARGO_BIN_EXE_winsh"))
        .env(config, &root.0)
        .args(["-c", "true"])
        .output()
        .unwrap();
    assert!(result.status.success() && !profile.exists());
    let start = std::sync::Barrier::new(2);
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| {
                let mut shell = Shell::with_environment(
                    root.0.clone(),
                    [
                        (config.into(), root.0.clone().into_os_string()),
                        ("HOME".into(), root.0.clone().into_os_string()),
                        ("GIT_DIR".into(), root.0.join("no-repo").into_os_string()),
                    ],
                )
                .unwrap();
                let mut streams = ShellIo::inherited();
                start.wait();
                assert_eq!(
                    shell
                        .load_interactive_profile(&mut streams)
                        .unwrap()
                        .status(),
                    0
                );
                let prompt = shell.render_prompt(false, &mut streams).unwrap();
                assert!(prompt.visible.ends_with("   ~  "), "{}", prompt.visible);
                assert!(prompt.display.contains("\x1b[48;2;59;133;134m"));
            });
        }
    });
    assert_eq!(
        fs::read_to_string(profile).unwrap(),
        include_str!("../winshrc")
    );
}

#[test]
fn powerline_prompt_tracks_worktree_branch_and_quotes_names_as_text() {
    let mut f = Fixture::new();
    let root = &f.root.0;
    let work = root.join("project%F{red}");
    let sub = work.join("src");
    fs::create_dir_all(&sub).unwrap();
    fs::create_dir(root.join("metadata")).unwrap();
    fs::write(work.join(".git"), "gitdir: ../metadata\n").unwrap();
    let head = root.join("metadata/HEAD");
    fs::write(&head, "ref: refs/heads/feature/%n\n").unwrap();
    let config = if cfg!(windows) {
        "APPDATA"
    } else {
        "XDG_CONFIG_HOME"
    };
    let setup = format!(
        "export {config}={} HOME={}; unset GIT_DIR; cd {}",
        quote(root),
        quote(root),
        quote(&sub)
    );
    assert_eq!(f.run(&setup).status(), 0);
    f.shell.load_interactive_profile(&mut f.io).unwrap();
    let prompt = f.shell.render_prompt(false, &mut f.io).unwrap();
    assert!(
        prompt
            .visible
            .ends_with("  ~/project%F{red}/src    feature/%n  "),
        "{}",
        prompt.visible
    );
    assert!(prompt.display.contains("/\x1b[1msrc\x1b[22m "));
    assert!(prompt.display.contains("\x1b[48;2;123;168;118m"));
    fs::write(&head, "ffbbccdd01234567890123456789012345678901\n").unwrap();
    assert!(
        f.shell
            .render_prompt(false, &mut f.io)
            .unwrap()
            .visible
            .ends_with(" ffbbccdd  ")
    );
    assert!(f.err().is_empty(), "{}", f.err());
}

#[test]
fn prompt_expands_session_state_and_preserves_the_previous_command_status() {
    let mut f = Fixture::new();
    let config = if cfg!(windows) {
        "APPDATA"
    } else {
        "XDG_CONFIG_HOME"
    };
    let home = quote(&f.root.0);
    f.run(&format!("export {config}={home} HOME={home}"));
    f.shell.load_interactive_profile(&mut f.io).unwrap();
    assert_eq!(f.run("PROMPT2='more> '; mkdir sub; cd sub; label() { printf '(%s)' \"$?\"; }; PS1='%~ %1~ %d $(label) %(?.ok.%F{red}bad-%?%f) %% '; false").status(), 1);
    let prompt = f.shell.render_prompt(false, &mut f.io).unwrap();
    let directory = &f.root.0.join("sub");
    let prefix = format!(
        "{} sub {} (1) ",
        Path::new("~").join("sub").display(),
        directory.display()
    );
    assert_eq!(prompt.display, format!("{prefix}\x1b[31mbad-1\x1b[39m % "));
    assert_eq!(prompt.visible, format!("{prefix}bad-1 % "));
    assert_eq!(f.shell.last_status(), 1);
    assert_eq!(
        f.shell.render_prompt(true, &mut f.io).unwrap().visible,
        "more> "
    );
    f.run("precmd() { PROMPT=\"last=$?\"; }; false");
    f.shell.run_precmd(&mut f.io).unwrap();
    assert_eq!(
        f.shell.render_prompt(false, &mut f.io).unwrap().visible,
        "last=1"
    );
    assert_eq!(f.shell.last_status(), 1);
    f.run("unset -f precmd; unsetopt promptsubst; PROMPT='$(label) %?'; false");
    assert_eq!(
        f.shell.render_prompt(false, &mut f.io).unwrap().display,
        "$(label) 1"
    );
    f.run("echo \"$PS1\" \"$PS2\"");
    assert_eq!(f.out(), "$(label) %? more> \n");
    assert!(f.err().is_empty());
}

#[test]
fn personal_profile_can_remove_the_default_and_only_loads_at_the_prompt() {
    let mut f = Fixture::new();
    let config_var = if cfg!(windows) {
        "APPDATA"
    } else {
        "XDG_CONFIG_HOME"
    };
    let dir = f.root.0.join("winsh");
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("winshrc"),
        "unalias ls\nalias greet='echo from-profile'\n",
    )
    .unwrap();
    let profile_root = f.root.0.clone();
    f.run(&format!("export {config_var}={}", quote(&profile_root)));
    assert_eq!(
        f.shell
            .load_interactive_profile(&mut f.io)
            .unwrap()
            .status(),
        0
    );
    assert_eq!(f.run("alias ls").status(), 1);
    f.run("greet");
    assert_eq!(f.out(), "from-profile\n");
    assert_eq!(
        fs::read_to_string(dir.join("winshrc")).unwrap(),
        "unalias ls\nalias greet='echo from-profile'\n"
    );
    let result = Command::new(env!("CARGO_BIN_EXE_winsh"))
        .env(config_var, &profile_root)
        .args(["-c", "alias"])
        .output()
        .unwrap();
    assert!(result.status.success() && result.stdout.is_empty() && result.stderr.is_empty());
}

#[test]
fn ls_flags_override_alias_defaults_and_keep_redirected_output_plain() {
    let mut f = Fixture::new();
    f.run("mkdir listing; cd listing; mkdir Downloads Public; touch aaa.log picture.png");
    assert_eq!(
        f.run("alias ls='ls --colors --icons --group-directories-first'; ls")
            .status(),
        0
    );
    assert_eq!(f.out(), "Downloads\nPublic\naaa.log\npicture.png\n");
    f.run("ls --color=always --icons=always >../styled");
    let styled = fs::read_to_string(f.root.0.join("styled")).unwrap();
    assert!(styled.contains("\x1b[34m\u{f024d}\x1b[0m  \x1b[1;34mDownloads\x1b[0m"));
    assert!(styled.contains("\u{f115}\x1b[0m  \x1b[1;34mPublic"));
    assert!(styled.contains("\u{f18d}  aaa.log"));
    assert!(styled.contains("\x1b[35mpicture.png\x1b[0m"));
    f.run("touch Public/child; ls --icons=always --color=never -d Public >../folder");
    assert_eq!(
        fs::read_to_string(f.root.0.join("folder")).unwrap(),
        "\u{e5ff}  Public\n"
    );
    f.run("ls --icons=always --icons=never --color=always --colors=never | cat >../plain");
    assert_eq!(fs::read_to_string(f.root.0.join("plain")).unwrap(), f.out());
    assert_eq!(f.run("ls --icons=invalid").status(), 2);
}

#[test]
fn ls_lists_session_paths_without_path_tools() {
    let mut f = Fixture::new();
    let work = f.root.0.join("work");
    fs::create_dir(&work).unwrap();
    fs::create_dir(work.join("sub")).unwrap();
    for name in [".hidden", "-dash", "spaced file", "sub/child"] {
        fs::write(work.join(name), b"").unwrap();
    }
    assert_eq!(f.run("PATH=''; cd work; ls | cat").status(), 0);
    assert_eq!(f.out(), "-dash\nspaced file\nsub\n");
    assert_eq!(f.run("ls -A1 >../almost").status(), 0);
    assert_eq!(
        fs::read(f.root.0.join("almost")).unwrap(),
        b"-dash\n.hidden\nspaced file\nsub\n"
    );
    assert_eq!(f.run("ls -a sub >../all").status(), 0);
    assert_eq!(fs::read(f.root.0.join("all")).unwrap(), b".\n..\nchild\n");
    assert_eq!(f.run("ls -d -- sub -dash >../dirs").status(), 0);
    assert_eq!(fs::read(f.root.0.join("dirs")).unwrap(), b"-dash\nsub\n");
    assert_eq!(f.run("ls missing sub 'spaced file' >../mixed").status(), 2);
    assert_eq!(
        fs::read(f.root.0.join("mixed")).unwrap(),
        b"spaced file\n\nsub:\nchild\n"
    );
    assert!(f.err().contains("missing"));
    assert_eq!(f.run("ls -Z").status(), 2);
}

#[test]
fn cat_streams_binary_data_and_numbers_across_operands() {
    let mut f = Fixture::new();
    fs::create_dir(f.root.0.join("work")).unwrap();
    assert_eq!(f.run("cd work; PATH=''").status(), 0);
    let dir = f.shell.cwd().to_owned();
    let binary: Vec<u8> = (0..=255).cycle().take(1_000_000).collect();
    fs::write(dir.join("part one"), &binary).unwrap();
    fs::write(dir.join("-tail"), b"\0\xff\r\n").unwrap();
    fs::write(dir.join("stdin"), b"middle").unwrap();
    assert_eq!(
        f.run("cat -- 'part one' - -tail <stdin | cat >joined")
            .status(),
        0
    );
    assert_eq!(
        fs::read(dir.join("joined")).unwrap(),
        [binary.as_slice(), b"middle\0\xff\r\n"].concat()
    );
    fs::write(dir.join("first"), b"\npartial").unwrap();
    fs::write(dir.join("second"), b" line\n\nlast").unwrap();
    assert_eq!(f.run("cat -nb first second").status(), 0);
    assert_eq!(f.out(), "\n     1\tpartial line\n\n     2\tlast");
    assert_eq!(f.run("cat -n first >numbered").status(), 0);
    assert_eq!(
        fs::read(dir.join("numbered")).unwrap(),
        b"     1\t\n     2\tpartial"
    );
    assert_eq!(f.run("cat missing second >rest").status(), 1);
    assert_eq!(fs::read(dir.join("rest")).unwrap(), b" line\n\nlast");
    assert!(f.err().contains("missing"));
    assert_eq!(f.run("cat -X").status(), 1);
}

#[test]
fn recursive_grep_handles_files_directories_stdin_and_statuses() {
    let mut f = Fixture::new();
    fs::create_dir_all(f.root.0.join("logs/nested")).unwrap();
    fs::write(f.root.0.join("logs/nested/-"), b"no\nERROR deep\r\n").unwrap();
    fs::write(f.root.0.join("logs/.hidden"), b"error first\n").unwrap();
    fs::write(f.root.0.join("logs/empty"), b"").unwrap();
    assert_eq!(f.run("grep -rin error logs").status(), 0, "{}", f.err());
    assert_eq!(
        f.out(),
        "logs/.hidden:1:error first\nlogs/nested/-:2:ERROR deep\r\n"
    );
    assert_eq!(f.run("cd logs; grep -ric error >../counts").status(), 0);
    assert_eq!(
        fs::read(f.root.0.join("counts")).unwrap(),
        b".hidden:1\nempty:0\nnested/-:1\n"
    );
    assert_eq!(
        f.run("echo piped | grep -rh -e piped -e ERROR - nested >../mixed")
            .status(),
        0
    );
    assert_eq!(
        fs::read(f.root.0.join("mixed")).unwrap(),
        b"piped\nERROR deep\r\n"
    );
    assert_eq!(f.run("grep -r missing-pattern .").status(), 1);
    assert_eq!(f.run("grep -r ERROR absent nested >../found").status(), 2);
    assert_eq!(
        fs::read(f.root.0.join("found")).unwrap(),
        b"nested/-:ERROR deep\r\n"
    );
    assert_eq!(f.run("grep -rq ERROR absent nested").status(), 0);
    assert_eq!(f.run("grep -r ERROR nested/- >../single").status(), 0);
    assert_eq!(
        fs::read(f.root.0.join("single")).unwrap(),
        b"ERROR deep\r\n"
    );
}

#[test]
fn grep_selects_lines_counts_files_and_preserves_statuses() {
    let mut f = Fixture::new();
    fs::write(f.root.0.join("one"), b"ok\r\nError 42\r\nwarning\nError+").unwrap();
    fs::write(f.root.0.join("two"), b"error 3\n\xfferror 4\n").unwrap();
    assert_eq!(
        f.run("grep -inE '^error [[:digit:]]+$' one two").status(),
        0
    );
    assert_eq!(f.out(), "one:2:Error 42\r\ntwo:1:error 3\n");
    assert_eq!(
        f.run("grep -ivc -e error -e warning one two >counts")
            .status(),
        0
    );
    assert_eq!(
        fs::read(f.root.0.join("counts")).unwrap(),
        b"one:1\ntwo:0\n"
    );
    assert_eq!(f.run("grep 'Error+' one >basic").status(), 0);
    assert_eq!(fs::read(f.root.0.join("basic")).unwrap(), b"Error+\n");
    assert_eq!(f.run("grep -F '[abc]' one").status(), 1);
    assert_eq!(f.run("grep -F error two >bytes").status(), 0);
    assert_eq!(
        fs::read(f.root.0.join("bytes")).unwrap(),
        b"error 3\n\xfferror 4\n"
    );
    assert_eq!(f.run("grep -c absent one >count").status(), 1);
    assert_eq!(fs::read(f.root.0.join("count")).unwrap(), b"0\n");
    assert_eq!(f.run("grep -q Error missing one").status(), 0);
    assert_eq!(f.run("grep -q absent missing one").status(), 2);
    assert_eq!(f.run("grep -h Error missing one >rest").status(), 2);
    assert_eq!(
        fs::read(f.root.0.join("rest")).unwrap(),
        b"Error 42\r\nError+\n"
    );
    for source in [
        "grep",
        "grep -e",
        "grep -Z word",
        "grep '[' one",
        "grep -EF x one",
        "grep Error one >&-",
    ] {
        assert_eq!(f.run(source).status(), 2, "{source}");
    }
}

#[test]
fn streaming_utilities_reject_their_own_output() {
    let mut f = Fixture::new();
    fs::write(f.root.0.join("input"), b"").unwrap();
    fs::hard_link(f.root.0.join("input"), f.root.0.join("alias")).unwrap();
    // Run in a bounded child so a regression cannot hang the suite.
    let cwd = quote(&f.root.0);
    for (source, status) in [
        ("cat alias >>input", 1),
        ("cat <alias >>input", 1),
        ("grep match alias >>input", 2),
    ] {
        let output = cli_bounded(&format!("cd {cwd}; {source}"));
        assert_eq!(output.status.code(), Some(status));
        assert!(String::from_utf8_lossy(&output.stderr).contains("also the output file"));
        assert_eq!(fs::read(f.root.0.join("input")).unwrap(), b"");
    }
    fs::write(f.root.0.join("input"), b"match\n").unwrap();
    assert_eq!(f.run("grep -q match input >>alias").status(), 0);
}

#[test]
#[ignore = "set WINSH_TEST_OTHER_VOLUME to a writable directory on a second filesystem"]
fn mv_between_real_filesystems() {
    let mut f = Fixture::new();
    let other =
        tempfile::tempdir_in(env::var_os("WINSH_TEST_OTHER_VOLUME").expect("second volume"))
            .unwrap();
    let probe = f.root.0.join("probe");
    fs::write(&probe, b"probe").unwrap();
    let error =
        fs::rename(probe, other.path().join("probe")).expect_err("choose different filesystems");
    assert_eq!(error.kind(), std::io::ErrorKind::CrossesDevices);
    let dest = quote(other.path());
    assert_eq!(
        f.run("mkdir -p tree/sub; echo moved >tree/sub/file; echo new >plain")
            .status(),
        0
    );
    fs::write(other.path().join("plain"), b"old").unwrap();
    assert_eq!(
        f.run(&format!("mv tree plain {dest}")).status(),
        0,
        "{}",
        f.err()
    );
    assert!(!f.root.0.join("tree").exists() && !f.root.0.join("plain").exists());
    assert_eq!(
        fs::read(other.path().join("tree/sub/file")).unwrap(),
        b"moved\n"
    );
    assert_eq!(fs::read(other.path().join("plain")).unwrap(), b"new\n");
}

#[test]
fn everyday_file_operations_work_with_session_paths_and_multiple_operands() {
    let mut f = Fixture::new();
    fs::create_dir(f.root.0.join("work")).unwrap();
    let data = b"original\0\xff\r\n";
    fs::write(f.root.0.join("work/input"), data).unwrap();
    for cmd in [
        "cd work; export PATH=''",
        "mkdir -p 'space dir/nested' backup",
        "mkdir -p 'space dir/nested'",
        "touch input --literal 'space dir/nested/empty'",
        "cp input 'space dir/renamed'",
        "printf '%s' 'replace longer contents' >target",
        "cp input target",
        "cp -- input --literal backup",
        "cp -R 'space dir' backup",
        "cp -r 'space dir/.' backup",
        "mv 'space dir/renamed' backup/empty-name",
        "mv backup/empty-name target",
        "mv target TARGET",
        "touch -- -dash",
        "mv -- -dash --literal 'space dir'",
        "rm -- 'space dir/-dash' 'space dir/--literal'",
        "rm -rf 'space dir' missing",
    ] {
        assert_eq!(f.run(cmd).status(), 0, "{cmd}: {}", f.err());
    }
    let work = f.root.0.join("work");
    for name in [
        "input",
        "TARGET",
        "backup/input",
        "backup/space dir/renamed",
        "backup/renamed",
    ] {
        assert_eq!(fs::read(work.join(name)).unwrap(), data, "{name}");
    }
    for name in [
        "backup/--literal",
        "backup/space dir/nested/empty",
        "backup/nested/empty",
    ] {
        assert!(work.join(name).is_file(), "{name}");
    }
    assert!(!work.join("space dir").exists());
    assert!(!work.join("backup/empty-name").exists());
    assert!(f.out().is_empty() && f.err().is_empty());
}

#[test]
fn touch_preserves_contents_and_updates_only_the_requested_times() {
    use std::fs::FileTimes;
    use std::time::{Duration, UNIX_EPOCH};
    let mut f = Fixture::new();
    let path = f.root.0.join("data");
    fs::write(&path, b"keep").unwrap();
    let old = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    let reset = || {
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_accessed(old).set_modified(old))
            .unwrap()
    };
    reset();
    assert_eq!(f.run("touch -a data").status(), 0, "{}", f.err());
    let meta = fs::metadata(&path).unwrap();
    assert!(meta.accessed().unwrap() > old);
    assert_eq!(meta.modified().unwrap(), old);
    reset();
    assert_eq!(f.run("touch -m data").status(), 0);
    let meta = fs::metadata(&path).unwrap();
    assert_eq!(meta.accessed().unwrap(), old);
    assert!(meta.modified().unwrap() > old);
    assert_eq!(fs::read(&path).unwrap(), b"keep");
    assert_eq!(
        f.run("touch -c missing missing-dir/ && mkdir dir && touch dir")
            .status(),
        0,
        "{}",
        f.err()
    );
    assert!(!f.root.0.join("missing").exists());
}

#[test]
fn file_operations_reject_unsafe_targets_and_report_operand_errors() {
    let mut f = Fixture::new();
    fs::write(f.root.0.join("keep"), b"unchanged").unwrap();
    fs::hard_link(f.root.0.join("keep"), f.root.0.join("alias")).unwrap();
    assert_eq!(f.run("mkdir tree; touch tree/marker").status(), 0);
    for cmd in [
        "cp keep keep",
        "cp keep alias",
        "mv keep alias",
        "cp tree tree-copy",
        "cp -r tree tree/sub",
        "mv tree tree/sub",
        "cp keep alias missing",
        "mv keep alias missing",
        "rm tree",
        "rm keep/",
        "rm -r tree/.",
        "rm -r tree/..",
        "rm -rf ''",
        "mkdir tree",
        "mkdir absent/sub",
        "touch absent/sub",
        "mv -z keep lost",
        "mkdir",
        "touch",
        "cp",
        "cp keep",
        "mv",
        "mv keep",
        "rm",
    ] {
        assert_ne!(f.run(cmd).status(), 0, "{cmd}");
    }
    assert_eq!(fs::read(f.root.0.join("keep")).unwrap(), b"unchanged");
    assert!(f.root.0.join("tree/marker").exists());
    assert!(!f.root.0.join("tree/sub").exists());
    assert_eq!(
        f.run("mkdir tree created; echo $?; rm missing created-file 2>diagnostic")
            .status(),
        1
    );
    assert!(f.root.0.join("created").is_dir());
    assert_eq!(f.out(), "1\n");
    assert!(
        fs::read_to_string(f.root.0.join("diagnostic"))
            .unwrap()
            .contains("missing")
    );
    assert_eq!(f.run("rm -f").status(), 0);
    assert_eq!(f.run("cp missing keep created").status(), 1);
    assert_eq!(
        fs::read(f.root.0.join("created/keep")).unwrap(),
        b"unchanged"
    );
    assert_eq!(f.run("mv missing alias created").status(), 1);
    assert!(f.root.0.join("created/alias").is_file());
}

#[test]
fn recursive_operations_respect_directory_links() {
    let mut f = Fixture::new();
    let root = &f.root.0;
    fs::create_dir(root.join("real")).unwrap();
    fs::write(root.join("real/keep"), b"keep").unwrap();
    fs::create_dir(root.join("tree")).unwrap();
    let link = root.join("tree/link");
    let second = root.join("tree/second");
    #[cfg(unix)]
    for path in [&link, &second] {
        std::os::unix::fs::symlink(root.join("real"), path).unwrap();
    }
    #[cfg(windows)]
    {
        // Junctions exercise Windows directory reparse points without needing
        // the privilege required to create symbolic links.
        let result = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference='Stop'; foreach ($path in $env:WINSH_TEST_LINK, $env:WINSH_TEST_SECOND) { New-Item -ItemType Junction -Path $path -Target $env:WINSH_TEST_TARGET | Out-Null }"])
            .env("WINSH_TEST_LINK", &link).env("WINSH_TEST_SECOND", &second).env("WINSH_TEST_TARGET", root.join("real")).output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert_eq!(f.run("cp -r real tree/link").status(), 1);
    assert!(!f.root.0.join("real/real").exists());
    assert_eq!(f.run("grep -r keep tree").status(), 1);
    assert_eq!(f.run("grep -r keep tree/link").status(), 0, "{}", f.err());
    assert_eq!(f.out(), "tree/link/keep:keep\n");
    assert_eq!(f.run("rm tree/link/").status(), 0, "{}", f.err());
    assert!(!link.exists());
    assert_eq!(f.run("rm -r tree").status(), 0, "{}", f.err());
    assert_eq!(fs::read(f.root.0.join("real/keep")).unwrap(), b"keep");
}

#[test]
fn bundled_utilities_run_in_script_files_without_path_tools() {
    let root = TempDir::new();
    let script = root.0.join("combine.sh");
    fs::write(
        &script,
        "PATH=''\ncd \"$1\"\ncat -- 'first log' - 'last log'\n",
    )
    .unwrap();
    fs::write(root.0.join("first log"), b"info start\n").unwrap();
    fs::write(root.0.join("last log"), b"ERROR finish\n").unwrap();
    let args = [script.to_str().unwrap(), root.0.to_str().unwrap()];
    let combined = cli(&args, b"Error middle\n");
    assert_eq!(combined.status.code(), Some(0));
    assert_eq!(combined.stdout, b"info start\nError middle\nERROR finish\n");
    let script = root.0.join("scan.sh");
    fs::write(&script, "PATH=''\nif grep -qi error \"$1\"; then\n  cat -- \"$1\" | grep -in '^error'\nelse\n  exit 7\nfi\n").unwrap();
    let log = root.0.join("combined");
    fs::write(&log, combined.stdout).unwrap();
    let args = [script.to_str().unwrap(), log.to_str().unwrap()];
    let filtered = cli(&args, b"");
    assert_eq!(filtered.status.code(), Some(0));
    assert_eq!(filtered.stdout, b"2:Error middle\n3:ERROR finish\n");
    assert!(filtered.stderr.is_empty());
    fs::write(&log, b"ok\n").unwrap();
    assert_eq!(cli(&args, b"").status.code(), Some(7));

    fs::write(
        &script,
        r#"
        PATH=''
        cd "$1" && mkdir -p out/one && touch out/one/log || exit 1
        echo saved >out/one/log
        cp -r out/one out/two & wait "$!" || exit 2
        mv out/two/log out/result && cat out/result && rm -rf out
    "#,
    )
    .unwrap();
    let output = cli(&[script.to_str().unwrap(), root.0.to_str().unwrap()], b"");
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"saved\n");
    assert!(!root.0.join("out").exists());
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
        "PATH=/usr/bin:/bin; cat -- input - a <a >b".to_owned(),
        r"PATH=/usr/bin:/bin; printf '\nfirst\n\nlast' | cat -bn".to_owned(),
        r"PATH=/usr/bin:/bin; printf 'ok\nError 42\nerror 3\n' | grep -inE '^error [[:digit:]]+$'"
            .to_owned(),
        r"PATH=/usr/bin:/bin; printf 'a+b\nab\naab\n' | grep -n 'a\+b'".to_owned(),
        r"PATH=/usr/bin:/bin; printf 'a+b\nab\naab\n' | grep -n 'a+b'".to_owned(),
        "PATH=/usr/bin:/bin; grep -Fc missing a".to_owned(),
        "cd missing 1>a 2>&1-".to_owned(),
        ">a".to_owned(),
        "mkdir lib; printf 'echo from-path:$1; return 3\\n' >lib/library; printf 'echo from-cwd\\n' >library; PATH=lib; . library arg; echo $?; source -- ./library".to_owned(),
        "exec >b 2>&1; echo start; { exec >a; echo inside; } >input; (exec >a; echo sub); echo after >&2".to_owned(),
        "v=outer; f() { local v=inner; trap 'echo \"$?:$v:$1\"' EXIT; exit 3 >b; }; f arg >a".to_owned(),
        "printf '# empty\\n' >a; false; . ./a; echo $?; printf 'echo $?\\n' >a; false; . ./a".to_owned(),
        format!("cp {child} probe.exe; PATH=.; command -v ./probe.exe probe.exe"),
        format!("trap 'echo bad' EXIT; exec {child} both >a 2>b; echo bad"),
        format!("found=$(command -v {child}) && command \"$found\" status 7"),
        "out=a; out=b >\"$out\"; printf '<%s>\\n' \"$(<a)\" \"$(0<b)\"".to_owned(),
        "out=a; out=b printf data >\"$out\"; echo \"$out\"".to_owned(),
        "printf 'first\\nsecond\\n\\n' >a; f=a; v=$(<\"$f\"); printf '<%s>\\n' \"$v\" \"$(<a; :)\""
            .to_owned(),
        "f='a b'; { echo bad >$f || echo recovered; } 2>&-; echo done".to_owned(),
        "{ v=$(<missing); } 2>&- || echo read-failed".to_owned(),
        format!("unset x; export x; {child} env-is-set x || echo absent; x=passed; {child} env x"),
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
