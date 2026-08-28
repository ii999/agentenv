use std::path::{Path, PathBuf};

const PROJECT_FILE_NAME: &str = ".agentenv.toml";

/// Finds the nearest regular project file from `cwd` through the filesystem
/// root.
///
/// Discovery is deliberately infallible: a failed metadata probe is treated
/// as a non-match and the ancestor walk continues.
pub fn discover(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors().find_map(|directory| {
        let candidate = directory.join(PROJECT_FILE_NAME);
        std::fs::metadata(&candidate)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false)
            .then_some(candidate)
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::discover;

    fn project_file(directory: &Path) -> PathBuf {
        let path = directory.join(".agentenv.toml");
        fs::write(&path, "version = 1\n").expect("project file should be writable");
        path
    }

    fn nested_directory(root: &Path) -> PathBuf {
        let nested = root.join("nested/deeper");
        fs::create_dir_all(&nested).expect("nested directory should be creatable");
        nested
    }

    #[test]
    fn discovers_project_file_in_an_ancestor() {
        let root = tempdir().expect("temporary tree should be available");
        let expected = project_file(root.path());
        let cwd = nested_directory(root.path());

        assert_eq!(discover(&cwd), Some(expected));
    }

    #[test]
    fn nearest_project_file_wins() {
        let root = tempdir().expect("temporary tree should be available");
        project_file(root.path());
        let cwd = nested_directory(root.path());
        let expected = project_file(cwd.parent().expect("nested directory has a parent"));

        assert_eq!(discover(&cwd), Some(expected));
    }

    #[test]
    fn skips_directory_named_like_project_file() {
        let root = tempdir().expect("temporary tree should be available");
        let expected = project_file(root.path());
        let cwd = nested_directory(root.path());
        fs::create_dir(cwd.join(".agentenv.toml"))
            .expect("directory candidate should be creatable");

        assert_eq!(discover(&cwd), Some(expected));
    }

    #[cfg(unix)]
    #[test]
    fn skips_dangling_symlink_named_like_project_file() {
        use std::os::unix::fs::symlink;

        let root = tempdir().expect("temporary tree should be available");
        let expected = project_file(root.path());
        let cwd = nested_directory(root.path());
        symlink("missing-project-file", cwd.join(".agentenv.toml"))
            .expect("dangling symlink should be creatable");

        assert_eq!(discover(&cwd), Some(expected));
    }

    #[test]
    fn returns_none_when_no_project_file_exists() {
        let root = tempdir().expect("temporary tree should be available");
        let cwd = nested_directory(root.path());

        assert_eq!(discover(&cwd), None);
    }
}
