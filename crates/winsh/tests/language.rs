use std::{
    env,
    io::Write,
    process::{Command, Output, Stdio},
};
use winsh::syntax::{CommandKind, parse_script};

fn run(source: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_winsh"))
        .args(["-c", source])
        .output()
        .unwrap()
}
fn cases() -> Vec<(&'static str, &'static str, i32)> {
    vec![
        (
            "n=4; echo \"$(($n+2)):$((0 && 1/0)):$((1 || 1/0))\"",
            "6:0:1\n",
            0,
        ),
        (
            "unset x; printf '<%s>\\n' \"${x:-a  b}\"; false && echo \"${x:?must not expand}\"; echo survived",
            "<a  b>\nsurvived\n",
            0,
        ),
        ("i\\\nf true; then echo yes; f\\\ni", "yes\n", 0),
        (
            "x='two words'; printf '<%s>\\n' \"$x\" $x '' $missing",
            "<two words>\n<two>\n<words>\n<>\n",
            0,
        ),
        (
            "unset absent; empty=; printf '%s/%s/%s/%s\\n' \"${absent-default}\" \"${empty-default}\" \"${empty:-default}\" \"${absent+other}\"; echo \"${made:=new}:${#made}\"",
            "default//default/\nnew:3\n",
            0,
        ),
        (
            "set -- one 'two words' ''; printf '<%s>\\n' pre\"$@\"post; printf '%s/%s/%s\\n' \"$#\" \"$1\" \"${2}\"; shift 2; printf '[%s]\\n' \"$@\"",
            "<preone>\n<two words>\n<post>\n3/one/two words\n[]\n",
            0,
        ),
        (
            "set --; printf '<%s>\\n' before \"$@\" after; printf '<%s>\\n' \"$*\"",
            "<before>\n<after>\n<>\n",
            0,
        ),
        (
            "false && echo bad || echo recovered; true || echo bad && echo yes; ! true; echo \"$?\"; ! false",
            "recovered\nyes\n1\n",
            0,
        ),
        (
            "if false; then echo bad; elif true; then echo selected; else echo bad; fi; if false; then :; fi",
            "selected\n",
            0,
        ),
        (
            "n=0; while [ \"$n\" -lt 4 ]; do n=$((n+1)); if [ \"$n\" -eq 2 ]; then continue; fi; echo \"$n\"; done; until [ \"$n\" -eq 6 ]; do n=$((n+1)); done; echo \"end=$n\"",
            "1\n3\n4\nend=6\n",
            0,
        ),
        (
            "for a in 1 2; do for b in x y; do echo \"$a$b\"; continue 2; done; echo bad; done; for a in 1 2; do for b in x y; do echo stop; break 2; done; done",
            "1x\n2x\nstop\n",
            0,
        ),
        (
            "set -- one 'two words'; for x; do printf '<%s>\\n' \"$x\"; done; for x in; do echo bad; done",
            "<one>\n<two words>\n",
            0,
        ),
        (
            "x=outer; set -- parent; f() { local x=inner; printf '%s:%s:%s\\n' \"$x\" \"$1\" \"$#\"; return 7; echo bad; }; f child extra; printf '%s:%s:%s\\n' \"$?\" \"$x\" \"$1\"",
            "inner:child:2\n7:outer:parent\n",
            0,
        ),
        (
            "x=parent; ( x=child; echo \"$x\"; exit 3 ); echo \"$?:$x\"; { x=group; }; echo \"$x\"",
            "child\n3:parent\ngroup\n",
            0,
        ),
        (
            "x=parent; value=$(x=child; printf '%s\\n\\n' \"$x\"; exit 9); echo \"$?:$x:$value\"; echo \"nested=$(echo \"$(echo inside)\")\"",
            "9:parent:child\nnested=inside\n",
            0,
        ),
        (
            "n=7; echo \"$((n * 3 + 1)):$(( (2+3) * 4 )):$((0x10+010)):$((3<4 && 4!=2))\"",
            "22:20:24:1\n",
            0,
        ),
        (
            "printf 'first\\nsecond\\n' | while IFS= read -r line; do printf '<%s>\\n' \"$line\"; done",
            "<first>\n<second>\n",
            0,
        ),
        (
            "f() { echo function; false; }; f | { read line; echo \"$line\"; }; echo \"$?\"",
            "function\n0\n",
            0,
        ),
        (
            "value='hello world'; export EXPORTED=$value; echo \"$EXPORTED\"; unset EXPORTED; echo \"${EXPORTED:-gone}\"",
            "hello world\ngone\n",
            0,
        ),
        (
            "IFS=:; value='a::b:'; printf '<%s>\\n' $value",
            "<a>\n<>\n<b>\n",
            0,
        ),
        (
            "set -- a b c d e f g h i ten; echo \"${10}:$10:$#\"",
            "ten:a0:10\n",
            0,
        ),
        ("false; # preserve status\n", "", 1),
        ("echo done; exit 23; echo bad", "done\n", 23),
    ]
}

