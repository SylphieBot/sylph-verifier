#![allow(non_camel_case_types)]

mod internal {
    use *;
    error_chain! {
        foreign_links {
            Diesel(diesel::result::Error);
            Diesel_ConnectionError(diesel::result::ConnectionError);
            Diesel_RunMigrationsError(diesel_migrations::RunMigrationsError);
            Fmt(std::fmt::Error);
            Io(std::io::Error);
            R2D2(r2d2::Error);
            Reqwest(reqwest::Error);
            SerdeJson(serde_json::Error);
            Serenity(serenity::Error);
            Str_Utf8Error(std::str::Utf8Error);
            String_FromUtf8Error(std::string::FromUtf8Error);
            SystemTimeError(std::time::SystemTimeError);
        }

        errors {
            CommandAborted {
                description("command aborted")
            }

            InvalidToken {
                description("token must be six upper case letters")
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

