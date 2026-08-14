use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

use crate::error::{CliError, Result};

pub fn read_all(path: &str) -> Result<Vec<u8>> {
    if path == "-" {
        let mut out = Vec::new();
        io::stdin()
            .read_to_end(&mut out)
            .map_err(|e| CliError::io("read stdin", e))?;
        Ok(out)
    } else {
        fs::read(path).map_err(|e| CliError::io(format!("read {path}"), e))
    }
}

pub fn write_all(path: &str, bytes: &[u8]) -> Result<()> {
    if path == "-" {
        io::stdout()
            .write_all(bytes)
            .map_err(|e| CliError::io("write stdout", e))
    } else {
        fs::write(path, bytes).map_err(|e| CliError::io(format!("write {path}"), e))
    }
}

#[derive(Debug)]
pub struct TempPath {
    dir: PathBuf,
    path: PathBuf,
}
impl TempPath {
    pub fn new(name: &str) -> Result<Self> {
        let base = std::env::temp_dir();
        for attempt in 0..64u32 {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let dir = base.join(format!("avelune-{}-{nonce}-{attempt}", std::process::id()));
            #[cfg(unix)]
            let created = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = fs::DirBuilder::new();
                builder.mode(0o700);
                builder.create(&dir)
            };
            #[cfg(not(unix))]
            let created = fs::create_dir(&dir);
            match created {
                Ok(()) => {
                    let path = dir.join(name);
                    fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&path)
                        .map_err(|e| CliError::io(format!("reserve {}", path.display()), e))?;
                    return Ok(Self { dir, path });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(CliError::io("create private temporary directory", e)),
            }
        }
        Err(CliError::message(
            "could not reserve a unique temporary path",
        ))
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_dir(&self.dir);
    }
}

pub fn run_checked(command: &mut Command, description: &str) -> Result<()> {
    let status = command
        .status()
        .map_err(|e| CliError::io(format!("start {description}"), e))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Process {
            command: description.to_owned(),
            status,
        })
    }
}
