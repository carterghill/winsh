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
            "f(){ :; }; command -v if printf f missing_winsh_command; echo $?; command -v missing_winsh_command || echo absent; echo(){ printf shadow; }; command -- echo actual",
            "if\nprintf\nf\n0\nabsent\nactual\n",
            0,
        ),
        (
            "set -- -ab -cfoo -c 'two words' -x -- tail; while getopts :abc: o; do echo \"$o:$OPTIND:${OPTARG-unset}\"; done; echo \"$o:$OPTIND:${OPTARG-unset}\"",
            "a:1:unset\nb:2:unset\nc:3:foo\nc:5:two words\n?:6:x\n?:7:unset\n",
            0,
        ),
        (
            "getopts ':ab:' o -b; echo \"$?:$o:$OPTIND:$OPTARG\"; OPTIND=1; getopts a o -a; echo \"$o:$OPTIND:${OPTARG-unset}\"; OPTIND=1; OPTERR=0; getopts a o -z; echo \"$?:$o:${OPTARG-unset}\"",
            "0:::2:b\na:2:unset\n0:?:unset\n",
            0,
        ),
        (
            "set -- -abc; getopts abc o; f(){ local OPTIND=1; getopts xy c -xy; }; f; getopts abc o; echo $o:$OPTIND; OPTIND=1; getopts abc o; echo $o:$OPTIND",
            "b:1\na:1\n",
            0,
        ),
        (
            "set -- -ab -cd; export OPTIND=2 OPTERR=0; getopts abcd o; { getopts abcd o; echo $o:$OPTIND:$OPTERR; } & wait",
            "d:3:0\n",
            0,
        ),
        (
            "trap 'echo exit:$?' EXIT; (trap 'echo sub:$?' EXIT; exit 7); x=$(trap 'echo C:$?' EXIT; exit 2); echo \"$?:$x\"; exit 3",
            "sub:7\n2:C:2\nexit:3\n",
            3,
        ),
        (
            "trap 'echo prior:$?; exit 14' EXIT; exit 7",
            "prior:7\n",
            14,
        ),
        (
            "trap 'echo cleanup:$?' EXIT; { exec winsh_missing_exec_target; } 2>&-; echo bad",
            "cleanup:127\n",
            127,
        ),
        (
            "trap 'echo err:$?' ERR; false; echo status:$?; ! true; false && echo bad; f(){ false; }; f; true",
            "err:1\nstatus:1\nerr:1\n",
            0,
        ),
        (
            "set -e -o pipefail; trap 'echo failed:$?' ERR; trap 'echo exit:$?' EXIT; (exit 9) | true; echo bad",
            "failed:9\nexit:9\n",
            9,
        ),
        (
            "trap 'echo caught:$?' INT TERM; kill -INT $$; kill -TERM $$; trap '' TERM; kill -TERM $$; echo ok",
            "caught:0\ncaught:0\nok\n",
            0,
        ),
        (
            "trap 'echo cleanup' EXIT; saved=$(trap -p EXIT); trap - EXIT; echo \"$saved\"; trap -p EXIT",
            "trap -- 'echo cleanup' EXIT\n",
            0,
        ),
        ("set -euo pipefail; false; echo continued", "", 1),
        (
            "set -e; f(){ return 3; }; if f; then :; else echo \"$?\"; fi; ! ! false; echo continued",
            "3\n",
            1,
        ),
        (
            "set -u; set --; printf '<%s>\\n' \"${@-none}\" \"${*:-none}\"; unset v; echo \"${v:-'a}b'}\"",
            "<none>\n<none>\n'a}b'\n",
            0,
        ),
        (
            "set -e; f() { false; echo tested; }; if f; then :; fi; ! true; false && echo no; while false; do echo no; done; { false && :; }; echo end",
            "tested\nend\n",
            0,
        ),
        (
            "set -eu; unset x; set --; echo \"${x-default}:$#:$*:${#@}\"; echo \"$(false; echo sub)\"; set +e; false; set +u; echo \"<$x>\"",
            "default:0::0\nsub\n<>\n",
            0,
        ),
        (
            "set -o pipefail; (exit 7) | (exit 4) | true; echo \"$?\"; set +o pipefail; false | true; echo \"$?\"",
            "4\n0\n",
            0,
        ),
        ("set -e -o pipefail; (exit 7) | true; echo continued", "", 7),
        (
            "set -o pipefail; (exit 9) | true & pid=$!; wait \"$pid\" || echo \"$?\"; (set +o pipefail; false | true) && echo isolated; false | true",
            "9\nisolated\n",
            1,
        ),
        (
            "set -eu; (set +eu; false; echo \"${unbound:-sub}\"); set --; echo \"$((1 || absent)):$#\"; case $- in *e*u*) echo flags;; esac",
            "sub\n1:0\nflags\n",
            0,
        ),
        ("set -e; f() { false && :; }; f; echo continued", "", 1),
        (
            "set -e; if false | true; then echo last; fi; set -o pipefail; if false | true; then echo bad; else echo recovered; fi; ! false; echo end",
            "last\nrecovered\nend\n",
            0,
        ),
        (
            "set -e; ! false | true; echo kept; f() { false; echo missed; }; f; echo continued",
            "kept\n",
            1,
        ),
        (
            "unset dir; [ -d \"$dir\" ] || echo absent; [ -e '' ] || echo missing; [ -d . ] && echo directory; [ '!' = '!' ]",
            "absent\nmissing\ndirectory\n",
            0,
        ),
        (
            "unset v; set -- ${v:-\"a b\"} ${v:-''} pre${v:-a' b' c}post; printf '[%s]\\n' \"$#\" \"$@\"; printf '<%s>\\n' \"${v:-'two words'}\"",
            "[4]\n[a b]\n[]\n[prea b]\n[cpost]\n<'two words'>\n",
            0,
        ),
        (
            "unset v; set -- aa 'bb cc'; set -- ${v:-\"$@\"}; printf '<%s>\\n' \"$@\"; IFS=; set -- $@; echo \"$#/$1/$2\"",
            "<aa>\n<bb cc>\n2/aa/bb cc\n",
            0,
        ),
        (
            "IFS=:; set -- aa bb; cat <<EOF\n$@/$*\nEOF\ncase \"$@\" in 'aa bb') echo space;; *) echo other;; esac; x=\"$@\"; echo \"$x\"",
            "aa bb/aa bb\nspace\naa bb\n",
            0,
        ),
        (
            "unset x; export x; x=temp :; echo \"${x-fallback}\"; f() { local x; echo \"${x-local}\"; local x=kept; local x; echo \"$x\"; }; f; echo \"${x-still-unset}\"; x=; echo \"<${x-fallback}>\"",
            "fallback\nlocal\nkept\nstill-unset\n<>\n",
            0,
        ),
        (
            "IFS=; v=; set -- $v; echo \"$#\"; set -- \"$v\" a${v}b; echo \"$#:$2\"",
            "0\n2:ab\n",
            0,
        ),
        (
            "case 'two words' in (one|'two words') echo yes;; *) echo wrong;; esac",
            "yes\n",
            0,
        ),
        (
            "p='a*'; for x in abc 'a*' 'b/2' .x; do case $x in \"$p\") echo literal;; $p) echo variable;; [a-c]/?) echo bracket;; *) echo default;; esac; done",
            "variable\nliteral\nbracket\ndefault\n",
            0,
        ),
        (
            "case a in a) echo one ;& ${unset:?skip}) echo two ;;& b) echo bad ;; a) echo three;; esac",
            "one\ntwo\nthree\n",
            0,
        ),
        (
            "false; case x in esac; echo \"$?\"; case x in y) false;; esac; echo \"$?\"; case x in x) false;; esac",
            "0\n0\n",
            1,
        ),
        ("false; case a in a) ;; esac; echo \"$?\"", "0\n", 0),
        (
            "for x in 7 A z; do case $x in [[:digit:]]) echo digit;; [![:lower:]]) echo upper;; *) echo lower;; esac; done",
            "digit\nupper\nlower\n",
            0,
        ),
        (
            "echo \"$(case yes in yes) case no in no) printf nested;; esac;; esac)\"",
            "nested\n",
            0,
        ),
        (
            "x='two words'; cat <<EOF\n\"$x\" '${x}' $(printf sub) $((2+3)) *.rs ~\n\\$x \\\\ \\\" \\q\nEOF",
            "\"two words\" 'two words' sub 5 *.rs ~\n$x \\ \\\" \\q\n",
            0,
        ),
        (
            "cat <<'E'ND | cat\n$HOME '$(never)' \\ literal\nEND\ncat <<\\END\n$HOME\nEND",
            "$HOME '$(never)' \\ literal\n$HOME\n",
            0,
        ),
        (
            "cat <<-EOF\n\t\tfirst\n \tkeep\n\tEOF\ncat <<EOF\nE\\\nOF\necho after",
            "first\n \tkeep\nafter\n",
            0,
        ),
        (
            "cat <<A <<B; cat <<C # collect in order\nfirst\nA\nsecond\nB\nthird\nC",
            "second\nthird\n",
            0,
        ),
        (
            "f() { cat <<DOC\n$1\nDOC\n}; f first; f second; false && cat <<EOF\n${unset:?skipped}\nEOF\n:",
            "first\nsecond\n",
            0,
        ),
        (
            "echo \"$(cat <<'EOF'\n) ' \" #\nEOF\n)\"; read -r value 2<<EOF 0<&2\nread value\nEOF\necho \"$value\"",
            ") ' \" #\nread value\n",
            0,
        ),
        ("cat <<EOF |\npayload\nEOF\ncat", "payload\n", 0),
        ("cat <<''\n\nprintf done", "done", 0),
        (
            "for x in - m ']'; do case $x in [a'-'z]) echo dash;; [\\]]) echo bracket;; [!z]) echo other;; esac; done",
            "dash\nother\nbracket\n",
            0,
        ),
        (
            "case a in a|${absent:?skipped}) false ;;& b) echo bad;; esac",
            "",
            1,
        ),
        (
            "for x in a b; do case $x in a) continue ;& *) echo b;; esac; done",
            "b\n",
            0,
        ),
        (
            "cat <<'END'\nbackslash\\\nEND\ncat <<EOF\nslash\\\\\nEOF",
            "backslash\\\nslash\\\n",
            0,
        ),
        (
            "cat <<$marker\nliteral delimiter\n$marker",
            "literal delimiter\n",
            0,
        ),
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
        // Send script bytes through stdin: MSYS argv decoding can consume a
        // backslash immediately before a newline in a native Windows argument.
        let mut reference = Command::new(&bash)
            .args(["--noprofile", "--norc", "-s"])
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .env("MSYS2_ARG_CONV_EXCL", "*")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        reference
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        let expected = reference.wait_with_output().unwrap();
        let actual = run(source);
        assert_eq!(
            actual.status.code(),
            expected.status.code(),
            "status: {source}"
        );
        assert_eq!(actual.stdout, expected.stdout, "stdout: {source}");
        assert_eq!(actual.stderr, expected.stderr, "stderr: {source}");
    }
    nounset_files(Some(&bash));
}

