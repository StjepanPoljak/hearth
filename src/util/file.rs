use std::fs::{ self, File };
use anyhow::{ Context, Result };

/* io::Error from the OS holds only the errno; these name the file. */
pub fn read_file(path: &str) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("Failed to read {}", path))
}

pub fn open_file(path: &str) -> Result<File> {
    File::open(path).with_context(|| format!("Failed to read {}", path))
}
