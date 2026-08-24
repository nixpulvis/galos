use std::{env, error, fmt};

pub type Result<T> = std::result::Result<T, Error>;

pub enum Error {
    /// The variable itself, unset or not UTF-8
    DatabaseUrl(env::VarError),

    /// The file that would have set it, there and unreadable
    Dotenv(dotenv::Error),

    Sqlx(sqlx::Error),

    /// A build wrote to disk and the write failed.
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::DatabaseUrl(env::VarError::NotUnicode(_)) => {
                write!(f, "DATABASE_URL is set to something that is not UTF-8")
            }
            Error::DatabaseUrl(_) => write!(
                f,
                "DATABASE_URL is not set, and no .env file set it either. \
                 Put it in the environment, or in a .env file in the \
                 working directory or one above it, e.g. \
                 DATABASE_URL=postgresql://postgres@localhost/galos_development"
            ),
            Error::Dotenv(e) => write!(f, ".env could not be read: {}", e),
            Error::Sqlx(e) => write!(f, "{}", e),
            Error::Io(e) => write!(f, "{}", e),
        }
    }
}

/// What `Display` says, rather than the derived form
///
/// A `main` returning `Result` prints its error with `Debug`, and the derived
/// one names neither the file nor the variable it was wanted for.
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Error::DatabaseUrl(e) => Some(e),
            Error::Dotenv(e) => Some(e),
            Error::Io(e) => Some(e),
            Error::Sqlx(e) => Some(e),
        }
    }
}

impl From<dotenv::Error> for Error {
    fn from(err: dotenv::Error) -> Error {
        Error::Dotenv(err)
    }
}

impl From<env::VarError> for Error {
    fn from(err: env::VarError) -> Error {
        Error::DatabaseUrl(err)
    }
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Error {
        Error::Sqlx(err)
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Error {
        Error::Io(err)
    }
}
