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
        let dir = std::env::temp_dir().join(format!("fs-util-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("entry.bin");
        std::fs::write(&file, b"x").unwrap();
        sync_parent(&file).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
