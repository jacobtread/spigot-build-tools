use crate::cmd::{run_command, CommandError};
use crate::fs::{create_directory, remove_existing};
use crate::RootContext;
use derive_more::{Display, From};
use log::{info, warn};
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs::create_dir_all;
use tokio::try_join;

#[derive(Debug, From, Display)]
pub enum RepositoryError {
    #[display(fmt = "IO Error occurred while working with repositories: {}", _0)]
    IO(io::Error),
    #[display(fmt = "Unable to execute git command: {}", _0)]
    CommandError(CommandError),
}

type RepoResult<T> = Result<T, RepositoryError>;

const BUILD_DATA_URL: &str = "https://hub.spigotmc.org/stash/scm/spigot/builddata.git";
const BUKKIT_URL: &str = "https://hub.spigotmc.org/stash/scm/spigot/bukkit.git";
const CRAFT_BUKKIT_URL: &str = "https://hub.spigotmc.org/stash/scm/spigot/craftbukkit.git";
const SPIGOT_URL: &str = "https://hub.spigotmc.org/stash/scm/spigot/spigot.git";

pub(crate) async fn init_repositories(root: &RootContext<'_>) -> RepoResult<()> {
    let (bd_repo, bk_repo, cb_repo, sp_repo) = try_join!(
        init_repository(root, BUILD_DATA_URL, "BuildData"),
        init_repository(root, BUKKIT_URL, "Bukkit"),
        init_repository(root, CRAFT_BUKKIT_URL, "CraftBukkit"),
        init_repository(root, SPIGOT_URL, "Spigot"),
    )?;

    info!("{bd_repo:?}");
    info!("{bk_repo:?}");
    info!("{cb_repo:?}");
    info!("{sp_repo:?}");

    Ok(())
}

async fn init_repository(
    root: &RootContext<'_>,
    url: &'static str,
    name: &'static str,
) -> RepoResult<Repository> {
    let path = root.root_path.join(name);
    info!("{path:?}");
    create_directory(&path).await?;
    // If the git is not valid we must remove it and clone again
    if !is_valid_git(&path) {
        remove_existing(&path).await?;
        create_dir_all(&path).await?;
        run_command(root.root_path, "git", &["clone", url, name]).await?;
    }
    Ok(Repository { url, name, path })
}

/// Checks whether the provided path contains a git directory
fn is_valid_git(path: impl AsRef<Path>) -> bool {
    let path = path.as_ref().join(".git");
    path.exists() && path.is_dir()
}

#[derive(Debug)]
pub struct Repository {
    url: &'static str,
    name: &'static str,
    path: PathBuf,
}

#[cfg(test)]
mod test {
    use crate::git::{init_repositories, init_repository, RepoResult, BUILD_DATA_URL};
    use crate::RootContext;
    use env_logger::WriteStyle;
    use log::info;
    use log::LevelFilter;
    use std::path::Path;

    fn init_logger() {
        env_logger::builder()
            .write_style(WriteStyle::Always)
            .filter_level(LevelFilter::Info)
            .try_init()
            .ok();
    }

    #[tokio::test]
    async fn init_build_data() -> RepoResult<()> {
        init_logger();
        let context = RootContext {
            root_path: Path::new("build"),
        };
        let repo = init_repository(&context, BUILD_DATA_URL, "BuildData").await?;
        info!("{repo:?}");
        Ok(())
    }

    #[tokio::test]
    async fn init_all() -> RepoResult<()> {
        init_logger();
        let context = RootContext {
            root_path: Path::new("build"),
        };
        init_repositories(&context).await?;
        Ok(())
    }
}
