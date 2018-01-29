#![allow(non_camel_case_types)]

use serenity::{Error as SerenityError};
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::borrow::Cow;
use std::error;
use std::fmt;

mod internal {
    use *;
    error_chain! {
        foreign_links {
            Fmt(std::fmt::Error);
            Io(std::io::Error);
            ParseIntError(std::num::ParseIntError);
            R2D2(r2d2::Error);
            Reqwest(reqwest::Error);
            Rusqlite(rusqlite::Error);
            Rusqlite_FromSqlError(rusqlite::types::FromSqlError);
            SerdeJson(serde_json::Error);
            Str_Utf8Error(std::str::Utf8Error);
            String_FromUtf8Error(std::string::FromUtf8Error);
            SystemTimeError(std::time::SystemTimeError);
        }

        errors {
            Serenity(err: serenity::Error) {
                description("serenity encountered an error")
                display("{}", err)
            }

            HttpError(status: hyper::status::StatusCode) {
                description("serenity encountered an non-successful status")
                display("Serenity encountered an non-successful status: {}", status)
            }

            CommandError(err: std::borrow::Cow<'static, str>) {
                description("command encountered an error")
                display("{}", err)
            }

            LZ4Error {
                description("LZ4 error")
            }

            Panicked {
                description("panic encountered")
            }
        }
    }
}
// Reexport these types so IDEs pick up on them correctly.
pub use self::internal::{Error, ErrorKind, Result, ResultExt};

// Reexport this for convinence.
pub use hyper::status::StatusCode;

impl From<SerenityError> for Error {
    fn from(err: SerenityError) -> Self {
        match err {
            SerenityError::Http(HttpError::UnsuccessfulRequest(ref res)) =>
                Error::from_kind(ErrorKind::HttpError(res.status)),
            err =>
                Error::from_kind(ErrorKind::Serenity(err)),
        }
    }
}

#[derive(Debug)]
struct StringError(String);
impl fmt::Display for StringError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl error::Error for StringError {
    fn description(&self) -> &str {
        &self.0
    }
}

impl Error {
    pub fn to_sync_error(&self) -> impl error::Error + Send + Sync + 'static {
        StringError(format!("{}", self))
    }
}

macro_rules! cmd_error {
    ($err:expr $(,)*) => {
        bail!($crate::errors::ErrorKind::CommandError($err.into()))
    };
    ($err:expr, $($arg:expr),* $(,)*) => {
        bail!($crate::errors::ErrorKind::CommandError(format!($err, $($arg,)*).into()))
    };
}
macro_rules! cmd_ensure {
    ($cond:expr, $err:expr $(,)*) => {
        ensure!($cond, $crate::errors::ErrorKind::CommandError($err.into()))
    };
    ($cond:expr, $err:expr, $($arg:expr),* $(,)*) => {
        ensure!($cond, $crate::errors::ErrorKind::CommandError(format!($err, $($arg,)*).into()))
    };
}


pub trait ResultCmdExt<T> {
    fn cmd_ok(self) -> Result<()>;
    fn status_to_cmd<F, R: Into<Cow<'static, str>>>(
        self, code: StatusCode, f: F
    ) -> Result<T> where F: FnOnce() -> R;
    fn discord_to_cmd(self) -> Result<T>;
}
impl <T> ResultCmdExt<T> for Result<T> {
    fn cmd_ok(self) -> Result<()> {
        match self {
            Ok(_) | Err(Error(box (ErrorKind::CommandError(_), _))) => Ok(()),
            Err(e) => Err(e),
        }
    }
    fn status_to_cmd<F, R: Into<Cow<'static, str>>>(
        self, code: StatusCode, f: F
    ) -> Result<T> where F: FnOnce() -> R {
        match self {
            Ok(v) => Ok(v),
            Err(Error(box (ErrorKind::HttpError(err_code), _))) if code == err_code =>
                Err(ErrorKind::CommandError(f().into()).into()),
            Err(e) => Err(e),
        }
    }
    fn discord_to_cmd(self) -> Result<T> {
        match self {
            Ok(v) => Ok(v),
            Err(Error(box (ErrorKind::Serenity(
                SerenityError::Model(ModelError::Hierarchy)
            ), _))) => cmd_error!(
                "The bot's role does not have sufficient rank in the hierarchy to do that. Please \
                 ensure that it has a role with a greater rank than all roles it needs to manage."
            ),
            Err(Error(box (ErrorKind::Serenity(
                SerenityError::Model(ModelError::InvalidPermissions(_))
            ), _))) => cmd_error!(
                "The bot does not have sufficient permissions to do that. Please check that: \n\
                 • It has the permissions it requires: Manage Roles, Manage Nicknames, \
                   Read Messages, Send Messages, Manage Messages, Read Message History\n\
                 • There is no per-channel permissions overwrites preventing it from using \
                   those permissions on this channel."
            ),
            Err(Error(box (ErrorKind::HttpError(StatusCode::Forbidden), _))) => cmd_error!(
                "The bot has encountered an unknown permissions error. Please check that:\n\
                 • It has the permissions it requires: Manage Roles, Manage Nicknames, \
                   Read Messages, Send Messages, Manage Messages, Read Message History\n\
                 • There is no per-channel permissions overwrites preventing it from using \
                   those permissions on this channel.\n\
                 • It has a role with a greater rank than all roles it needs to manage."
            ),
            Err(Error(box (ErrorKind::HttpError(StatusCode::NotFound), _))) => cmd_error!(
                "A user, message, role or channel the bot is configured to use has been deleted."
            ),
            Err(e) => Err(e),
        }
    }
}

pub trait IntoResultCmdExt<T> {
    fn to_cmd_err<F, R: Into<Cow<'static, str>>>(self, f: F) -> Result<T> where F: FnOnce() -> R;
}
impl <T, E: ResultExt<T>> IntoResultCmdExt<T> for E {
    fn to_cmd_err<F, R: Into<Cow<'static, str>>>(self, f: F) -> Result<T> where F: FnOnce() -> R {
        self.chain_err(|| ErrorKind::CommandError(f().into()))
    }
}