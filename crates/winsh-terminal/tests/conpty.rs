#![cfg(windows)]
#![allow(unsafe_code)]

use std::{
    env,
    ffi::OsString,
    io::{self, BufRead, IsTerminal, Write},
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, WAIT_OBJECT_0},
    System::{
        Console::{
            CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
        },
        Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
    },
};
use winsh_terminal::{
    session::{Event, Session},
    terminal::Terminal,
};

struct Harness {
    session: Session,
    terminal: Arc<Mutex<Terminal>>,
}

impl Harness {
    fn new() -> Self {
        let terminal = Arc::new(Mutex::new(Terminal::new(12, 80)));
        let arguments: Vec<OsString> = [
            "--ignored",
            "--exact",
            "fixture",
            "--nocapture",
            "--test-threads=1",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let session = Session::spawn(
            &env::current_exe().unwrap(),
            &arguments,
            12,
            80,
            Arc::clone(&terminal),
        )
        .unwrap();
        let harness = Self { session, terminal };
        harness.wait_for("READY");
        harness
    }

    fn contents(&self) -> String {
        self.terminal.lock().unwrap().screen().contents()
    }

    fn shell(history: &Path) -> Self {
        Self::shell_with_profile(history, Path::new(env!("CARGO_TARGET_TMPDIR")))
    }

    fn shell_with_profile(history: &Path, appdata: &Path) -> Self {
        let shell = Path::new(env!("CARGO_BIN_EXE_winsh-terminal")).with_file_name("winsh.exe");
        assert!(
            shell.is_file(),
            "build the workspace first: {}",
            shell.display()
        );
        // Isolate this child from the developer's history and startup script.
        let args = [
            OsString::from("-c"),
            OsString::from(r#"WINSH_HISTORY_FILE="$1" APPDATA="$3" "$2""#),
            OsString::from("test"),
            history.into(),
            shell.clone().into(),
            appdata.into(),
        ];
        let terminal = Arc::new(Mutex::new(Terminal::new(40, 160)));
        let session = Session::spawn(&shell, &args, 40, 160, Arc::clone(&terminal)).unwrap();
        let harness = Self { session, terminal };
        wait_for_prompt(&harness);
        harness
    }

    fn enter(&self, command: &str) {
        self.session
            .write(format!("{command}\r").into_bytes())
            .unwrap();
    }

    fn wait_for(&self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let contents = self.contents();
            if contents.contains(text) {
                return;
            }
            while let Some(event) = self.session.try_event() {
                match event {
                    Event::Error(error) => panic!("{error}; screen: {contents}"),
                    Event::Exited(status) => {
                        panic!("exited {status} before {text:?}; screen: {contents}")
                    }
                    Event::Resized(_, _) => {}
                }
            }
            assert!(
                Instant::now() < deadline,
                "waiting for {text:?}; screen: {contents}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(&self) {
        self.session.write(b"quit\r".to_vec()).unwrap();
        self.wait_for_exit(0);
    }

    fn wait_for_exit(&self, expected: i32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.session.try_event() {
                Some(Event::Exited(status)) => {
                    assert_eq!(status, expected);
                    return;
                }
                Some(Event::Error(error)) => panic!("{error}"),
                _ => {}
            }
            assert!(Instant::now() < deadline, "session did not exit");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn conpty_provides_a_console_and_round_trips_unicode_input() {
    let harness = Harness::new();
    harness.wait_for("SIZE=80x12");
    harness
        .session
        .write("hello café 日本語 🦀\r".as_bytes().to_vec())
        .unwrap();
    harness.wait_for("RECEIVED=hello café 日本語 🦀");
    harness.finish();
}

#[test]
fn resizing_changes_the_child_console_and_terminal_grid() {
    let harness = Harness::new();
    harness.session.resize(20, 100);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match harness.session.try_event() {
            Some(Event::Resized(20, 100)) => break,
            Some(Event::Error(error)) => panic!("{error}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "resize did not complete");
        thread::sleep(Duration::from_millis(10));
    }
    harness.session.write(b"size\r".to_vec()).unwrap();
    harness.wait_for("SIZE=100x20");
    assert_eq!(harness.terminal.lock().unwrap().screen().size(), (20, 100));
    harness.finish();
}

#[test]
fn closing_a_session_terminates_its_native_descendants() {
    let harness = Harness::new();
    harness.session.write(b"spawn\r".to_vec()).unwrap();
    harness.wait_for("DESCENDANT=");
    let contents = harness.contents();
    let id: u32 = contents
        .split("DESCENDANT=")
        .nth(1)
        .unwrap()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap();
    // SAFETY: request only wait access to the fixture's descendant. The handle
    // stays valid after its process exits and is closed exactly once below.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, id) };
    assert!(!process.is_null(), "descendant must still be running");
    let start = Instant::now();
    drop(harness);
    // SAFETY: valid owned process handle; bounded wait verifies tree cleanup.
    let result = unsafe { WaitForSingleObject(process, 5_000) };
    // SAFETY: release the uniquely owned handle acquired above.
    unsafe { CloseHandle(process) };
    assert_eq!(result, WAIT_OBJECT_0);
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[test]
fn failed_process_start_returns_an_error_without_hanging() {
    let terminal = Arc::new(Mutex::new(Terminal::new(12, 80)));
    let directory = env::temp_dir(); // Exists, but cannot be executed.
    assert!(Session::spawn(&directory, &[], 12, 80, terminal).is_err());
}

#[test]
#[ignore = "requires cargo build --workspace in the same target directory"]
fn winsh_interactive_ls_uses_profile_icons_colors_and_a_column_major_grid() {
    let tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let config = tmp.path().join("winsh");
    std::fs::create_dir(&config).unwrap();
    std::fs::write(config.join("winshrc"), "\u{feff}alias show='ls'\r\n").unwrap();
    let listing = tmp.path().join("listing");
    for folder in ["Downloads", "Pictures", "Public"] {
        std::fs::create_dir_all(listing.join(folder)).unwrap();
    }
    for file in ["aaa.log", "zzz.log"] {
        std::fs::write(listing.join(file), "").unwrap();
    }
    let h = Harness::shell_with_profile(Path::new(""), tmp.path());
    h.enter(&format!(
        "cd '{}'",
        listing.display().to_string().replace('\'', "'\\''")
    ));
    h.enter("COLUMNS=25 show");
    h.wait_for("\u{f115}  Public");
    let contents = h.contents();
    assert!(
        contents.contains("\u{f024d}  Downloads  \u{f18d}  aaa.log"),
        "{contents}"
    );
    assert!(
        contents.contains("\u{f024f}  Pictures   \u{f18d}  zzz.log"),
        "{contents}"
    );
    {
        let term = h.terminal.lock().unwrap();
        let screen = term.screen();
        let row = (0..screen.size().0)
            .find(|&r| screen.cell(r, 0).unwrap().contents() == "\u{f024d}")
            .unwrap();
        assert!(matches!(
            screen.cell(row, 3).unwrap().fgcolor(),
            vt100::Color::Idx(4 | 12)
        ));
    }
    h.enter("show | cat");
    h.wait_for("\nDownloads\nPictures\nPublic\naaa.log\nzzz.log");
    h.enter("exit 0");
    h.wait_for_exit(0);
}

#[test]
#[ignore = "requires cargo build --workspace in the same target directory"]
fn winsh_interactive_smoke() {
    let harness = Harness::shell(Path::new(""));

    // Cancel an unfinished quoted command, then run a native foreground child.
    harness.enter("pwd 'unfinished");
    harness.wait_for("\n>");
    // A painted prompt can precede the next console read. Wait for input echo
    // before sending Ctrl+C so this exercises an active multiline read.
    harness.session.write(b"pending-input".to_vec()).unwrap();
    harness.wait_for("> pending-input");
    harness.session.write(vec![3]).unwrap();
    wait_for_prompt(&harness);
    harness.enter(r#"powershell.exe -NoLogo -NoProfile -Command 'Write-Output ("foreground-" + "ready"); Start-Sleep 60'"#);
    harness.wait_for("foreground-ready");
    harness.session.write(vec![3]).unwrap();
    wait_for_prompt(&harness);
    harness.enter(
        r#"powershell.exe -NoLogo -NoProfile -Command 'Write-Output ("after-" + "interrupt")'"#,
    );
    harness.wait_for("after-interrupt");
    // Interrupt both native pipeline stages, then verify another command runs.
    harness.enter(concat!(
        r#"powershell.exe -NoLogo -NoProfile -Command 'Write-Output ("pipeline-" + "ready"); Start-Sleep 60'"#,
        " | powershell.exe -NoLogo -NoProfile -Command ",
        "'while ($null -ne ($line = [Console]::ReadLine())) { [Console]::WriteLine($line) }'",
    ));
    harness.wait_for("pipeline-ready");
    harness.session.write(vec![3]).unwrap();
    wait_for_prompt(&harness);
    harness.enter(r#"powershell.exe -NoLogo -NoProfile -Command 'Write-Output ("after-pipeline-" + "interrupt")'"#);
    harness.wait_for("after-pipeline-interrupt");
    for line in [
        "check() {",
        "for label in language; do",
        r#"printf '%s-%s\n' "$label" "$1""#,
        "done",
        "}",
        "check passed",
    ] {
        harness.enter(line);
    }
    harness.wait_for("language-passed");
    harness.enter("PATH='' ls -d . | cat | grep -nF .");
    harness.wait_for("1:.");
    harness.enter(r"printf 'ok\nError from pipeline\n' | cat | grep -in '^error'");
    harness.wait_for("2:Error from pipeline");
    harness.enter("grep -n needle");
    harness.enter("needle from console");
    harness.wait_for("1:needle from console");
    harness.session.write(vec![3]).unwrap();
    wait_for_prompt(&harness);
    harness.enter(r#"printf 'utility-status-%s\n' "$?""#);
    harness.wait_for("utility-status-130");
    harness.enter("exit 42");
    harness.wait_for_exit(42);
}

#[test]
#[ignore = "requires cargo build --workspace in the same target directory"]
fn winsh_file_operations_round_trip_in_the_terminal() {
    let work = std::path::absolute(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
        .join(format!(
            "terminal-files-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    std::fs::create_dir(&work).unwrap();
    let h = Harness::shell(Path::new(""));
    h.enter(&format!(
        "cd '{}' && export PATH='' && echo cwd-ready",
        work.to_str().unwrap().replace('\'', "'\\''")
    ));
    h.wait_for("\ncwd-ready");
    h.enter(
        "mkdir -p 'project/src' && touch 'project/src/notes' && echo shipped >project/src/notes",
    );
    h.enter("cp -r project copy && mv copy/src/notes copy/done && grep -rn shipped copy");
    h.wait_for("\ncopy/done:1:shipped");
    h.enter("rm -r project copy && echo cleaned-up");
    h.wait_for("\ncleaned-up");
    h.enter("exit");
    h.wait_for_exit(0);
    std::fs::remove_dir(work).unwrap(); // Both directory trees must be gone.
}

#[test]
#[ignore = "requires cargo build --workspace in the same target directory"]
fn winsh_controls_foreground_and_background_jobs() {
    let h = Harness::shell(Path::new(""));
    h.enter(r#"powershell.exe -NoLogo -NoProfile -Command 'Write-Output ("job-" + "ready"); Start-Sleep 60' | cat"#);
    h.wait_for("\njob-ready");
    h.session.write(vec![26]).unwrap();
    h.wait_for("[1] Stopped");
    wait_for_prompt(&h);
    h.enter("bg %1");
    h.wait_for("[1] Running");
    h.enter("grep -n needle &");
    h.wait_for("[2] ");
    h.enter("echo prompt-still-works");
    h.wait_for("\nprompt-still-works");
    h.enter("fg %2");
    h.enter("needle from foreground");
    h.wait_for("1:needle from foreground");
    h.session.write(vec![3]).unwrap();
    wait_for_prompt(&h);
    h.enter("echo interrupt-$?; jobs %+ | grep -q Running && echo survivor-$?");
    h.wait_for("\ninterrupt-130");
    h.wait_for("\nsurvivor-0");
    h.enter("! true | false && echo negation-works");
    h.wait_for("\nnegation-works");
    h.enter("stop %1; kill %1; wait %1; echo killed-$?");
    h.wait_for("\nkilled-143");
    h.enter("cat -n");
    h.enter("reader-ready");
    h.wait_for("1  reader-ready");
    h.session.write(vec![26]).unwrap();
    h.wait_for("[3] Stopped");
    h.enter("bg; echo reader-in-background");
    h.wait_for("\nreader-in-background");
    h.enter("fg; echo reader-exited-$?");
    h.enter("reader-resumed");
    h.wait_for("2  reader-resumed");
    h.session.write(vec![4]).unwrap();
    h.wait_for("\nreader-exited-0");
    h.enter("exit 0");
    h.wait_for_exit(0);
}

#[test]
#[ignore = "requires cargo build --workspace in the same target directory"]
fn winsh_tab_completes_at_the_prompt() {
    let harness = Harness::shell(Path::new(""));
    // Remove host-dependent command matches and change the logical cwd.
    harness.enter("export PATH=''; cd src");
    harness.wait_for(r"winsh-terminal\src $");
    harness.enter("pri\t'%s\\n' command-completed");
    harness.wait_for("\ncommand-completed");
    harness.enter("echo input-prefix | gre\tinput");
    harness.wait_for("\ninput-prefix");
    harness.enter("echo 'sess\t");
    harness.wait_for("\nsession.rs");
    harness.enter("complete_one() { echo first-result; }; complete_two() { echo second-result; }");
    harness.session.write(b"comple\t\t".to_vec()).unwrap();
    harness.wait_for("\ncomplete_one");
    harness.enter("tw\t");
    harness.wait_for("\nsecond-result");
    harness.enter("exit");
    harness.wait_for_exit(0);
}

#[test]
#[ignore = "requires cargo build --workspace in the same target directory"]
fn winsh_history_survives_restarts() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::path::absolute(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
        .join(format!("history-{}-{nonce}", std::process::id()));
    let first = Harness::shell(&path);
    let second = Harness::shell(&path);
    first.enter("echo 'from");
    first.wait_for("\n>");
    first.enter("first'");
    first.wait_for("\nfrom\nfirst");
    second.enter("echo saved");
    second.wait_for("\nsaved");
    second.enter("echo 'discarded");
    second.wait_for("\n>");
    second.session.write(vec![3]).unwrap();
    wait_for_prompt(&second);
    // Closing the window can kill winsh without giving it a graceful exit.
    drop(first);
    drop(second);

    let fresh = Harness::shell(&path);
    fresh.session.write(b"echo draft\x1b[A".to_vec()).unwrap();
    fresh.wait_for("$ echo saved");
    fresh.session.write(b"\x1b[B".to_vec()).unwrap();
    fresh.wait_for("$ echo draft");
    fresh.session.write(b"\x1b[A again\r".to_vec()).unwrap();
    fresh.wait_for("\nsaved again");
    // Recall the original multiline entry and execute it in the new shell.
    fresh
        .session
        .write(b"\x1b[A\x1b[A\x1b[A\r".to_vec())
        .unwrap();
    fresh.wait_for("\nfrom\nfirst");
    fresh
        .session
        .write(b"echo eof-buffer\x1a".to_vec())
        .unwrap();
    fresh.wait_for("\neof-buffer");
    wait_for_prompt(&fresh);
    fresh.session.write(vec![26]).unwrap();
    fresh.wait_for_exit(0);
    drop(fresh);
    let history = std::fs::read_to_string(&path).unwrap();
    assert!(!history.contains("discarded"));
    std::fs::remove_file(path).unwrap();
}

fn wait_for_prompt(harness: &Harness) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let prompt = format!("{} $", env::current_dir().unwrap().display());
    // The terminal can trim trailing blank cells and scroll old prompts off screen.
    while !harness.contents().trim_end().ends_with(&prompt) {
        assert!(
            Instant::now() < deadline,
            "waiting for {prompt:?}; screen: {}",
            harness.contents()
        );
        thread::sleep(Duration::from_millis(10));
    }
}

// The integration-test executable doubles as its own native console fixture.
// These two entries run only in the child process, never in the normal suite.
#[test]
#[ignore = "subprocess fixture, launched by the ConPTY integration tests"]
fn fixture() {
    assert!(io::stdin().is_terminal());
    assert!(io::stdout().is_terminal());
    print_size();
    println!("READY");
    io::stdout().flush().unwrap();
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        match line.as_str() {
            "quit" => break,
            "size" => print_size(),
            "spawn" => {
                let mut child = Command::new(env::current_exe().unwrap())
                    .args(["--ignored", "--exact", "descendant_fixture", "--nocapture"])
                    .spawn()
                    .unwrap();
                let _ = child.wait();
            }
            _ => println!("RECEIVED={line}"),
        }
        io::stdout().flush().unwrap();
    }
}

fn print_size() {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: this fixture is attached to ConPTY; valid borrowed console handle
    // and initialized output structure.
    assert_ne!(
        unsafe { GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &mut info) },
        0
    );
    println!(
        "SIZE={}x{}",
        info.dwSize.X,
        info.srWindow.Bottom - info.srWindow.Top + 1
    );
}

#[test]
#[ignore = "subprocess fixture, launched by the descendant-cleanup test"]
fn descendant_fixture() {
    println!("DESCENDANT={}", std::process::id());
    io::stdout().flush().unwrap();
    thread::sleep(Duration::from_secs(60));
}
