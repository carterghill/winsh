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
            WordPartKind::Parameter(text) => result.push_str(&format!("${{{text}}}")),
            WordPartKind::CommandSubstitution(text) => result.push_str(&format!("$({text})")),
            WordPartKind::Arithmetic(text) => result.push_str(&format!("$(({text}))")),
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
        "echo (a)",
        "echo )",
        "echo $[1+2]",
        "echo `date`",
        "echo $'text'",
        "echo $\"text\"",
        "echo <(pwd)",
        "echo \"`date`\"",
        "echo $\\\n'ansi'",
    ] {
        let error = parse_simple_command(input).unwrap_err();
        assert!(
            matches!(
                error.kind,
                ParseErrorKind::UnsupportedSyntax(_) | ParseErrorKind::MultipleCommands
            ),
            "{input:?}: {error}"
        );
        assert!(!error.is_incomplete());
    }
    assert_eq!(words(r#"echo ';' "|" \& \$\(date\)"#)[1..3], [";", "|"]);
}

#[test]
fn parses_pipelines_and_ordered_redirections_without_losing_quote_context() {
    use winsh::syntax::{PipeKind, RedirectionKind as R, parse_pipeline};
    let source = "  <'in file' program first 2>>errors second >out |& next 2>&1 | last # tail";
    let parsed = parse_pipeline(source).unwrap().unwrap();
    assert_eq!(
        parsed.connections,
        [PipeKind::StdoutAndStderr, PipeKind::Stdout]
    );
    assert_eq!(parsed.commands.len(), 3);
    let first = &parsed.commands[0];
    assert_eq!(
        first
            .words
            .iter()
            .map(|word| quote_removed(&word.parts))
            .collect::<Vec<_>>(),
        ["program", "first", "second"]
    );
    assert_eq!(
        first
            .redirections
            .iter()
            .map(|redirect| redirect.kind)
            .collect::<Vec<_>>(),
        [R::Input, R::Append, R::Output]
    );
    assert_eq!(first.redirections[1].descriptor, Some(2));
    assert_eq!(
        quote_removed(&first.redirections[0].target.parts),
        "in file"
    );
    assert!(matches!(
        first.redirections[0].target.parts[0].kind,
        WordPartKind::SingleQuoted(_)
    ));
    assert_eq!(&source[first.redirections[1].span.clone()], "2>>errors");
    assert_eq!(parsed.commands[1].redirections[0].kind, R::DuplicateOutput);
    assert_eq!(
        &source[parsed.span],
        "<'in file' program first 2>>errors second >out |& next 2>&1 | last"
    );
}

#[test]
fn recognizes_adjacent_operators_io_numbers_and_quoted_literals() {
    use winsh::syntax::{RedirectionKind as R, parse_pipeline};
    for (source, kinds) in [
        (
            "cmd<input>output>>append",
            vec![R::Input, R::Output, R::Append],
        ),
        ("cmd<>rw>|overwrite", vec![R::ReadWrite, R::Clobber]),
        (
            "cmd&>all&>>more",
            vec![R::OutputAndError, R::AppendAndError],
        ),
        ("cmd<&0>&2", vec![R::DuplicateInput, R::DuplicateOutput]),
        (
            "cmd &\\\n>all >\\\n>append",
            vec![R::OutputAndError, R::Append],
        ),
    ] {
        let parsed = parse_simple_command(source).unwrap().unwrap();
        assert_eq!(
            parsed
                .redirections
                .iter()
                .map(|r| r.kind)
                .collect::<Vec<_>>(),
            kinds,
            "{source}"
        );
    }
    let parsed = command("cmd '2'>out 2 >other 2\\\n>>errors");
    assert_eq!(parsed.words.len(), 3);
    assert_eq!(
        parsed
            .redirections
            .iter()
            .map(|r| r.descriptor)
            .collect::<Vec<_>>(),
        [None, None, Some(2)]
    );
    assert_eq!(
        words(r#"cmd 'a|b' "a>b" a\<b"#),
        ["cmd", "a|b", "a>b", "a<b"]
    );
    let parsed = parse_pipeline("one|\n# comment\ntwo").unwrap().unwrap();
    assert_eq!(parsed.commands.len(), 2);
    let parsed = command(">empty");
    assert!(parsed.words.is_empty());
    assert_eq!(parsed.redirections.len(), 1);
}

#[test]
fn missing_pipeline_stages_are_incomplete_but_missing_filenames_are_errors() {
    use winsh::syntax::parse_pipeline;
    for source in ["cmd |", "cmd |&\n# waiting"] {
        let error = parse_pipeline(source).unwrap_err();
        assert_eq!(error.kind, ParseErrorKind::MissingCommand);
        assert!(error.is_incomplete());
        assert!(source.get(error.span).is_some());
    }
    for source in ["cmd >", "cmd 2>&", "cmd >\nfile", "cmd > | next"] {
        let error = parse_pipeline(source).unwrap_err();
        assert_eq!(
            error.kind,
            ParseErrorKind::MissingRedirectionTarget,
            "{source}"
        );
        assert!(!error.is_incomplete());
    }
    for source in [
        "| cmd",
        "cmd | | next",
        "cmd || next",
        "cmd && next",
        "cmd <<<text",
        "cmd 9>file",
    ] {
        assert!(
            matches!(
                parse_pipeline(source).unwrap_err().kind,
                ParseErrorKind::UnsupportedSyntax(_) | ParseErrorKind::MultipleCommands
            ),
            "{source}"
        );
    }
    for source in ["cmd <<EOF", "cmd <<EOF\nbody\n", "cmd <<A <<B\nA\n"] {
        assert!(
            parse_pipeline(source).unwrap_err().is_incomplete(),
            "{source}"
        );
    }
    let pipeline = parse_pipeline("cat <<A | cat <<-'B'\none\nA\n\ttwo\n\tB")
        .unwrap()
        .unwrap();
    let docs: Vec<_> = pipeline
        .commands
        .iter()
        .map(|command| command.redirections[0].here_document.as_ref().unwrap())
        .collect();
    assert_eq!((&*docs[0].body, docs[0].quoted), ("one\n", false));
    assert_eq!((&*docs[1].body, docs[1].quoted), ("two\n", true));
}

#[test]
fn pipeline_corpus_never_panics_and_has_valid_word_and_redirection_spans() {
    use winsh::syntax::parse_pipeline;
    let alphabet = [
        'a', '2', ' ', '\n', '\\', '\'', '"', '#', '|', '<', '>', '&', '🦀',
    ];
    for mut value in 0..alphabet.len().pow(4) {
        let mut source = String::new();
        for _ in 0..4 {
            source.push(alphabet[value % alphabet.len()]);
            value /= alphabet.len();
        }
        match parse_pipeline(&source) {
            Ok(Some(pipeline)) => {
                assert_eq!(pipeline.connections.len() + 1, pipeline.commands.len());
                for command in pipeline.commands {
                    assert!(!command.words.is_empty() || !command.redirections.is_empty());
                    assert!(source.get(command.span.clone()).is_some());
                    for word in command
                        .words
                        .iter()
                        .chain(command.redirections.iter().map(|r| &r.target))
                    {
                        assert!(!word.parts.is_empty());
                        assert!(
                            word.span.start >= command.span.start
                                && word.span.end <= command.span.end
                        );
                        check_parts(&source, &word.parts, word.span.clone());
                    }
                    for redirect in command.redirections {
                        assert!(source.get(redirect.span).is_some());
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                assert!(!error.span.is_empty());
                assert!(source.get(error.span).is_some(), "{source:?}");
            }
        }
    }
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
fn simple_parser_preserves_assignments_for_the_language_evaluator() {
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
        assert!(parse_simple_command(source).is_ok(), "{source}");
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
