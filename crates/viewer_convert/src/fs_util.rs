use std::{io, path::Path};

/// Flushes the directory entry of `path` by syncing its parent (unix only).
pub(crate) fn sync_parent(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        std::fs::File::open(parent)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sync_parent_succeeds_for_file_in_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("entry.bin");
        std::fs::write(&file, b"x").unwrap();
        sync_parent(&file).unwrap();
    }
}
