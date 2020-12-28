use tokio::process::Command;
use std::path::PathBuf;
use crate::jsonl::FilterReportedExt;
use futures::{Stream, StreamExt};

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("could not get command output")]
    CommandOutput(#[source] crate::jsonl::Error),
    #[error("could not spawn the test")]
    SpawnTest(#[source] std::io::Error),
    #[error("could not wait for the test")]
    WaitTest(#[source] std::io::Error),
    #[error("test failed with {0}")]
    TestStatus(std::process::ExitStatus),
}

#[derive(serde::Deserialize)]
struct ProfileSchema {
    test: bool,
}

#[derive(serde::Deserialize)]
struct TargetSchema {
    src_path: PathBuf,
}

#[derive(serde::Deserialize)]
struct ArtifactSchema {
    executable: Option<PathBuf>,
    profile: ProfileSchema,
    target: TargetSchema,
    package_id: String,
}

impl crate::Context {

    pub(crate) async fn test(&self, args: &clap::ArgMatches<'_>) -> Result<(), Error> {
        // Build tests and collect the artifacts.
        let mut cmd = Command::new("cargo");
        cmd.arg("test")
            .arg("--message-format").arg("json")
            .arg("--no-run")
            .kill_on_drop(true);
        let mut tests = Vec::new();
        let mut artifacts = self.get_reason_json_lines(cmd, "compiler-artifact").filter_reported();
        futures::pin_mut!(artifacts);
        while let Some(result) = artifacts.next().await {
            let artifact: ArtifactSchema = result.map_err(Error::CommandOutput)?;
            if artifact.profile.test {
                tests.push(artifact);
            }
        }

        let mut test_results = futures::stream::iter(tests.into_iter()).map(|artifact| {
            self.run_test(artifact)
        }).buffer_unordered(1); // TODO: this can be >1 in most cases.

        while let Some(result) = test_results.next().await {
            todo!()
        }

        Ok(())
    }

    // FIXME: ideally we ask cargo to run tests instead...
    async fn run_test(&self, artifact: ArtifactSchema) -> Result<Vec<crate::phab::Test>, Error> {
        let mut cmd = if let Some(executable) = artifact.executable {
            tokio::process::Command::new(executable)
        } else {
            eprintln!("warning: test without executable?");
            return Ok(vec![]);
        };
        cmd.kill_on_drop(true);
        let cwd = artifact.target.src_path.ancestors().filter_map(|path| {
            let toml = path.join("Cargo.toml");
            if toml.exists() {
                Some(path)
            } else {
                None
            }
        }).next();

        if let Some(cwd) = cwd {
            cmd.current_dir(cwd);
        } else {
            eprintln!(
                "warning: could not discover cwd for test built from {:?}",
                artifact.target.src_path
            );
        }

        // FIXME: should imitate cargo environment here.
        let child = cmd.spawn().map_err(Error::SpawnTest)?;
        let exit_status = child.wait_with_output().await.map_err(Error::WaitTest)?.status;

        let result = if exit_status.success() {
            crate::phab::TestResult::Pass
        } else {
            crate::phab::TestResult::Fail
        };

        Ok(vec![crate::phab::Test {
            name: "todo".into(),
            result,
            namespace: None,
            duration: None,
            // TODO: add output of the test suite
            details: None,
            format: None,
        }])
    }
}
