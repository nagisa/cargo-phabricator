use tokio::io::AsyncBufReadExt;
use futures::{FutureExt, StreamExt, TryStreamExt};

#[derive(thiserror::Error, Debug)]
pub(crate) enum StreamValuesError {
    #[error("could not read a line from the reader")]
    ReadLine(#[source] std::io::Error),
    #[error("could not parse a line as json: {1:?}")]
    ParseLine(#[source] serde_json::Error, Vec<u8>),
}

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("could not spawn command: {1:?}")]
    Spawn(#[source] std::io::Error, tokio::process::Command),
    #[error("could not obtain the exit code")]
    WaitChild(#[source] std::io::Error),
    #[error("command failed with {0}")]
    ExitStatus(std::process::ExitStatus),
    #[error(transparent)]
    StreamValue(#[from] StreamValuesError),
    #[error("could not parse a value as json")]
    ParseValue(#[source] serde_json::Error),
}

pub(crate) trait FilterReportedExt<'a> {
    type Filtered;
    fn filter_reported(self) -> Self::Filtered;
}

impl<'a, S, T> FilterReportedExt<'a> for S
where S: futures::Stream<Item=Result<T, Error>> + Send + 'a,
      T: 'a + Send
{
    type Filtered = futures::stream::BoxStream<'a, Result<T, Error>>;
    fn filter_reported(self) -> Self::Filtered {
        self.filter_map(|v| async move {
            match v {
                Ok(v) => Some(Ok(v)),
                Err(crate::jsonl::Error::StreamValue(
                        crate::jsonl::StreamValuesError::ParseLine(e, line)
                )) => {
                    eprintln!(
                        "warning: `cargo` output a value that couldn't be parsed: {}\n{}",
                        e,
                        String::from_utf8_lossy(&line)
                    );
                    return None;
                },
                Err(crate::jsonl::Error::ParseValue(e)) => {
                    eprintln!(
                        "warning: `cargo` output a value that couldn't be parsed: {}",
                        e,
                    );
                    return None;
                },
                Err(e) => return Some(Err(e)),
            }
        }).boxed()
    }
}

impl crate::Context {
    pub(crate) fn get_reason_json_lines<T>(&self, cmd: tokio::process::Command, reason: &'static str)
    -> impl futures::Stream<Item=Result<T, Error>>
    where T: serde::de::DeserializeOwned + Send + 'static {
        self.get_stdout_json_lines(cmd).filter_map(move |v| {
            async move {
                let value: serde_json::Value = match v {
                    Err(e) => return Some(Err(e)),
                    Ok(v) => v,
                };
                if value.get("reason").and_then(|v| v.as_str()) != Some(reason) {
                    return None;
                }
                match serde_json::from_value(value) {
                    Err(e) => return Some(Err(Error::ParseValue(e))),
                    Ok(v) => return Some(Ok(v)),
                }
            }
        })

    }

    pub(crate) fn get_stdout_json_lines<T>(&self, mut cmd: tokio::process::Command)
    -> impl futures::Stream<Item=Result<T, Error>>
    where T: serde::de::DeserializeOwned + Send + 'static {
        cmd.stdout(std::process::Stdio::piped());
        match cmd.spawn() {
            Ok(mut c) => {
                self.stream_values(
                    tokio::io::BufReader::new(c.stdout.take().expect("we're capturing the stdout"))
                ).map_err(Error::StreamValue).chain(async move {
                    let exit_status = c.wait_with_output().await.map_err(Error::WaitChild)?.status;
                    if !exit_status.success() {
                        return Err(Error::ExitStatus(exit_status));
                    }
                    Ok(None)
                }.into_stream().filter_map(|v| async move { v.transpose() })).boxed()
            }
            Err(e) => async move { Err(Error::Spawn(e, cmd)) }.into_stream().boxed(),
        }
    }

    pub(crate) fn stream_values<T, R>(&self, reader: R)
    -> impl futures::Stream<Item=Result<T, StreamValuesError>>
    where T: serde::de::DeserializeOwned + Send + 'static,
          R: AsyncBufReadExt + Unpin,
    {
        let line_buffer = Vec::with_capacity(1024);
        futures::stream::try_unfold((reader, line_buffer), |(mut reader, mut line)| {
            async move {
                line.clear();
                reader.read_until(b'\n', &mut line).await.map_err(StreamValuesError::ReadLine)?;
                if line.is_empty() {
                    return Ok(None);
                }
                match serde_json::from_slice(&line) {
                    Ok(v) => return Ok(Some((v, (reader, line)))),
                    Err(e) => return Err(StreamValuesError::ParseLine(e, line)),
                }
            }
        })
    }
}
