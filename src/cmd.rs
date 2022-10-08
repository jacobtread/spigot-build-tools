use derive_more::Display;
use derive_more::From;
use log::{error, info, warn};
use std::future::poll_fn;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::task::Poll;
use tokio::io::{self, AsyncBufReadExt, AsyncRead, BufReader, Lines};
use tokio::process::Command;
use tokio::select;

#[derive(Debug, From, Display)]
pub enum CommandError {
    #[display(fmt = "IO Error occurred while executing command: {}", _0)]
    IO(io::Error),
    #[display(fmt = "Provided command string didn't contain a command. (Was it empty?)")]
    MissingCommand,
    #[display(fmt = "Process exited with non-zero exit code: Code {}", _0)]
    NoZeroExitCode(i32),
}

type CommandResult<T> = Result<T, CommandError>;

/// Executes the provided command with the arguments provided
pub async fn run_command(
    working_dir: impl AsRef<Path>,
    command: &str,
    args: &[&str],
) -> CommandResult<()> {
    let mut command = Command::new(command);
    command.args(args);
    command.current_dir(working_dir);
    command.stderr(Stdio::piped());
    command.stdout(Stdio::piped());
    apply_env(&mut command);

    let exit_status = pipe_and_wait(command).await?;
    let code = exit_status.code().unwrap_or(0);
    if code != 0 {
        return Err(CommandError::NoZeroExitCode(code));
    }

    Ok(())
}

/// Executes the provided command in the provided working directory
/// in this case the command is a format string which can contain
/// format arguments (i.e. {0} {1}) these variables are provided in
/// the `args_in` slice
pub async fn run_command_format(
    working_dir: impl AsRef<Path>,
    command: &str,
    args_in: &[&str],
) -> CommandResult<()> {
    let (cmd, args) = split_command(command).ok_or(CommandError::MissingCommand)?;
    let args = transform_args(args, args_in);

    let mut command = Command::new(cmd);
    command.args(&args);
    command.current_dir(working_dir);
    command.stderr(Stdio::piped());
    command.stdout(Stdio::piped());
    apply_env(&mut command);

    let exit_status = pipe_and_wait(command).await?;
    let code = exit_status.code().unwrap_or(0);
    if code != 0 {
        return Err(CommandError::NoZeroExitCode(code));
    }

    Ok(())
}

/// Applies the build tools specific command environment variables
/// if they aren't already added as system variables
fn apply_env(command: &mut Command) {
    // Java specific environment variables
    const JAVA_ENV: &str = "_JAVA_OPTIONS";
    if std::env::var(JAVA_ENV).is_err() {
        command.env(
            JAVA_ENV,
            "-Djdk.net.URLClassPath.disableClassPathURLCheck=true",
        );
    }

    // Maven specific environment variables
    const MAVEN_ENV: &str = "MAVEN_OPTS";
    if std::env::var(MAVEN_ENV).is_err() {
        command.env(MAVEN_ENV, "-Xmx1024M");
    }
}

/// Custom buf reader structure for reading from a buffered
/// reader in a select statement where the underlying reader
/// could possibly be None (In case of stderr / stdout) this
/// should be used in a select macro
#[derive(Debug)]
struct OptionalReader<V> {
    child: Option<Lines<BufReader<V>>>,
}

impl<V> OptionalReader<V>
where
    V: Unpin + AsyncRead,
{
    /// Constructor for creating a new reader
    fn new(value: Option<V>) -> Self {
        Self {
            child: value.map(|value| BufReader::new(value).lines()),
        }
    }

    /// Async function for reading the next line. If the underlying
    /// reader is None this will just await infinitely
    async fn next_line(&mut self) -> io::Result<Option<String>> {
        if let Some(child) = &mut self.child {
            return child.next_line().await;
        }
        // Never resolve if no child
        return poll_fn(|_| Poll::Pending).await;
    }
}

/// Spawns the command child piping its output to the error logging for
/// the application and waiting until the process exists returning the
/// exit status of the program or an Error
async fn pipe_and_wait(mut command: Command) -> CommandResult<ExitStatus> {
    let mut child = command.spawn()?;

    let mut stdout = OptionalReader::new(child.stdout.take());
    let mut stderr = OptionalReader::new(child.stderr.take());

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
        Some((level, text))
    }

    /// Pipes the line to the proper output channel if this
    /// line represents an error which crosses multiple lines
    /// then that state is returned
    fn pipe_line(line: &str, errored: &mut bool) {
        if let Some((level, text)) = split_line(line) {
            match level {
                "WARN" => warn!("{text}"),
                "FATAL" | "ERROR" => error!("{text}"),
                _ => info!("{text}"),
            }
            return;
        }

        // Java exceptions
        if line.starts_with("Exception in thread") {
            error!("{line}");
            *errored = true;
        } else if line.contains("Error") {
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
                    pipe_line(&line, &mut errored);
                }
            }
            result = stderr.next_line() => {
                let result = result?;
                if let Some(line) = result {
                    pipe_line(&line, &mut errored);
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
            return None;
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
    use crate::cmd::{run_command_format, CommandError, CommandResult};
    use env_logger::WriteStyle;
    use log::LevelFilter;
    use std::env::current_dir;

    fn init_logger() {
        env_logger::builder()
            .write_style(WriteStyle::Always)
            .filter_level(LevelFilter::Info)
            .try_init()
            .ok();
    }

    #[tokio::test]
    async fn test() -> CommandResult<()> {
        init_logger();

        let working_dir = current_dir()?;

        let command = "bash ./test/test.sh {0}";
        let args = ["target"];

        run_command_format(&working_dir, command, &args).await
    }

    #[tokio::test]
    async fn test_err() -> CommandResult<()> {
        init_logger();

        let working_dir = current_dir()?;

        let command = "bash ./test/test_err.sh {0}";
        let args = ["target"];
        let error_code = 5;

        let err = run_command_format(&working_dir, command, &args)
            .await
            .unwrap_err();

        match err {
            CommandError::NoZeroExitCode(code) => {
                assert_eq!(code, error_code)
            }
            err => return Err(err),
        }

        Ok(())
    }
}
