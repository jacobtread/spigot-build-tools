use std::path::Path;

pub(crate) mod cmd;
pub(crate) mod fs;
pub(crate) mod git;

pub(crate) struct RootContext<'a> {
    pub root_path: &'a Path,
}

pub fn run(root_path: &Path) {
    let context = RootContext { root_path };
}
