//! Native work directory for spools and temporary files, removed on drop.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use crate::ConvertError;

pub(crate) struct Workspace {
    path: PathBuf,
}

impl Workspace {
    pub fn create(output_path: &Path) -> Result<Self, ConvertError> {
        let parent = output_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let file_name = output_path
            .file_name()
            .ok_or_else(|| ConvertError::TemporaryPathUnavailable(output_path.to_owned()))?;
        for _ in 0..100 {
            let counter =
                crate::publish::TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut name = file_name.to_os_string();
            name.push(format!(".{}.{}.work", std::process::id(), counter));
            let path = parent.join(name);
            match fs::create_dir(&path) {
                Ok(()) => {
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(ConvertError::Io(error)),
            }
        }
        Err(ConvertError::TemporaryPathUnavailable(
            output_path.to_owned(),
        ))
    }

    pub fn root(&self) -> &Path {
        &self.path
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
