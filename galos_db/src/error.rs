use std::{env, error, fmt};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub enum Error {
    Env(dotenv::Error),
    Sqlx(sqlx::Error),
    Json(serde_json::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            // TODO: Pretty print, see above todo.
            Error::Env(e) => write!(f, "{}", e),
            Error::Sqlx(e) => write!(f, "{}", e),
            Error::Json(e) => write!(f, "{}", e),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match *self {
            Error::Env(ref e) => Some(e),
            Error::Sqlx(ref e) => Some(e),
            Error::Json(ref e) => Some(e),
        }
    }
}

impl From<dotenv::Error> for Error {
    fn from(err: dotenv::Error) -> Error {
        Error::Env(err)
    }
}

impl From<env::VarError> for Error {
    fn from(err: env::VarError) -> Error {
        Error::Env(dotenv::Error::EnvVar(err))
    }
}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Error {
        Error::Sqlx(err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Error {
        Error::Json(err)
    }
}
