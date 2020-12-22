use tokio::process::Command;
use std::path::PathBuf;
use futures::StreamExt;
use crate::jsonl::FilterReportedExt;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("could not publish lints to phabricator")]
    PublishLints(#[source] crate::phab::Error),
    #[error("could not get command output")]
    CommandOutput(#[source] crate::jsonl::Error),
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
        let mut lints = Vec::with_capacity(64);
        let result = self.check_inner(&mut lints, subcommand, args).await;
        if !lints.is_empty() {
            self.publish_work(
                &lints,
                &[],
            ).await.map_err(Error::PublishLints)?;
        }
        result
    }

    async fn check_inner(&self, lints: &mut Vec<crate::phab::Lint>, subcommand: &str, args: &clap::ArgMatches<'_>) -> Result<(), Error> {
        let mut cmd = Command::new("cargo");
        cmd.arg(subcommand)
           .arg("--message-format").arg("json")
           .kill_on_drop(true);
        if let Some(args) = args.values_of_os("args") {
            cmd.args(args);
        }
        let values = self.get_reason_json_lines(cmd, "compiler-message").filter_reported();
        futures::pin_mut!(values);
        while let Some(result) = values.next().await {
            let lint: LintSchema = result.map_err(Error::CommandOutput)?;
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
        Ok(())
    }
}