#[test]
fn language_behaviors_have_explicit_results() {
    for (source, stdout, status) in cases() {
        let actual = run(source);
        assert_eq!(
            actual.status.code(),
            Some(status),
            "{source}: {}",
            String::from_utf8_lossy(&actual.stderr)
        );
        assert_eq!(String::from_utf8_lossy(&actual.stdout), stdout, "{source}");
        assert!(
            actual.stderr.is_empty(),
            "{source}: {}",
            String::from_utf8_lossy(&actual.stderr)
        );
    }
}

#[test]
#[ignore = "requires Bash; set WINSH_TEST_BASH to its executable path"]
fn scripts_and_language_match_bash() {
    let bash = env::var_os("WINSH_TEST_BASH").expect("set WINSH_TEST_BASH");
    for (source, _, _) in cases() {
        let expected = Command::new(&bash)
            .args(["--noprofile", "--norc", "-c", source])
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .env("MSYS2_ARG_CONV_EXCL", "*")
            .output()
            .unwrap();
        let actual = run(source);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "status: {source}"
        );
        assert_eq!(actual.stdout, expected.stdout, "stdout: {source}");
        assert_eq!(actual.stderr, expected.stderr, "stderr: {source}");
    }
}

#[test]
fn command_string_arguments_follow_bash_zero_and_positional_conventions() {
    let actual = Command::new(env!("CARGO_BIN_EXE_winsh"))
        .args([
            "-c",
            "printf '<%s>\\n' \"$0\" \"$@\"",
            "custom name",
            "",
            "two words",
        ])
        .output()
        .unwrap();
    assert!(actual.status.success());
    assert_eq!(actual.stdout, b"<custom name>\n<>\n<two words>\n");
}

#[test]
fn multiline_compound_commands_share_the_interactive_input_parser() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_winsh"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"f() {\nfor x in one two; do\necho \"$x\"\ndone\n}\nf\nexit 8\n")
        .unwrap();
    let actual = child.wait_with_output().unwrap();
    assert_eq!(actual.status.code(), Some(8));
    assert_eq!(actual.stdout, b"one\ntwo\n");
    assert!(actual.stderr.is_empty(), "{:?}", actual.stderr);
}

#[test]
fn syntax_tree_keeps_nested_compounds_and_incomplete_input() {
    let ast = parse_script(
        "if true; then for x in a b; do echo \"$x\"; done; else false; fi >out && echo yes",
    )
    .unwrap();
    assert!(matches!(
        ast.lists[0].pipelines[0].commands[0].kind,
        CommandKind::If { .. }
    ));
    for source in [
        "if true",
        "if true; then echo yes",
        "for x in a; do echo x",
        "f() {",
        "echo $(echo x",
        "true &&",
    ] {
        assert!(
            parse_script(source).unwrap_err().is_incomplete(),
            "{source}"
        );
    }
    for source in [
        "then echo x",
        "if; then :; fi",
        "echo x &",
        "echo x ;;",
        "echo x; case y in",
        "for x do echo x; done",
        "items[0]=value",
    ] {
        assert!(parse_script(source).is_err(), "{source}");
    }
}

#[test]
fn script_corpus_never_panics_and_errors_keep_valid_source_ranges() {
    let alphabet = [
        "a", " ", ";", "\n", "|", "&", "(", ")", "{", "}", "'", "\"", "$", "\\", "🦀",
    ];
    for length in 0..=4 {
        for mut number in 0..alphabet.len().pow(length) {
            let mut source = String::new();
            for _ in 0..length {
                source.push_str(alphabet[number % alphabet.len()]);
                number /= alphabet.len();
            }
            if let Err(error) = parse_script(&source) {
                assert!(
                    source.get(error.span.clone()).is_some(),
                    "{source:?}: {error}"
                );
            }
        }
    }
    let nested = format!("echo {}x{}", "$(echo ".repeat(40), ")".repeat(40));
    assert!(parse_script(&nested).is_err());
}

#[test]
fn errors_and_recursion_limits_keep_the_engine_alive() {
    for (source, status) in [
        ("shift 99", 1),
        ("break", 1),
        ("return 5", 1),
        ("[ x", 2),
        ("printf '%q' x", 2),
        ("echo $((1/0))", 2),
        ("echo ${x:1}", 2),
        ("f() { f; }; f", 2),
    ] {
        let actual = run(source);
        assert_eq!(
            actual.status.code(),
            Some(status),
            "{source}: {}",
            String::from_utf8_lossy(&actual.stderr)
        );
        assert!(!actual.stderr.is_empty(), "{source}");
    }
}
