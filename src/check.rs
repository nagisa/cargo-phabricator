use tokio::process::Command;
use tokio::io::AsyncBufReadExt;
use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("could not spawn `cargo fmt`")]
    SpawnCargoCheck(#[source] std::io::Error),
    #[error("could not read a line of `cargo fmt` output")]
    ReadLine(#[source] std::io::Error),
    #[error("could not publish lints to phabricator")]
    PublishLints(#[source] crate::phab::Error),
    #[error("could not obtain the exit code")]
    WaitChild(#[source] std::io::Error),
    #[error("command failed with exit code {0}")]
    ExitStatus(std::process::ExitStatus),
}

#[derive(serde::Deserialize)]
struct ReasonSchema {
    reason: String,
}

#[derive(serde::Deserialize)]
#[serde(rename_all="kebab-case")]
enum LintLevel {
    Error,
    Warning,
    Note,
    Help,
    FailureNote,
}

impl From<LintLevel> for crate::phab::Severity {
    fn from(level: LintLevel) -> Self {
        match level {
            LintLevel::Error => Self::Error,
            LintLevel::Warning => Self::Warning,
            LintLevel::Note => Self::Advice,
            LintLevel::Help => Self::Advice,
            LintLevel::FailureNote => Self::Error,
        }
    }
}

#[derive(serde::Deserialize)]
struct SpanSchema {
    column_start: u64,
    line_start: u64,
    file_name: String,
    is_primary: bool,
}

#[derive(serde::Deserialize)]
struct CodeSchema {
    code: String,
}

#[derive(serde::Deserialize)]
struct MessageSchema {
    rendered: String,
    level: LintLevel,
    code: Option<CodeSchema>,
    spans: Vec<SpanSchema>,
    message: String,
}

#[derive(serde::Deserialize)]
struct TargetSchema {
    src_path: String,
}

#[derive(serde::Deserialize)]
struct LintSchema {
    message: MessageSchema,
    target: TargetSchema,
}

impl crate::Context {
    pub(crate) async fn check(&self, subcommand: &str, args: &clap::ArgMatches<'_>) -> Result<(), Error> {
        let mut cmd = Command::new("cargo");

        cmd.arg(subcommand)
            .arg("--message-format").arg("json")
            .stdout(std::process::Stdio::piped());
        if let Some(args) = args.values_of_os("args") {
            cmd.args(args);
        }
        let mut child = cmd.spawn().map_err(Error::SpawnCargoCheck)?;
        let mut stdout = tokio::io::BufReader::new(child.stdout.as_mut().expect("we're capturing the stdout"));
        let mut line = Vec::with_capacity(1024);
        let mut lints = Vec::with_capacity(64);
        loop {
            line.clear();
            stdout.read_until(b'\n', &mut line).await.map_err(Error::ReadLine)?;
            if line.is_empty() {
                break;
            }

            match serde_json::from_slice(&line) {
                Ok(ReasonSchema { reason }) if reason == "compiler-message" => {},
                Ok(_) => continue,
                Err(e) => {
                    eprintln!("warning: `cargo check` output a line that couldn't be parsed: {}\n{}", e, String::from_utf8_lossy(&line));
                    continue;
                }
            }

            let lint: LintSchema = match serde_json::from_slice(&line) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("warning: `cargo check` output a line that couldn't be parsed: {}\n{}", e, String::from_utf8_lossy(&line));
                    continue;
                }
            };

            // So far it seems that the only messages where the code is missing are things like `N
            // warnings emitted`.
            let code = if let Some(code) = lint.message.code {
                format!("CHECK{}", code.code)
            } else {
                continue;
            };
            let description = format!("```\n{}\n```", lint.message.rendered.trim());
            let lint = match lint.message.spans.iter().find(|s| s.is_primary) {
                Some(span) => crate::phab::Lint {
                    name: lint.message.message.into(),
                    code: code.into(),
                    severity: lint.message.level.into(),
                    line: Some(span.line_start),
                    column: Some(span.column_start),
                    path: PathBuf::from(&span.file_name).into(),
                    description: Some(description.into()),
                },
                None => {
                    let filename = PathBuf::from(lint.target.src_path);
                    let filename = filename.strip_prefix(&self.arcconfig).unwrap_or(&filename);
                    crate::phab::Lint {
                        name: lint.message.message.into(),
                        code: code.into(),
                        severity: lint.message.level.into(),
                        line: None,
                        column: None,
                        path: PathBuf::from(filename).into(),
                        description: Some(description.into())
                    }
                },
            };

            lint.report();
            lints.push(lint);
        }

        let exit_status = child.wait_with_output().await.map_err(Error::WaitChild)?.status;
        if !lints.is_empty() {
            self.publish_work(
                &lints,
                &[],
            ).await.map_err(Error::PublishLints)?;
        }
        if !exit_status.success() {
            Err(Error::ExitStatus(exit_status))
        } else {
            Ok(())
        }
    }

}

