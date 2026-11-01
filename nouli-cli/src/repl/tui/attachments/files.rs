//! Bounded path discovery; file contents are read only on explicit selection.
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(crate) fn discover(root: &Path) -> Vec<PathBuf> {
    let start = Instant::now();
    let mut files = ignore::WalkBuilder::new(root)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(std::cmp::Ord::cmp)
        .build()
        .take(20_000)
        .take_while(|_| start.elapsed() < Duration::from_millis(250))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()))
        .map(ignore::DirEntry::into_path)
        .take(5_000)
        .collect::<Vec<_>>();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_is_recursive_and_respects_hidden_ignored_and_symlink_paths() {
        let root = std::env::temp_dir().join(format!(
            "nouli-files-{}",
            nouli_core::identity::Id::generate()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        for name in [".secret", "skip.txt", "nested/旅行.csv"] {
            std::fs::write(root.join(name), "data").unwrap();
        }
        std::fs::write(root.join(".gitignore"), "skip.txt\n").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.join("nested/旅行.csv"), root.join("link.csv")).unwrap();
        assert_eq!(discover(&root), vec![root.join("nested/旅行.csv")]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
