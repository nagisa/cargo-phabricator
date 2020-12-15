use std::path::PathBuf;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("could not obtain the current working directory")]
    CurrentDir(#[source] std::io::Error),

    #[error("could not open .arcconfig: {1:?}")]
    OpenArcConfig(#[source] std::io::Error, PathBuf),

    #[error("could not find any directory with `.arcconfig` containing `repository.callsign` in it")]
    FindArcConfig,
}

#[derive(serde::Deserialize)]
struct ArcConfigSchema {
    #[serde(rename = "repository.callsign")]
    callsign: String,

    #[serde(rename = "phabricator.uri")]
    phab_uri: Option<String>,
}

pub(crate) struct ArcConfig {
    pub(crate) location: PathBuf,
    pub(crate) phab_uri: Option<String>,
}

/// Find an arcconfig above the current working directory.
///
/// The expectation that there's `.arcconfig` at the repository root with `repository.callsign`
/// setting in it.
pub(crate) fn find() -> Result<ArcConfig, Error> {
    let mut cwd = std::env::current_dir().map_err(Error::CurrentDir)?;
    loop {
        let file_name = cwd.join(".arcconfig");
        let mut file = match std::fs::File::open(&file_name) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                if !cwd.pop() {
                    return Err(Error::FindArcConfig);
                }
                continue;
            },
            Err(e) => return Err(Error::OpenArcConfig(e, file_name)),
        };
        if let Ok(c) = serde_json::from_reader::<_, ArcConfigSchema>(&mut file) {
            return Ok(ArcConfig {
                location: cwd,
                phab_uri: c.phab_uri,
            });
        }
        if !cwd.pop() {
            return Err(Error::FindArcConfig);
        }
    }
}
