use crate::{
    ShellIo, Utility, builtins, expand, pipeline, platform, redirect,
    streams::Stream,
    syntax::{self, Command, CommandKind, Script, ScriptPipeline, Word, WordPartKind},
};
use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::{Path, PathBuf},
    thread,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Continue(i32),
    Exit(i32),
}
impl Outcome {
    pub fn status(self) -> i32 {
        match self {
            Self::Continue(s) | Self::Exit(s) => s,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flow {
    Return(i32),
    Break(usize),
    Continue(usize),
}
#[derive(Clone)]
pub(crate) struct SavedVariable {
    value: Option<OsString>,
    exported: Option<OsString>,
}

/// Explicit session state; the engine never changes the host cwd or environment.
#[derive(Clone)]
pub struct Shell {
    pub(crate) shell_pid: u32,
    pub(crate) jobs: crate::jobs::Jobs,
    pub(crate) cwd: PathBuf,
    pub(crate) environment: Vec<(OsString, OsString)>,
    pub(crate) variables: BTreeMap<String, OsString>,
    pub(crate) functions: BTreeMap<String, Command>,
    pub(crate) aliases: BTreeMap<String, crate::aliases::Alias>,
    pub(crate) positional: Vec<OsString>,
    pub(crate) arg0: OsString,
    pub(crate) flow: Option<Flow>,
    pub(crate) loop_depth: usize,
    pub(crate) function_depth: usize,
    pub(crate) source_depth: usize,
    pub(crate) scopes: Vec<BTreeMap<String, SavedVariable>>,
    pub(crate) substitution_status: Option<i32>,
    pub(crate) cancellation: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) utility_executable: Option<PathBuf>,
    capture_depth: usize,
    script_depth: usize,
    abort_script: bool,
    pub(crate) last_status: i32,
}
impl Shell {
    pub fn new() -> io::Result<Self> {
        Self::with_environment(env::current_dir()?, env::vars_os())
    }
    pub fn with_environment(
        cwd: PathBuf,
        environment: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> io::Result<Self> {
        if !cwd.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "shell working directory must be absolute",
            ));
        }
        platform::require_directory(&cwd)?;
        let mut shell = Self {
            shell_pid: std::process::id(),
            jobs: Default::default(),
            cwd,
            environment: environment.into_iter().collect(),
            variables: BTreeMap::new(),
            functions: BTreeMap::new(),
            aliases: BTreeMap::new(),
            positional: Vec::new(),
            arg0: "winsh".into(),
            flow: None,
            loop_depth: 0,
            function_depth: 0,
            source_depth: 0,
            scopes: Vec::new(),
            substitution_status: None,
            capture_depth: 0,
            script_depth: 0,
            abort_script: false,
            cancellation: None,
            utility_executable: None,
            last_status: 0,
        };
        shell.set_env("PWD", shell.cwd.clone().into_os_string());
        Ok(shell)
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    /// Use a winsh executable for utility commands so foreground cancellation
    /// is handled by the OS. Embedded sessions run utilities directly by default.
    pub fn set_utility_executable(&mut self, executable: PathBuf) {
        self.utility_executable = Some(executable);
    }
    pub fn last_status(&self) -> i32 {
        self.last_status
    }
    /// Load interactive defaults, followed by the user's optional startup script.
    pub fn load_interactive_profile(&mut self, io: &mut ShellIo) -> io::Result<Outcome> {
        self.run("alias ls='ls --color --icons --directories-first'", io)?;
        #[cfg(windows)]
        let root = self
            .env("APPDATA")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        #[cfg(unix)]
        let root = self
            .env("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| {
                self.home()
                    .map(|p| Path::new(p).join(".config"))
                    .filter(|p| p.is_absolute())
            });
        if let Some(path) = root.map(|p| p.join("winsh").join("winshrc")) {
            match path.try_exists() {
                Ok(true) => {
                    return crate::language_builtins::dispatch(
                        self,
                        "source",
                        &[path.into_os_string()],
                        io,
                    );
                }
                Ok(false) => {}
                Err(error) => io.err(format_args!("winsh: {}: {error}\n", path.display()))?,
            }
        }
        Ok(Outcome::Continue(0))
    }
    pub fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }
    pub fn set_arguments(&mut self, arg0: OsString, arguments: Vec<OsString>) {
        self.arg0 = arg0;
        self.positional = arguments;
    }
    pub fn run(&mut self, source: &str, io: &mut ShellIo) -> io::Result<Outcome> {
        match syntax::parse_script(source) {
            Ok(script) => self.execute_script(&script, io),
            Err(error) => {
                io.err(format_args!("winsh: {error}\n"))?;
                Ok(self.record(Outcome::Continue(2)))
            }
        }
    }
    pub fn execute(
        &mut self,
        command: &syntax::SimpleCommand,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        self.command(
            &Command {
                kind: CommandKind::Simple(command.clone()),
                redirections: Vec::new(),
            },
            io,
            false,
        )
    }
    pub fn execute_pipeline(
        &mut self,
        pipeline: &syntax::Pipeline,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        self.evaluate_pipeline(
            &ScriptPipeline {
                label: None,
                commands: pipeline
                    .commands
                    .iter()
                    .cloned()
                    .map(|simple| Command {
                        kind: CommandKind::Simple(simple),
                        redirections: Vec::new(),
                    })
                    .collect(),
                connections: pipeline.connections.clone(),
                negated: false,
            },
            io,
        )
    }
    pub fn execute_script(&mut self, script: &Script, io: &mut ShellIo) -> io::Result<Outcome> {
        if self.script_depth == 0 {
            self.abort_script = false;
        }
        self.script_depth += 1;
        let result = self.script(script, io);
        self.script_depth -= 1;
        if self.script_depth == 0 {
            self.abort_script = false;
        }
        result
    }
    fn script(&mut self, script: &Script, io: &mut ShellIo) -> io::Result<Outcome> {
        let mut outcome = Outcome::Continue(self.last_status);
        for list in &script.lists {
            if list.pipelines.is_empty() || list.operators.len() + 1 != list.pipelines.len() {
                io.err(format_args!("winsh: invalid command-list syntax tree\n"))?;
                return Ok(self.record(Outcome::Continue(2)));
            }
            if let Some(label) = &list.background {
                let mut task = list.clone();
                task.background = None;
                outcome = self.launch_list(task, label, io)?;
                continue;
            }
            for (index, pipeline) in list.pipelines.iter().enumerate() {
                if self
                    .cancellation
                    .as_ref()
                    .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
                {
                    return Err(io::Error::other(
                        "pipeline cancelled after host I/O failure",
                    ));
                }
                if index > 0
                    && match list.operators[index - 1] {
                        syntax::ListOperator::And => outcome.status() != 0,
                        syntax::ListOperator::Or => outcome.status() == 0,
                    }
                {
                    continue;
                }
                outcome = self.evaluate_pipeline(pipeline, io)?;
                if matches!(outcome, Outcome::Exit(_)) || self.flow.is_some() || self.abort_script {
                    return Ok(outcome);
                }
            }
        }
        Ok(outcome)
    }
    pub(crate) fn evaluate_pipeline(
        &mut self,
        pipeline: &ScriptPipeline,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        if pipeline.commands.is_empty() || pipeline.connections.len() + 1 != pipeline.commands.len()
        {
            io.err(format_args!("winsh: invalid pipeline syntax tree\n"))?;
            return Ok(self.record(Outcome::Continue(2)));
        }
        if self.jobs.interactive && pipeline.uses_subshell() {
            // The worker evaluates the full pipeline, including its negation.
            return self.launch_pipeline(pipeline, io);
        }
        let outcome = if pipeline.commands.len() == 1 {
            self.command(&pipeline.commands[0], io, false)?
        } else {
            Outcome::Continue(pipeline::run(self, pipeline, io)?)
        };
        let outcome = if pipeline.negated && matches!(outcome, Outcome::Continue(_)) {
            Outcome::Continue(i32::from(outcome.status() == 0))
        } else {
            outcome
        };
        Ok(self.record(outcome))
    }
    pub(crate) fn command(
        &mut self,
        command: &Command,
        io: &mut ShellIo,
        pipe_stderr: bool,
    ) -> io::Result<Outcome> {
        let mut streams = io.try_clone()?;
        let result = if let CommandKind::Simple(simple) = &command.kind {
            self.simple(simple, &mut streams, pipe_stderr)
        } else {
            match redirect::apply(self, &command.redirections, &mut streams)? {
                Some(failure) => Ok(failure),
                None => {
                    if pipe_stderr {
                        streams.duplicate(2, 1)?;
                    }
                    self.compound(&command.kind, &mut streams)
                }
            }
        };
        match result {
            Ok(outcome) => Ok(self.record(outcome)),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::InvalidInput
                ) =>
            {
                let _ = streams.err(format_args!("winsh: {error}\n"));
                Ok(self.record(Outcome::Continue(1)))
            }
            Err(error) => Err(error),
        }
    }
    pub(crate) fn expansion_failure(
        &mut self,
        error: expand::ExpansionError,
        io: &mut ShellIo,
    ) -> io::Result<Outcome> {
        self.abort_script = true;
        io.err(format_args!("winsh: {error}\n"))?;
        Ok(Outcome::Continue(2))
    }
    fn simple(
        &mut self,
        simple: &syntax::SimpleCommand,
        io: &mut ShellIo,
        pipe_stderr: bool,
    ) -> io::Result<Outcome> {
        self.substitution_status = None;
        let expanded = crate::aliases::expand(&self.aliases, simple);
        let simple = expanded.as_ref().unwrap_or(simple);
        let mut assignments = Vec::new();
        let mut start = 0;
        for word in &simple.words {
            let Some(assignment) = assignment(word) else {
                break;
            };
            assignments.push(assignment);
            start += 1;
        }
        let prepared = (|| {
            let mut arguments = Vec::new();
            for word in &simple.words[start..] {
                if arguments
                    .first()
                    .is_some_and(|name: &OsString| name == "export" || name == "local")
                    && assignment(word).is_some()
                {
                    arguments.push(expand::scalar(word, self, io)?);
                } else {
                    arguments.extend(expand::fields(word, self, io, true)?);
                }
            }
            Ok::<_, expand::ExpansionError>(arguments)
        })();
        let arguments = match prepared {
            Ok(p) => p,
            Err(error) => return self.expansion_failure(error, io),
        };
        if let Some(failure) = redirect::apply(self, &simple.redirections, io)? {
            return Ok(failure);
        }
        if pipe_stderr {
            io.duplicate(2, 1)?;
        }
        let temporary = !arguments.is_empty();
        let mut saved = BTreeMap::new();
        for (name, word, append) in assignments {
            if temporary {
                saved
                    .entry(name.clone())
                    .or_insert_with(|| self.save_var(&name));
            }
            let value = match expand::scalar(&word, self, io) {
                Ok(value) => value,
                Err(error) => {
                    self.abort_script = true;
                    for (name, value) in saved {
                        self.restore_var(&name, value);
                    }
                    io.err(format_args!("winsh: {error}\n"))?;
                    return Ok(Outcome::Continue(2));
                }
            };
            let mut joined = if append {
                self.var(&name).unwrap_or_default().to_owned()
            } else {
                OsString::new()
            };
            joined.push(value);
            if temporary {
                self.set_env(&name, joined);
                self.variables.remove(&name);
            } else {
                self.set_var(&name, joined);
            }
        }
        let outcome = self.invoke(&arguments, io);
        for (name, value) in saved {
            self.restore_var(&name, value);
        }
        outcome
    }
    fn invoke(&mut self, arguments: &[OsString], io: &mut ShellIo) -> io::Result<Outcome> {
        let Some(name) = arguments.first() else {
            return Ok(Outcome::Continue(self.substitution_status.unwrap_or(0)));
        };
        if let Some(body) = name
            .to_str()
            .and_then(|name| self.functions.get(name))
            .cloned()
        {
            if self.function_depth >= 64 {
                io.err(format_args!("winsh: function recursion limit (64)\n"))?;
                return Ok(Outcome::Continue(2));
            }
            let previous = std::mem::replace(&mut self.positional, arguments[1..].to_vec());
            self.function_depth += 1;
            self.scopes.push(BTreeMap::new());
            let result = self.command(&body, io, false);
            self.function_depth -= 1;
            self.positional = previous;
            for (name, value) in self.scopes.pop().unwrap() {
                self.restore_var(&name, value);
            }
            if let Some(Flow::Return(status)) = self.flow {
                self.flow = None;
                return Ok(Outcome::Continue(status));
            }
            result
        } else if builtins::recognizes(name) {
            Ok(pipeline::builtin(self, arguments, io))
        } else if let Some(utility) = Utility::from_name(name) {
            let status = if let Some(executable) = &self.utility_executable {
                let mut child_args = vec![executable.as_os_str().to_owned(), "--utility".into()];
                child_args.extend_from_slice(arguments);
                self.run_program(&child_args, Some(utility), io)?
            } else {
                utility.run(self, &arguments[1..], io)?
            };
            Ok(Outcome::Continue(status))
        } else {
            Ok(Outcome::Continue(self.run_program(arguments, None, io)?))
        }
    }
    fn compound(&mut self, kind: &CommandKind, io: &mut ShellIo) -> io::Result<Outcome> {
        match kind {
            CommandKind::Simple(_) => unreachable!(),
            CommandKind::Function { name, body } => {
                self.functions.insert(name.clone(), *body.clone());
                Ok(Outcome::Continue(0))
            }
            CommandKind::Group {
                body,
                subshell: false,
            } => self.execute_script(body, io),
            CommandKind::Group {
                body,
                subshell: true,
            } => {
                let mut child = self.clone();
                child.flow = None;
                Ok(Outcome::Continue(child.execute_script(body, io)?.status()))
            }
            CommandKind::If {
                branches,
                otherwise,
            } => {
                for (condition, body) in branches {
                    let result = self.execute_script(condition, io)?;
                    if matches!(result, Outcome::Exit(_))
                        || self.flow.is_some()
                        || self.abort_script
                    {
                        return Ok(result);
                    }
                    if result.status() == 0 {
                        return self.execute_script(body, io);
                    }
                }
                match otherwise {
                    Some(body) => self.execute_script(body, io),
                    None => Ok(Outcome::Continue(0)),
                }
            }
            CommandKind::Case { word, arms } => {
                let value = match expand::scalar(word, self, io) {
                    Ok(value) => value,
                    Err(error) => return self.expansion_failure(error, io),
                };
                let value = value.to_string_lossy();
                let mut outcome = Outcome::Continue(0);
                let mut fall_through = false;
                for arm in arms {
                    if !fall_through {
                        let mut matched = false;
                        for pattern in &arm.patterns {
                            let pattern = match expand::case_pattern(pattern, self, io) {
                                Ok(pattern) => pattern,
                                Err(error) => return self.expansion_failure(error, io),
                            };
                            if crate::glob::matches(&pattern, &value) {
                                matched = true;
                                break;
                            }
                        }
                        if !matched {
                            continue;
                        }
                    }
                    outcome = if arm.body.lists.is_empty() {
                        Outcome::Continue(0)
                    } else {
                        self.execute_script(&arm.body, io)?
                    };
                    if matches!(outcome, Outcome::Exit(_))
                        || self.flow.is_some()
                        || self.abort_script
                        || arm.terminator == syntax::CaseTerminator::Break
                    {
                        break;
                    }
                    fall_through = arm.terminator == syntax::CaseTerminator::FallThrough;
                }
                Ok(outcome)
            }
            CommandKind::For { name, words, body } => {
                let values = if let Some(words) = words {
                    let mut values = Vec::new();
                    for word in words {
                        match expand::fields(word, self, io, true) {
                            Ok(fields) => values.extend(fields),
                            Err(e) => {
                                self.abort_script = true;
                                io.err(format_args!("winsh: {e}\n"))?;
                                return Ok(Outcome::Continue(2));
                            }
                        }
                    }
                    values
                } else {
                    self.positional.clone()
                };
                self.loop_depth += 1;
                let result = (|| {
                    let mut outcome = Outcome::Continue(0);
                    for value in values {
                        self.set_var(name, value);
                        outcome = self.execute_script(body, io)?;
                        if self.loop_action(outcome) {
                            break;
                        }
                    }
                    Ok(outcome)
                })();
                self.loop_depth -= 1;
                result
            }
            CommandKind::While {
                condition,
                body,
                until,
            } => {
                self.loop_depth += 1;
                let result = (|| {
                    let mut outcome = Outcome::Continue(0);
                    loop {
                        let test = self.execute_script(condition, io)?;
                        if matches!(test, Outcome::Exit(_))
                            || self.flow.is_some()
                            || self.abort_script
                        {
                            outcome = test;
                            self.loop_action(test);
                            break;
                        }
                        if (test.status() == 0) == *until {
                            break;
                        }
                        outcome = self.execute_script(body, io)?;
                        if self.loop_action(outcome) {
                            break;
                        }
                    }
                    Ok(outcome)
                })();
                self.loop_depth -= 1;
                result
            }
        }
    }
    /// Consume one loop level. Continue(1) advances this loop; larger levels unwind.
    fn loop_action(&mut self, outcome: Outcome) -> bool {
        if self.abort_script {
            return true;
        }
        if matches!(outcome, Outcome::Exit(_)) {
            return true;
        }
        match self.flow {
            Some(Flow::Break(1)) => {
                self.flow = None;
                true
            }
            Some(Flow::Continue(1)) => {
                self.flow = None;
                false
            }
            Some(Flow::Break(n)) => {
                self.flow = Some(Flow::Break(n - 1));
                true
            }
            Some(Flow::Continue(n)) => {
                self.flow = Some(Flow::Continue(n - 1));
                true
            }
            Some(Flow::Return(_)) => true,
            None => false,
        }
    }
    pub(crate) fn capture(&self, source: &str, streams: &ShellIo) -> io::Result<(OsString, i32)> {
        if self.capture_depth >= 32 {
            return Err(io::Error::other("command substitution nesting limit (32)"));
        }
        let (mut reader, writer) = io::pipe()?;
        let mut io = streams.try_clone()?;
        io.set(1, Stream::Writer(writer))?;
        let mut child = self.clone();
        child.flow = None;
        child.capture_depth += 1;
        child.script_depth = 0;
        child.abort_script = false;
        thread::scope(|scope| {
            let collector = scope.spawn(move || {
                let mut bytes = Vec::new();
                reader.read_to_end(&mut bytes)?;
                Ok::<_, io::Error>(bytes)
            });
            let outcome = child.run(source, &mut io);
            drop(io);
            let mut bytes = collector
                .join()
                .map_err(|_| io::Error::other("capture worker panicked"))??;
            while bytes.last() == Some(&b'\n') {
                bytes.pop();
            }
            if bytes.contains(&0) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "command substitution output contains NUL",
                ));
            }
            let text = String::from_utf8(bytes).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "command substitution output must be UTF-8",
                )
            })?;
            Ok((text.into(), outcome?.status()))
        })
    }
    pub fn interrupt(&mut self) {
        self.last_status = 130;
    }
    pub(crate) fn record(&mut self, outcome: Outcome) -> Outcome {
        self.last_status = outcome.status();
        outcome
    }
    pub(crate) fn env(&self, name: &str) -> Option<&OsStr> {
        platform::env_value(&self.environment, name)
    }
    pub(crate) fn var(&self, name: &str) -> Option<&OsStr> {
        self.variables
            .get(name)
            .map(OsString::as_os_str)
            .or_else(|| self.env(name))
    }
    pub(crate) fn home(&self) -> Option<&OsStr> {
        self.var("HOME").or_else(|| {
            if cfg!(windows) {
                self.var("USERPROFILE")
            } else {
                None
            }
        })
    }
    pub(crate) fn set_env(&mut self, name: &str, value: OsString) {
        self.environment
            .retain(|(key, _)| !platform::env_name_eq(key, name));
        self.environment.push((name.into(), value));
    }
    pub(crate) fn set_var(&mut self, name: &str, value: OsString) {
        if self.env(name).is_some() {
            self.set_env(name, value);
            self.variables.remove(name);
        } else {
            self.variables.insert(name.into(), value);
        }
    }
    pub(crate) fn unset(&mut self, name: &str) {
        self.variables.remove(name);
        self.environment
            .retain(|(key, _)| !platform::env_name_eq(key, name));
    }
    pub(crate) fn save_var(&self, name: &str) -> SavedVariable {
        SavedVariable {
            value: self.variables.get(name).cloned(),
            exported: self.env(name).map(OsStr::to_owned),
        }
    }
    pub(crate) fn restore_var(&mut self, name: &str, saved: SavedVariable) {
        self.unset(name);
        if let Some(v) = saved.value {
            self.variables.insert(name.into(), v);
        }
        if let Some(v) = saved.exported {
            self.set_env(name, v);
        }
    }
    pub(crate) fn parameter_is_set(&self, name: &str) -> bool {
        if name == "0" {
            true
        } else if let Ok(n) = name.parse::<usize>() {
            n > 0 && n <= self.positional.len()
        } else {
            matches!(name, "?" | "#" | "$" | "@" | "*")
                || (name == "!" && self.jobs.last_pid.is_some())
                || self.var(name).is_some()
        }
    }
    pub(crate) fn parameter(&self, name: &str) -> OsString {
        match name {
            "0" => self.arg0.clone(),
            "?" => self.last_status.to_string().into(),
            "#" => self.positional.len().to_string().into(),
            "$" => self.shell_pid.to_string().into(),
            "!" => self
                .jobs
                .last_pid
                .map(|pid| pid.to_string())
                .unwrap_or_default()
                .into(),
            "*" | "@" => {
                let separator = self
                    .var("IFS")
                    .map(|v| {
                        v.to_string_lossy()
                            .chars()
                            .next()
                            .map(|c| c.to_string())
                            .unwrap_or_default()
                    })
                    .unwrap_or_else(|| " ".into());
                let mut out = OsString::new();
                for (i, arg) in self.positional.iter().enumerate() {
                    if i > 0 {
                        out.push(&separator);
                    }
                    out.push(arg);
                }
                out
            }
            _ => {
                if let Ok(n) = name.parse::<usize>() {
                    self.positional
                        .get(n.wrapping_sub(1))
                        .cloned()
                        .unwrap_or_default()
                } else {
                    self.var(name).unwrap_or_default().to_owned()
                }
            }
        }
    }
}
pub(crate) fn assignment(word: &Word) -> Option<(String, Word, bool)> {
    let first = word.parts.first()?;
    let WordPartKind::Text(text) = &first.kind else {
        return None;
    };
    let (name, value) = text.split_once('=')?;
    let append = name.ends_with('+');
    let name = name.strip_suffix('+').unwrap_or(name);
    if !syntax::valid_name(name) {
        return None;
    }
    let mut word = word.clone();
    word.parts[0].kind = WordPartKind::Text(value.into());
    Some((name.into(), word, append))
}
