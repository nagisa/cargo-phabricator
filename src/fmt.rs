use tokio::process::Command;
use tokio::io::AsyncBufReadExt;
use futures::StreamExt;
use std::fmt::Write;
use std::path::{Path, PathBuf};

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("could not spawn `cargo fmt`")]
    SpawnCargoFmt(#[source] std::io::Error),
    #[error("could not read a line of `cargo fmt` output")]
    ReadLine(#[source] std::io::Error),
    #[error("could not publish lints to phabricator")]
    PublishLints(#[source] crate::phab::Error),
    #[error("could not obtain the exit code")]
    WaitChild(#[source] std::io::Error),
    #[error("command failed with exit code {0}")]
    ExitStatus(std::process::ExitStatus),
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
        let mut cmd = Command::new("cargo");

        cmd.arg("fmt")
            .arg("--message-format").arg("json")
            .stdout(std::process::Stdio::piped());
        if let Some(args) = args.values_of_os("args") {
            cmd.args(args);
        }
        let mut child = cmd.spawn().map_err(Error::SpawnCargoFmt)?;
        let mut stdout = tokio::io::BufReader::new(child.stdout.as_mut().expect("we're capturing the stdout"));
        let mut line = Vec::with_capacity(1024);
        let mut lints = Vec::with_capacity(64);
        loop {
            line.clear();
            stdout.read_until(b'\n', &mut line).await.map_err(Error::ReadLine)?;
            if line.is_empty() {
                break;
            }
            let files: Vec<FileSchema> = match serde_json::from_slice(&line) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("warning: `cargo fmt` output a line that couldn't be parsed: {}\n{}", e, String::from_utf8_lossy(&line));
                    continue;
                }
            };
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
