use std::env::current_dir;
use std::ffi::c_int;
use std::future::{Future, poll_fn};
use std::io::BufRead;
use std::path::Path;
use std::pin::Pin;
use std::process::ExitStatus;
use std::task::{Context, Poll};
use derive_more::From;
use derive_more::Display;
use log::{error, info, warn};
use tokio::process::Command;
use tokio::io::{AsyncReadExt, self, BufReader, AsyncBufReadExt, Lines, AsyncRead};
use tokio::{select, try_join};

#[derive(Debug, From, Display)]
pub enum CommandError {
    #[display(fmt = "IO Error occurred while executing command: {}", _0)]
    IO(io::Error),
    #[display(fmt = "Provided command string didn't contain a command. (Was it empty?)")]
    MissingCommand,
    #[display(fmt = "Process exited with non-zero exit code: Code {}", _0)]
    NoZeroExitCode(i32)
}

type CommandResult<T> = Result<T, CommandError>;

/// Executes the provided command in the provided working directory
/// in this case the command is a format string which can contain
/// format arguments (i.e. {0} {1}) these variables are provided in
/// the `args_in` slice
pub async fn run_command_format(
    working_dir: impl AsRef<Path>,
    command: &str,
    args_in: &[&str],
) -> CommandResult<()> {
    let (cmd, args) = split_command(command)
        .ok_or(CommandError::MissingCommand)?;
    let args = transform_args(args, args_in);

    let mut command = Command::new(cmd);
    command.args(&args);
    command.current_dir(working_dir);

    // Java specific environment variables
    const JAVA_ENV: &str = "_JAVA_OPTIONS";
    if std::env::var(JAVA_ENV).is_err() {
        command.env(JAVA_ENV, "-Djdk.net.URLClassPath.disableClassPathURLCheck=true");
    }

    // Maven specific environment variables
    const MAVEN_ENV: &str = "MAVEN_OPTS";
    if std::env::var(MAVEN_ENV).is_err() {
        command.env(MAVEN_ENV, "-Xmx1024M");
    }

    let exit_status = pipe_and_wait(command).await?;
    let code = exit_status.code()
            .unwrap_or(0);
    if code != 0  {
        return Err(CommandError::NoZeroExitCode(code));
    }


    Ok(())
}

struct OptionalReader<V> {
    child: Option<Lines<BufReader<V>>>,
}

impl<V> OptionalReader<V> where V: Unpin + AsyncRead {
    fn new(value: Option<V>) -> Self {
        Self {
            child: value
                .map(|value| BufReader::new(value).lines())
        }
    }

    async fn next_line(&mut self) -> io::Result<Option<String>> {
        if let Some(child) = &mut self.child {
            return child.next_line().await;
        }
        // Never resolve if no child
        return poll_fn(|_| Poll::Pending).await
    }
}

impl<V> Future for OptionalReader<V> where V: AsyncRead {
    type Output = io::Result<Option<String>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

/// Spawns the command child piping its output to the error logging for
/// the application and waiting until the process exists returning the
/// exit status of the program or an Error
async fn pipe_and_wait(mut command: Command) -> CommandResult<ExitStatus> {
    let mut child = command.spawn()?;

    let mut stdout =  OptionalReader::new(child.stdout.take());
    let mut stderr =  OptionalReader::new(child.stderr.take());


    /// Splits a piped line output into the line itself and a
    /// logging level if one is present
    fn split_line(line: &str) -> Option<(&str, &str)> {
        let start = line.find('[')?;
        let end = line.find(']')?;
        if end <= start {
            return None;
        }
        let level = &line[start + 1..end - 1];
        let text = &line[end + 1..];
        Some((level ,text))
    }

    /// Pipes the line to the proper output channel if this
    /// line represents an error which crosses multiple lines
    /// then that state is returned
    fn pipe_line(line: &str, stderr: bool, errored: &mut bool) {
        if let Some((level, text)) = split_line(line) {
            match level {
                "WARN" => warn!("{text}"),
                "FATAL" | "ERROR" => error!("{text}"),
                _ => {
                    if stderr {
                        error!("{text}");
                    } else {
                        info!("{text}");
                    }
                }
            }
            return;
        }

        // Java exceptions
        if line.starts_with("Exception in thread") {
            error!("{line}");
            *errored = true;
        } else if line.contains("Error") || stderr {
            error!("{line}");
        } else {
            info!("{line}")
        }
    }

    let mut errored = false;

    loop {
        select! {
            result = stdout.next_line() => {
                let result = result?;
                if let Some(line) = result {
                    pipe_line(&line, false, &mut errored);
                }
            }
            result = stderr.next_line() => {
                let result = result?;
                if let Some(line) = result {
                    pipe_line(&line, true, &mut errored);
                }
            }
            result = child.wait() => {
                let result = result?;
                return Ok(result);
            }
        }
    }
}

/// Splits the command into the command itself and a vector
/// containing the additional arguments
fn split_command(value: &str) -> Option<(&str, Vec<&str>)> {
    let mut parts = value.split_whitespace();
    let command = parts.next()?;
    let args = parts.collect::<Vec<&str>>();
    Some((command, args))
}

/// Transforms the provided `args` formatting them replacing their
/// values with those stored in the `args_in` slice
fn transform_args<'a: 'b, 'b>(args: Vec<&'a str>, args_in: &'a [&str]) -> Vec<&'b str> {

    /// Parses a format value from the provided `value`
    /// returning the index stored inside it or None if
    /// it could not be parsed as a format
    fn parse_format(value: &str) -> Option<usize> {
        let start = value.find('{')?;
        let end = value.find('}')?;
        if end <= start {
            return  None;
        }
        let format = &value[start + 1..end];
        format.parse::<usize>().ok()
    }

    let mut out = Vec::with_capacity(args.len());
    for arg in args {
        if let Some(index) = parse_format(arg) {
            if let Some(value) = args_in.get(index) {
                out.push(*value);
                continue;
            }
        }
        out.push(arg)
    }
    out
}

#[cfg(test)]
mod test {
    use std::env::current_dir;
    use std::io;
    use crate::cmd::{CommandResult, run_command_format};

    #[tokio::test]
    async fn test_ls() -> CommandResult<()> {
        let working_dir = current_dir()?;

        let command = "bash ./test/test.sh {0}";
        let args = ["target"];

        run_command_format(&working_dir, command, &args).await
    }

}