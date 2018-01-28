#![allow(non_camel_case_types)]

use hyper::status::StatusCode;
use serenity::{Error as SerenityError};
use serenity::model::prelude::*;
use serenity::prelude::*;
use std::borrow::Cow;
use std::error;
use std::fmt;

// TODO: Add a more detailed error message for Discord http errors.

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
            Serenity(serenity::Error);
            Str_Utf8Error(std::str::Utf8Error);
            String_FromUtf8Error(std::string::FromUtf8Error);
            SystemTimeError(std::time::SystemTimeError);
        }

        errors {
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
    fn discord_to_cmd(self) -> Result<T>;
}
impl <T> ResultCmdExt<T> for Result<T> {
    fn cmd_ok(self) -> Result<()> {
        match self {
            Ok(_) | Err(Error(box (ErrorKind::CommandError(_), _))) => Ok(()),
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
            Err(Error(box (ErrorKind::Serenity(
                SerenityError::Http(HttpError::UnsuccessfulRequest(ref res))
            ), _))) if res.status == StatusCode::Forbidden => cmd_error!(
                "The bot has encountered an unknown permissions error. Please check that:\n\
                 • It has the permissions it requires: Manage Roles, Manage Nicknames, \
                   Read Messages, Send Messages, Manage Messages, Read Message History\n\
                 • There is no per-channel permissions overwrites preventing it from using \
                   those permissions on this channel.\n\
                 • It has a role with a greater rank than all roles it needs to manage."
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