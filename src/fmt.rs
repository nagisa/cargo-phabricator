use tokio::process::Command;
use futures::StreamExt;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use crate::jsonl::FilterReportedExt;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("could not publish lints to phabricator")]
    PublishLints(#[source] crate::phab::Error),
    #[error("could not get command output")]
    CommandOutput(#[source] crate::jsonl::Error),
    #[error("formatting issues found")]
    Formatting,
}

#[derive(Debug, serde::Deserialize)]
struct MismatchSchema {
    expected: String,
    expected_begin_line: u64,
    expected_end_line: u64,
    original: String,
    original_begin_line: u64,
    original_end_line: u64,
}

#[derive(Debug, serde::Deserialize)]
struct FileSchema {
    name: String,
    mismatches: Vec<MismatchSchema>,
}

fn make_lint(file: &Path, mismatch: &MismatchSchema) -> Result<crate::phab::Lint, Error> {
    let mut description = String::with_capacity(mismatch.original.len() + mismatch.expected.len() + 128);
    description.push_str("```lang=diff\n");
    if !mismatch.original.is_empty() {
        for line in mismatch.original.split("\n") {
            write!(&mut description, "-{}\n", line).expect("can't fail");
        }
    }
    if !mismatch.expected.is_empty() {
        for line in mismatch.expected.split("\n") {
            write!(&mut description, "+{}\n", line).expect("can't fail");
        }
    }
    description.push_str("```");
    Ok(crate::phab::Lint {
        name: "format mismatch".into(),
        code: "RUSTFMT".into(),
        severity: crate::phab::Severity::Error,
        path: PathBuf::from(file).into(),
        description: Some(description.into()),
        line: Some(mismatch.original_end_line),
        column: None,
    })
}

impl crate::Context {
    pub(crate) async fn fmt(&self, args: &clap::ArgMatches<'_>) -> Result<(), Error> {
        let mut lints = Vec::with_capacity(64);
        let result = self.fmt_inner(&mut lints, args).await;
        if !lints.is_empty() {
            self.publish_work(
                &lints,
                &[],
            ).await.map_err(Error::PublishLints)?;
            return Err(Error::Formatting);
        }
        result
    }

    pub(crate) async fn fmt_inner(&self, lints: &mut Vec<crate::phab::Lint>, args: &clap::ArgMatches<'_>) -> Result<(), Error> {
        let mut cmd = Command::new("cargo");
        cmd.arg("fmt")
            .arg("--message-format").arg("json")
            .kill_on_drop(true);
        if let Some(args) = args.values_of_os("args") {
            cmd.args(args);
        }
        let mut values = self.get_stdout_json_lines(cmd).filter_reported();
        while let Some(result) = values.next().await {
            let files: Vec<FileSchema> = result.map_err(Error::CommandOutput)?;
            for file in files {
                for mismatch in &file.mismatches {
                    let filename = Path::new(&file.name);
                    let filename = filename.strip_prefix(&self.arcconfig).unwrap_or(filename);
                    let lint = make_lint(filename, mismatch)?;
                    lint.report();
                    lints.push(lint);
                }
            }
        }
        Ok(())
    }
}
