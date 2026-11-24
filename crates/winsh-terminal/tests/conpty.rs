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
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.session.try_event() {
                Some(Event::Exited(status)) => {
                    assert_eq!(status, 0);
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
fn winsh_interactive_smoke() {
    let shell = Path::new(env!("CARGO_BIN_EXE_winsh-terminal")).with_file_name("winsh.exe");
    assert!(
        shell.is_file(),
        "build the workspace first: {}",
        shell.display()
    );
    let terminal = Arc::new(Mutex::new(Terminal::new(40, 160)));
    let session = Session::spawn(&shell, &[], 40, 160, Arc::clone(&terminal)).unwrap();
    let harness = Harness { session, terminal };
    harness.wait_for(" $ ");

    // Cancel an unfinished quoted command, then run a native foreground child.
    harness.enter("pwd 'unfinished");
    harness.wait_for("> ");
    let prompts = harness.contents().matches(" $ ").count();
    harness.session.write(vec![3]).unwrap();
    wait_for_prompt_count(&harness, prompts + 1);
    harness.enter(r#"powershell.exe -NoLogo -NoProfile -Command 'Write-Output ("foreground-" + "ready"); Start-Sleep 60'"#);
    harness.wait_for("foreground-ready");
    let prompts = harness.contents().matches(" $ ").count();
    harness.session.write(vec![3]).unwrap();
    wait_for_prompt_count(&harness, prompts + 1);
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
    let prompts = harness.contents().matches(" $ ").count();
    harness.session.write(vec![3]).unwrap();
    wait_for_prompt_count(&harness, prompts + 1);
    harness.enter(r#"powershell.exe -NoLogo -NoProfile -Command 'Write-Output ("after-pipeline-" + "interrupt")'"#);
    harness.wait_for("after-pipeline-interrupt");
    harness.enter("exit 42");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match harness.session.try_event() {
            Some(Event::Exited(status)) => {
                assert_eq!(status, 42);
                break;
            }
            Some(Event::Error(error)) => panic!("{error}"),
            _ => {}
        }
        assert!(Instant::now() < deadline, "shell did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_prompt_count(harness: &Harness, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while harness.contents().matches(" $ ").count() < count {
        assert!(
            Instant::now() < deadline,
            "no prompt after interrupt; screen: {}",
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
