use std::borrow::Cow;
use std::path::Path;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("could not send a request to conduit endpoint")]
    MakeRequest(#[source] reqwest::Error),
    #[error("conduit responded with a failure code {0}")]
    ResponseCode(reqwest::StatusCode),
    #[error("could not read the response code for conduit API call")]
    GetResponseBody(#[source] reqwest::Error),
    #[error("could not decode conduit response as JSON")]
    DecodeResponseJson(#[source] serde_json::Error),
    #[error("conduit API request returned a failure: {1}")]
    Api(#[source] Option<Box<dyn std::error::Error>>, String),
    #[error("could not encode the request parameters as JSON")]
    EncodeJson(#[source] serde_json::Error),
}

#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    Advice,
    Autofix,
    Warning,
    Error,
    Disabled,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Severity::Advice => f.write_str("advice"),
            Severity::Autofix => f.write_str("autofix"),
            Severity::Warning => f.write_str("warning"),
            Severity::Error => f.write_str("error"),
            Severity::Disabled => f.write_str("disabled"),
        }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct Lint {
    pub(crate) name: Cow<'static, str>,
    pub(crate) code: Cow<'static, str>,
    pub(crate) severity: Severity,
    pub(crate) path: Cow<'static, Path>,
    pub(crate) description: Option<Cow<'static, str>>,
    pub(crate) line: Option<u64>,
    pub(crate) column: Option<u64>,
}

impl Lint {
    pub(crate) fn report(&self) {
        if let Severity::Disabled = self.severity { return; }

        if let Some(line) = self.line {
            if let Some(column) = self.column {
                println!(
                    "{}[{}]: {}\n   --> {}:{}:{}",
                    self.severity, self.code, self.name,
                    self.path.display(), line, column
                );
            } else {
                println!(
                    "{}[{}]: {}\n   --> {}:{}",
                    self.severity, self.code, self.name,
                    self.path.display(), line
                );
            }
        } else {
            println!(
                "{}[{}]: {}\n   --> {}",
                self.severity, self.code, self.name,
                self.path.display()
            );
        }
        if let Some(descr) = &self.description {
            println!("{}\n", descr);
        }
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TestResult {
    Pass,
    Fail,
    Skip,
    Broken,
    Unsound,
}

#[derive(serde::Serialize)]
pub(crate) struct Test {
    name: Cow<'static, str>,
    result: TestResult,
    namespace: Option<Cow<'static, str>>,
    duration: Option<f64>,
    details: Option<&'static str>,
    format: Option<&'static str>,
}

#[derive(serde::Serialize)]
struct ConduitParams<'a> {
    token: &'a str,
}

#[derive(serde::Serialize)]
struct Params<'a> {
    #[serde(rename="buildTargetPHID")]
    build_target_phid: &'a str,
    lint: &'a [Lint],
    unit: &'a [Test],
    #[serde(rename="__conduit__")]
    conduit: ConduitParams<'a>,
}

#[derive(serde::Deserialize)]
struct ResponseSchema {
    error_code: Option<String>,
    error_info: Option<String>,
}


impl crate::Context {
    pub(crate) async fn publish_work(
        &self,
        lints: &[Lint],
        tests: &[Test]
    ) -> Result<(), Error> {
        let params = Params {
            build_target_phid: &self.build_phid,
            lint: lints,
            unit: tests,
            conduit: ConduitParams {
                token: &self.token,
            },
        };
        let json = serde_json::to_string(&params).map_err(Error::EncodeJson)?;
        let response = reqwest::Client::new()
            .post(&format!("{}/api/harbormaster.sendmessage", self.phab_uri))
            .form(&[("params", json)])
            .send()
            .await
            .map_err(Error::MakeRequest)?;

        if !response.status().is_success() {
            return Err(Error::ResponseCode(response.status()));
        }

        let response_body = response.text().await.map_err(Error::GetResponseBody)?;
        let response: ResponseSchema = serde_json::from_str(&response_body)
            .map_err(Error::DecodeResponseJson)?;
        if let Some(code) = response.error_code {
            return Err(Error::Api(response.error_info.map(Into::into), code));
        }
        Ok(())
    }
}
