use std::{error, fmt, io};

#[derive(Debug)]
pub struct Error(ErrorKind);

#[derive(Debug)]
pub enum ErrorKind {
    NoPageSize,
    InvalidSize(usize),
    IO(io::Error),
}

impl From<ErrorKind> for Error {
    fn from(k: ErrorKind) -> Self {
        Error(k)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Self {
        Error(ErrorKind::IO(err))
    }
}

impl Error {
    pub fn no_page_size() -> Error {
        Error(ErrorKind::NoPageSize)
    }

    pub fn invalid_size(size: usize) -> Error {
        Error(ErrorKind::InvalidSize(size))
    }

    pub fn io(err: io::Error) -> Error {
        Error(ErrorKind::IO(err))
    }

    pub fn last_os_error() -> Error {
        Error::io(io::Error::last_os_error())
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match &self.0 {
            ErrorKind::IO(err) => Some(err),
            _ => None,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            ErrorKind::NoPageSize => write!(fmt, "could not obtain the system's page size"),
            ErrorKind::InvalidSize(size) => write!(
                fmt,
                "the buffer's size: {size} is invalid; must be > 0 and a power of two"
            ),
            ErrorKind::IO(err) => write!(fmt, "IO error: {err}"),
        }
    }
}
