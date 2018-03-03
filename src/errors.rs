use failure::Backtrace;
use parking_lot::Mutex;
use serenity::{Error as SerenityError};
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::borrow::Cow;
use std::error::{Error as StdError};
use std::fmt;
use std::option::NoneError;
use std::result::{Result as StdResult};

pub use failure::{Fail, ResultExt};
pub use hyper::status::StatusCode;

pub struct StdErrorWrapper(Mutex<Box<dyn StdError + Send + 'static>>);
impl fmt::Debug for StdErrorWrapper {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&*self.0.lock(), f)
    }
}
impl fmt::Display for StdErrorWrapper {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&*self.0.lock(), f)
    }
}

#[derive(Fail, Debug, Display)]
pub enum ErrorKind {
    #[display(fmt = "{}", _0)]
    StringError(Cow<'static, str>),
    #[display(fmt = "{}", _0)]
    StdError(StdErrorWrapper),
    #[display(fmt = "{}", _0)]
    CommandError(Cow<'static, str>),
    #[display(fmt = "None found when Some expected.")]
    SomeExpected,
    #[display(fmt = "Sylph-Verifier encountered a panic.")]
    Panicked,

    #[display(fmt = "Serenity has encountered an permissions error.")]
    SerenityPermissionError,
    #[display(fmt = "Discord snowflake ID does not exist.")]
    SerenityNotFoundError,
    #[display(fmt = "Serenity encountered an non-successful status code: {:?}", _0)]
    SerenityHttpError(StatusCode),
}

pub struct Error(pub Box<(ErrorKind, Option<Backtrace>)>);
impl Fail for Error {
    fn cause(&self) -> Option<&dyn Fail> {
        (*self.0).0.cause()
    }

    fn backtrace(&self) -> Option<&Backtrace> {
        (*self.0).1.as_ref()
    }
}
impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&(*self.0).0, f)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&(*self.0).0, f)
    }
}
impl From<ErrorKind> for Error {
    fn from(err: ErrorKind) -> Self {
        let backtrace = match err {
            ErrorKind::CommandError(_) | ErrorKind::Panicked => None,
            _ => Some(Backtrace::new()),
        };
        Error(Box::new((err, backtrace)))
    }
}

pub type Result<T> = ::std::result::Result<T, Error>;

mod impls {
    use *;
    use errors::*;
    use parking_lot::Mutex;

    macro_rules! from_err {
        ($($t:ty),* $(,)*) => {$(
            impl From<$t> for Error {
                fn from(err: $t) -> Self {
                    ErrorKind::StdError(StdErrorWrapper(Mutex::new(Box::new(err)))).into()
                }
            }
        )*}
    }
    from_err! {
        std::fmt::Error, std::io::Error, std::num::ParseIntError, std::str::Utf8Error,
        std::string::FromUtf8Error, std::time::SystemTimeError, r2d2::Error, reqwest::Error,
        rusqlite::Error, rusqlite::types::FromSqlError, serde_json::Error,
    }
}

impl From<SerenityError> for Error {
    fn from(err: SerenityError) -> Self {
        match err {
            SerenityError::Model(ModelError::Hierarchy) |
            SerenityError::Model(ModelError::InvalidPermissions(_)) =>
                ErrorKind::SerenityPermissionError,
            SerenityError::Http(HttpError::UnsuccessfulRequest(ref res)) => match res.status {
                StatusCode::NotFound => ErrorKind::SerenityNotFoundError,
                StatusCode::Forbidden => ErrorKind::SerenityPermissionError,
                status => ErrorKind::SerenityHttpError(status),
            }
            err => ErrorKind::StdError(StdErrorWrapper(Mutex::new(Box::new(err)))),
        }.into()
    }
}
impl From<NoneError> for Error {
    fn from(_: NoneError) -> Self {
        ErrorKind::SomeExpected.into()
    }
}

macro_rules! match_err {
    ($pat:pat) => { Error(box ($pat, _)) }
}

macro_rules! bail {
    ($err:expr $(,)*) => {
        return Err(::errors::ErrorKind::StringError($err.into()).into())
    };
    ($err:expr, $($arg:expr),* $(,)*) => {
        return Err(::errors::ErrorKind::StringError(format!($err, $($arg,)*).into()).into())
    };
}
macro_rules! ensure {
    ($cond:expr) => {
        if !$cond {
            bail!(stringify!($cond))
        }
    };
    ($cond:expr, $($rest:tt)*) => {
        if !$cond {
            bail!($($rest)*)
        }
    };
}

macro_rules! cmd_error {
    ($err:expr $(,)*) => {
        return Err(::errors::ErrorKind::CommandError($err.into()).into())
    };
    ($err:expr, $($arg:expr),* $(,)*) => {
        return Err(::errors::ErrorKind::CommandError(format!($err, $($arg,)*).into()).into())
    };
}
macro_rules! cmd_ensure {
    ($cond:expr, $($rest:tt)*) => {
        if !$cond {
            cmd_error!($($rest)*)
        }
    }
}

pub trait ResultCmdExt<T> {
    fn drop_nonfatal(self) -> Result<()>;
    fn status_to_cmd<R: Into<Cow<'static, str>>>(
        self, code: StatusCode, f: impl FnOnce() -> R
    ) -> Result<T>;
}
impl <T> ResultCmdExt<T> for Result<T> {
    fn drop_nonfatal(self) -> Result<()> {
        match self {
            Ok(_) => Ok(()),
            Err(match_err!(ErrorKind::CommandError(_))) => Ok(()),
            Err(match_err!(ErrorKind::SerenityNotFoundError)) => Ok(()),
            Err(match_err!(ErrorKind::SerenityPermissionError)) => Ok(()),
            Err(e) => Err(e)
        }
    }
    fn status_to_cmd<R: Into<Cow<'static, str>>>(
        self, code: StatusCode, f: impl FnOnce() -> R
    ) -> Result<T> {
        match self {
            Ok(v) => Ok(v),
            Err(match_err!(ErrorKind::SerenityHttpError(err_code))) if code == err_code =>
                Err(ErrorKind::CommandError(f().into()).into()),
            Err(e) => Err(e)
        }
    }
}

pub trait IntoResultCmdExt<T> {
    fn to_cmd_err<R: Into<Cow<'static, str>>>(self, f: impl FnOnce() -> R) -> Result<T>;
}
impl <T, E> IntoResultCmdExt<T> for StdResult<T, E> {
    fn to_cmd_err<R: Into<Cow<'static, str>>>(self, f: impl FnOnce() -> R) -> Result<T> {
        self.map_err(|_| ErrorKind::CommandError(f().into()).into())
    }
}
impl <T> IntoResultCmdExt<T> for Option<T> {
    fn to_cmd_err<R: Into<Cow<'static, str>>>(self, f: impl FnOnce() -> R) -> Result<T> {
        match self {
            Some(t) => Ok(t),
            None => Err(ErrorKind::CommandError(f().into()).into()),
        }
    }
}