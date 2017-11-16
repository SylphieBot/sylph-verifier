use std;

error_chain! {
    foreign_links {
        Fmt(std::fmt::Error);
        Io(std::io::Error);
        StringFromUtf8Error(std::string::FromUtf8Error);
        SystemTimeError(std::time::SystemTimeError);
    }

    errors {
        InvalidToken {
            description("token must be six upper case letters")
        }

        LZ4Error {
            description("LZ4 error")
        }

        RblxWrongHeader {
            description("place file had invalid header")
        }

        RblxWrongFooter {
            description("place file had invalid footer")
        }

        NonZeroUnknownField {
            description("unknown field in place file not zero")
        }

        UnknownTypeID(v: u32) {
            description("found unknown type id")
            display("found unknown type id: {}", v)
        }
    }
}