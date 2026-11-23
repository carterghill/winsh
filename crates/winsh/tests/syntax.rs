use winsh::syntax::{
    ParseErrorKind, Quote, SimpleCommand, WordPart, WordPartKind, parse_simple_command,
};

fn command(source: &str) -> SimpleCommand {
    parse_simple_command(source).unwrap().unwrap()
}

// Only a quoting-test helper: real arguments must go through expansion before
// quote removal. The engine deliberately does not expose this flattening API.
fn quote_removed(parts: &[WordPart]) -> String {
    let mut result = String::new();
    for part in parts {
        match &part.kind {
            WordPartKind::Text(text) | WordPartKind::SingleQuoted(text) => result.push_str(text),
            WordPartKind::DoubleQuoted(parts) => result.push_str(&quote_removed(parts)),
            WordPartKind::Escaped(ch) => result.push(*ch),
        }
    }
    result
}

fn words(source: &str) -> Vec<String> {
    command(source)
        .words
        .iter()
        .map(|word| quote_removed(&word.parts))
        .collect()
}

// Literal argument fixtures; also checked against Bash by the optional test.
fn quoting_cases() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("one two\tthree", vec!["one", "two", "three"]),
        ("café 日本語 🦀", vec!["café", "日本語", "🦀"]),
        (
            "one\u{a0}two\u{b}three\u{c}four",
            vec!["one\u{a0}two\u{b}three\u{c}four"],
        ),
        ("one\rtwo", vec!["one\rtwo"]),
        (
            r#"'hello world' "hello world""#,
            vec!["hello world", "hello world"],
        ),
        (r#"ab" c"'d'ef"#, vec!["ab cdef"]),
        (r#"'' "" a""b ''"""#, vec!["", "", "ab", ""]),
        (r#"'can'\''t'"#, vec!["can't"]),
        (
            r#"'a\b $HOME `date` "quoted"'"#,
            vec![r#"a\b $HOME `date` "quoted""#],
        ),
        (
            r#"one\ two \#hash foo#bar"#,
            vec!["one two", "#hash", "foo#bar"],
        ),
        ("one\\\ttwo", vec!["one\ttwo"]),
        (r#"\$HOME \* \? \[x\]"#, vec!["$HOME", "*", "?", "[x]"]),
        (
            r#"\; \& \| \< \> \( \) \' \" \\"#,
            vec![";", "&", "|", "<", ">", "(", ")", "'", "\"", "\\"],
        ),
        (
            r#""a\"b" "\$HOME" "\`date\`" "\\""#,
            vec!["a\"b", "$HOME", "`date`", "\\"],
        ),
        (r#""\q" "\!" "\'""#, vec!["\\q", "\\!", "\\'"]),
        (r##""a'b" "#;|&<>()""##, vec!["a'b", "#;|&<>()"]),
        (
            r#"'C:\Program Files\winsh' "C:\Program Files\winsh" C:/Users/Carter C:\\temp"#,
            vec![
                r"C:\Program Files\winsh",
                r"C:\Program Files\winsh",
                "C:/Users/Carter",
                r"C:\temp",
            ],
        ),
        (r"C:\temp", vec!["C:temp"]),
        (
            "'one\ntwo' \"three\nfour\"",
            vec!["one\ntwo", "three\nfour"],
        ),
        ("'one\\\ntwo'", vec!["one\\\ntwo"]),
        ("one\\\ntwo \\\nthree", vec!["onetwo", "three"]),
        ("one\\\n two", vec!["one", "two"]),
        ("\"one\\\ntwo\"", vec!["onetwo"]),
        ("\"\\\n\"", vec![""]),
        ("\"one\\\\\ntwo\"", vec!["one\\\ntwo"]),
        ("one\\\n", vec!["one"]),
        ("one \\\n", vec!["one"]),
        ("one # ignored ' \" \\ | $(bad)", vec!["one"]),
        (
            r##"''#literal ""#also foo#bar # ignored"##,
            vec!["#literal", "#also", "foo#bar"],
        ),
        ("one \\\n# ignored", vec!["one"]),
        ("one\\\n#literal", vec!["one#literal"]),
        ("one # ignored\\\n# another comment", vec!["one"]),
        ("one\r\n", vec!["one\r"]),
        (
            r#"'${x}' '\$(x)' '$' '$[' '$"'"#,
            vec!["${x}", "\\$(x)", "$", "$[", "$\""],
        ),
    ]
}

#[test]
fn parses_literal_words_with_bash_quoting_rules() {
    for (input, expected) in quoting_cases() {
        let source = format!("capture {input}");
        assert_eq!(&words(&source)[1..], expected, "input: {input:?}");
    }
}

#[test]
fn blank_lines_comments_and_continuations_do_not_create_words() {
    for input in [
        "",
        " \t\n",
        "# ignored",
        "\\\n",
        " \\\n# ignored\n\t",
        "# first\\\n# second\n",
    ] {
        assert_eq!(parse_simple_command(input), Ok(None), "{input:?}");
    }
    assert_eq!(
        words("\n# before\n echo hello \n\n# after\n"),
        ["echo", "hello"]
    );
}

#[test]
fn preserves_quote_boundaries_escapes_and_original_byte_spans() {
    let source = "  echo a\"β\"' c'\\ d # tail";
    let parsed = command(source);
    assert_eq!(&source[parsed.span], "echo a\"β\"' c'\\ d");
    let word = &parsed.words[1];
    assert_eq!(&source[word.span.clone()], "a\"β\"' c'\\ d");
    let fragments: Vec<_> = word
        .parts
        .iter()
        .map(|part| &source[part.span.clone()])
        .collect();
    assert_eq!(fragments, ["a", "\"β\"", "' c'", "\\ ", "d"]);
    assert_eq!(word.parts[0].kind, WordPartKind::Text("a".into()));
    assert_eq!(word.parts[2].kind, WordPartKind::SingleQuoted(" c".into()));
    assert_eq!(word.parts[3].kind, WordPartKind::Escaped(' '));
    let WordPartKind::DoubleQuoted(inner) = &word.parts[1].kind else {
        panic!("expected double quotes")
    };
    assert_eq!(inner[0].kind, WordPartKind::Text("β".into()));
    assert_eq!(&source[inner[0].span.clone()], "β");
    assert_eq!(inner[0].span.len(), 2);
}

#[test]
fn keeps_empty_quotes_as_syntax() {
    let parsed = command(r#"echo '' "" a''b"#);
    assert_eq!(
        parsed.words[1].parts[0].kind,
        WordPartKind::SingleQuoted(String::new())
    );
    assert_eq!(
        parsed.words[2].parts[0].kind,
        WordPartKind::DoubleQuoted(Vec::new())
    );
    assert_eq!(parsed.words[3].parts.len(), 3);
}

#[test]
fn leaves_expansion_markers_and_their_quote_context_intact() {
    let parsed = command(r#"echo $HOME "$HOME\$HOME" '$HOME' \$HOME ~ *.rs a{b,c} $@ $?"#);
    assert_eq!(
        parsed.words[1].parts[0].kind,
        WordPartKind::Text("$HOME".into())
    );
    let WordPartKind::DoubleQuoted(inner) = &parsed.words[2].parts[0].kind else {
        panic!("expected double quotes")
    };
    assert_eq!(inner[0].kind, WordPartKind::Text("$HOME".into()));
    assert_eq!(inner[1].kind, WordPartKind::Escaped('$'));
    assert_eq!(inner[2].kind, WordPartKind::Text("HOME".into()));
    assert_eq!(
        parsed.words[3].parts[0].kind,
        WordPartKind::SingleQuoted("$HOME".into())
    );
    assert_eq!(parsed.words[4].parts[0].kind, WordPartKind::Escaped('$'));
    for (word, expected) in parsed.words[5..]
        .iter()
        .zip(["~", "*.rs", "a{b,c}", "$@", "$?"])
    {
        assert_eq!(word.parts[0].kind, WordPartKind::Text(expected.into()));
    }
}

#[test]
fn continuations_do_not_split_expansion_names() {
    let source = "echo $HO\\\nME \"$HO\\\nME\"";
    let parsed = command(source);
    assert_eq!(parsed.words[1].parts.len(), 1);
    assert_eq!(
        parsed.words[1].parts[0].kind,
        WordPartKind::Text("$HOME".into())
    );
    assert_eq!(&source[parsed.words[1].parts[0].span.clone()], "$HO\\\nME");
    let WordPartKind::DoubleQuoted(inner) = &parsed.words[2].parts[0].kind else {
        panic!("expected double quotes")
    };
    assert_eq!(inner.len(), 1);
    assert_eq!(inner[0].kind, WordPartKind::Text("$HOME".into()));
}

#[test]
fn reports_incomplete_input_at_the_opening_quote_or_escape() {
    for (source, kind, span) in [
        (
            "echo 'open",
            ParseErrorKind::UnclosedQuote(Quote::Single),
            5..6,
        ),
        (
            "echo \"open",
            ParseErrorKind::UnclosedQuote(Quote::Double),
            5..6,
        ),
        (r"echo \", ParseErrorKind::TrailingEscape, 5..6),
        ("echo \"open\\", ParseErrorKind::TrailingEscape, 10..11),
        (
            "🦀 'open",
            ParseErrorKind::UnclosedQuote(Quote::Single),
            5..6,
        ),
        (
            "echo 'can\\'t'",
            ParseErrorKind::UnclosedQuote(Quote::Single),
            12..13,
        ),
    ] {
        let error = parse_simple_command(source).unwrap_err();
        assert_eq!(error.kind, kind, "{source:?}");
        assert_eq!(error.span, span, "{source:?}");
        assert!(error.is_incomplete());
    }
}

#[test]
fn rejects_operators_and_nested_syntax_before_it_can_be_misread_as_arguments() {
    for input in [
        "echo a;b",
        "echo a && b",
        "echo a || b",
        "echo a | b",
        "echo a &",
        "echo >file",
        "echo <file",
        "echo (a)",
        "echo )",
        "echo $(date)",
        "echo ${HOME}",
        "echo $((1+2))",
        "echo $[1+2]",
        "echo `date`",
        "echo $'text'",
        "echo $\"text\"",
        "echo <(pwd)",
        "echo \"$(date)\"",
        "echo \"${HOME}\"",
        "echo \"`date`\"",
        "echo $\\\n(date)",
        "echo $\\\n'ansi'",
        "echo \"$\\\n{HOME}\"",
    ] {
        let error = parse_simple_command(input).unwrap_err();
        assert!(
            matches!(error.kind, ParseErrorKind::UnsupportedSyntax(_)),
            "{input:?}: {error}"
        );
        assert!(!error.is_incomplete());
    }
    let error = parse_simple_command("echo $\\\n(date)").unwrap_err();
    assert_eq!(error.span, 5..9);
    assert_eq!(words(r#"echo ';' "|" \& \$\(date\)"#)[1..3], [";", "|"]);
}

#[test]
fn rejects_multiple_commands_and_nul_with_source_locations() {
    for source in [
        "echo one\necho two",
        "echo one # comment\necho two",
        "echo one # comment\\\necho two",
    ] {
        let error = parse_simple_command(source).unwrap_err();
        assert_eq!(error.kind, ParseErrorKind::MultipleCommands);
        assert_eq!(&source[error.span], "e");
    }
    let error = parse_simple_command("echo 🦀\0").unwrap_err();
    assert_eq!(error.kind, ParseErrorKind::NullByte);
    assert_eq!(error.span, 9..10);
}

#[test]
fn defers_assignment_prefixes_and_reserved_command_words() {
    for source in [
        "HOME=value echo",
        "HOME='value' echo",
        "HOME+=value",
        "items[0]=value",
        "if true",
        "while true",
        "! echo",
        "{ echo",
        "[[ value",
        "time echo",
    ] {
        assert!(
            matches!(
                parse_simple_command(source).unwrap_err().kind,
                ParseErrorKind::UnsupportedSyntax(_)
            ),
            "{source}"
        );
    }
    for source in [
        r#""if" arg"#,
        r"\if arg",
        "echo if HOME=value",
        "echo { } [[ ]]",
        r#"HO"ME"=value"#,
    ] {
        assert!(parse_simple_command(source).is_ok(), "{source}");
    }
}

fn check_parts(source: &str, parts: &[WordPart], parent: std::ops::Range<usize>) {
    let mut end = parent.start;
    for part in parts {
        assert!(end <= part.span.start && part.span.start < part.span.end);
        assert!(part.span.end <= parent.end);
        assert!(source.get(part.span.clone()).is_some());
        if let WordPartKind::DoubleQuoted(children) = &part.kind {
            check_parts(source, children, part.span.clone());
        }
        end = part.span.end;
    }
}

#[test]
fn short_inputs_never_panic_and_all_spans_are_valid_utf8_ranges() {
    // A bounded exhaustive corpus exercises transitions between quotes, escapes,
    // comments, continuation, Unicode, and unsupported operators.
    let alphabet = [
        'a', ' ', '\t', '\n', '\\', '\'', '"', '$', '`', '#', '|', '🦀',
    ];
    for mut value in 0..alphabet.len().pow(4) {
        let mut source = String::new();
        for _ in 0..4 {
            source.push(alphabet[value % alphabet.len()]);
            value /= alphabet.len();
        }
        match parse_simple_command(&source) {
            Ok(Some(command)) => {
                assert!(!command.words.is_empty());
                assert!(source.get(command.span.clone()).is_some());
                let mut end = command.span.start;
                for word in command.words {
                    assert!(end <= word.span.start && word.span.start < word.span.end);
                    assert!(word.span.end <= command.span.end);
                    assert!(!word.parts.is_empty());
                    check_parts(&source, &word.parts, word.span.clone());
                    end = word.span.end;
                }
            }
            Ok(None) => {}
            Err(error) => {
                assert!(!error.span.is_empty());
                assert!(source.get(error.span).is_some());
            }
        }
    }
}

#[test]
#[ignore = "requires Bash; set WINSH_TEST_BASH to its executable path"]
fn literal_quoting_matches_bash() {
    use std::{io::Write, process::Stdio};

    let bash =
        std::env::var_os("WINSH_TEST_BASH").expect("set WINSH_TEST_BASH to a Bash executable");
    for (input, expected) in quoting_cases() {
        // The Windows Bash port used for development drops raw CR in source,
        // even with igncr disabled. Keep testing those cases in the native Rust
        // suite; compare them to Bash when running this test on Linux instead.
        if cfg!(windows) && input.contains('\r') {
            continue;
        }
        let mut child = std::process::Command::new(&bash)
            .args(["--noprofile", "--norc", "-s"])
            .env_remove("BASH_ENV")
            .env_remove("ENV")
            .env_remove("SHELLOPTS")
            .env_remove("BASHOPTS")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start Bash");
        // Feed exact source bytes without an extra layer of command-line quoting.
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("printf '%s\\0' {input}").as_bytes())
            .expect("write Bash fixture");
        let output = child.wait_with_output().expect("wait for Bash");
        assert!(
            output.status.success(),
            "{input:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut expected_bytes = Vec::new();
        for word in expected {
            expected_bytes.extend_from_slice(word.as_bytes());
            expected_bytes.push(0);
        }
        assert_eq!(output.stdout, expected_bytes, "Bash fixture: {input:?}");
        let parsed = words(&format!("capture {input}"));
        let mut actual = Vec::new();
        for word in &parsed[1..] {
            actual.extend_from_slice(word.as_bytes());
            actual.push(0);
        }
        assert_eq!(actual, output.stdout, "winsh vs Bash: {input:?}");
    }
}