#[test]
fn nounset_stops_noninteractive_scripts_and_preserves_default_expansion() {
    nounset_files(None);
}

fn nounset_files(bash: Option<&std::ffi::OsStr>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("strict.sh");
    for expression in [
        "$missing",
        "${missing}",
        "${#missing}",
        "$((missing + 1))",
        "$1",
    ] {
        std::fs::write(&path, format!(
            "unset missing; set -u; set --; echo \"${{missing:-fallback}}\"\necho \"{expression}\"\necho continued\n"
        )).unwrap();
        let actual = Command::new(env!("CARGO_BIN_EXE_winsh"))
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(actual.stdout, b"fallback\n", "{expression}");
        assert_eq!(actual.status.code(), Some(1), "{expression}");
        assert!(
            String::from_utf8_lossy(&actual.stderr).contains("unbound variable"),
            "{expression}"
        );
        if let Some(bash) = bash {
            let expected = Command::new(bash)
                .args(["--noprofile", "--norc"])
                .env_remove("BASH_ENV")
                .env_remove("ENV")
                .current_dir(dir.path())
                .arg("strict.sh")
                .output()
                .unwrap();
            assert_eq!(actual.status.code(), expected.status.code(), "{expression}");
            assert_eq!(actual.stdout, expected.stdout, "{expression}");
            assert!(
                String::from_utf8_lossy(&expected.stderr).contains("unbound variable"),
                "{expression}"
            );
        }
    }
    for (source, code) in [
        (
            "set -u; unset missing; echo \"$missing\"; echo continued",
            127,
        ),
        ("set -u; unset missing; (echo \"$missing\")", 1),
    ] {
        let actual = run(source);
        assert_eq!(actual.status.code(), Some(code), "{source}");
        assert!(actual.stdout.is_empty());
        assert!(String::from_utf8_lossy(&actual.stderr).contains("unbound variable"));
        if let Some(bash) = bash {
            let expected = Command::new(bash)
                .args(["--noprofile", "--norc", "-c", source])
                .env_remove("BASH_ENV")
                .env_remove("ENV")
                .output()
                .unwrap();
            assert_eq!(actual.status.code(), expected.status.code(), "{source}");
        }
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
fn script_file_accepts_crlf_here_documents_in_case_arms() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("choose.sh");
    std::fs::write(
        &path,
        "\u{feff}case $1 in\r\nok) cat <<-END\r\n\tHello $2\r\n\tEND\r\n;;\r\nesac",
    )
    .unwrap();
    let actual = Command::new(env!("CARGO_BIN_EXE_winsh"))
        .arg(path)
        .args(["ok", "世界"])
        .output()
        .unwrap();
    assert!(
        actual.status.success(),
        "{}",
        String::from_utf8_lossy(&actual.stderr)
    );
    assert_eq!(actual.stdout, "Hello 世界\n".as_bytes());
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
        .write_all(b"f() {\nfor x in one two; do\necho \"$x\"\ndone\n}\nf\ncase one in\none) cat <<-END\n\tinput\n\tEND\n;;\nesac\ncat <<''\n\nexit 8\n")
        .unwrap();
    let actual = child.wait_with_output().unwrap();
    assert_eq!(actual.status.code(), Some(8));
    assert_eq!(actual.stdout, b"one\ntwo\ninput\n");
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
        "echo x & &",
        "echo x ;;",
        "case x in a||b) echo bad;; esac",
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
