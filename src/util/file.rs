use std::fs::{ self, File };
use std::io;

/* io::Error from the OS holds only the errno; these put the path in front. */
pub fn file_error(path: &str) -> impl Fn(io::Error) -> io::Error + '_ {
    move |e| io::Error::new(e.kind(), format!("Failed to read {}: {}", path, e))
}

pub fn read_file(path: &str) -> io::Result<Vec<u8>> {
    fs::read(path).map_err(file_error(path))
}

pub fn open_file(path: &str) -> io::Result<File> {
    File::open(path).map_err(file_error(path))
}
