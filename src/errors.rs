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

pub struct StdErrorWrapper(Mutex<Box<StdError + Send + 'static>>);
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
pub enum Error {
    #[display(fmt = "{}", _0)]
    StringError(Cow<'static, str>, Backtrace),
    #[display(fmt = "{}", _0)]
    StdError(StdErrorWrapper, Backtrace),
    #[display(fmt = "{}", _0)]
    CommandError(Cow<'static, str>),
    #[display(fmt = "None found when Some expected.")]
    SomeExpected(Backtrace),
    #[display(fmt = "Sylph-Verifier encountered a panic.")]
    Panicked,

    #[display(fmt = "Serenity has encountered an permissions error.")]
    SerenityPermissionError(Backtrace),
    #[display(fmt = "Discord snowflake ID does not exist.")]
    SerenityNotFoundError(Backtrace),
    #[display(fmt = "Serenity encountered an non-successful status code: {:?}", _0)]
    SerenityHttpError(StatusCode, Backtrace),
}
pub type Result<T> = ::std::result::Result<T, Error>;

macro_rules! from_err {
    ($($t:ty),* $(,)*) => {$(
        impl From<$t> for ::errors::Error {
            fn from(err: $t) -> Self {
                ::errors::Error::StdError(::errors::StdErrorWrapper(
                                              ::parking_lot::Mutex::new(Box::new(err))
                                          ),
                                          ::failure::Backtrace::new())
            }
        }
    )*}
}
mod impls {
    use *;
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
                Error::SerenityPermissionError(Backtrace::new()),
            SerenityError::Http(HttpError::UnsuccessfulRequest(ref res)) => match res.status {
                StatusCode::NotFound => Error::SerenityNotFoundError(Backtrace::new()),
                StatusCode::Forbidden => Error::SerenityPermissionError(Backtrace::new()),
                status => Error::SerenityHttpError(status, Backtrace::new()),
            }
            err => Error::StdError(StdErrorWrapper(Mutex::new(Box::new(err))),
                                   Backtrace::new()),
        }
    }
}
impl From<NoneError> for Error {
    fn from(_: NoneError) -> Self {
        Error::SomeExpected(Backtrace::new())
    }
}

macro_rules! bail {
    ($err:expr $(,)*) => {
        return Err(::errors::Error::StringError($err.into(),
                                                ::failure::Backtrace::new()))
    };
    ($err:expr, $($arg:expr),* $(,)*) => {
        return Err(::errors::Error::StringError(format!($err, $($arg,)*).into(),
                                                ::failure::Backtrace::new()))
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
        return Err(::errors::Error::CommandError($err.into()))
    };
    ($err:expr, $($arg:expr),* $(,)*) => {
        return Err(::errors::Error::CommandError(format!($err, $($arg,)*).into()))
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
    fn status_to_cmd<F, R: Into<Cow<'static, str>>>(
        self, code: StatusCode, f: F
    ) -> Result<T> where F: FnOnce() -> R;
}
impl <T> ResultCmdExt<T> for Result<T> {
    fn drop_nonfatal(self) -> Result<()> {
        match self {
            Ok(_) => Ok(()),
            Err(Error::CommandError(_)) => Ok(()),
            Err(Error::SerenityNotFoundError(_)) => Ok(()),
            Err(Error::SerenityPermissionError(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn status_to_cmd<F, R: Into<Cow<'static, str>>>(
        self, code: StatusCode, f: F
    ) -> Result<T> where F: FnOnce() -> R {
        match self {
            Ok(v) => Ok(v),
            Err(Error::SerenityHttpError(err_code, _)) if code == err_code =>
                Err(Error::CommandError(f().into())),
            Err(e) => Err(e),
        }
    }
}

pub trait IntoResultCmdExt<T> {
    fn to_cmd_err<F, R: Into<Cow<'static, str>>>(self, f: F) -> Result<T> where F: FnOnce() -> R;
}
impl <T, E> IntoResultCmdExt<T> for StdResult<T, E> {
    fn to_cmd_err<F, R: Into<Cow<'static, str>>>(self, f: F) -> Result<T> where F: FnOnce() -> R {
        self.map_err(|_| Error::CommandError(f().into()))
    }
}
impl <T> IntoResultCmdExt<T> for Option<T> {
    fn to_cmd_err<F, R: Into<Cow<'static, str>>>(self, f: F) -> Result<T> where F: FnOnce() -> R {
        match self {
            Some(t) => Ok(t),
            None => Err(Error::CommandError(f().into())),
        }
    }
}