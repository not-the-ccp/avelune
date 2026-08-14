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
pub struct TempPath(PathBuf);
impl TempPath {
    pub fn new(name: &str) -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Self(std::env::temp_dir().join(format!("avelune-{}-{nonce}-{name}", std::process::id())))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
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
