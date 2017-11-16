use constant_time_eq::constant_time_eq;
use errors::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::fmt::{Display, Formatter, Write, Result as FmtResult};
use std::time::*;

const CHARACTERS: &'static str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const TOKEN_VERSION: &'static str = "1";

#[derive(Copy, Clone, Hash, Debug, PartialOrd, Ord)]
pub struct Token([u8; 6]);
impl Token {
    pub fn from_arr(arr: [u8; 6]) -> Token {
        Token(arr)
    }

    pub fn from_str(token: &str) -> Result<Token> {
        let token = token.as_bytes();
        ensure!(token.len() == 6, ErrorKind::InvalidToken);

        let mut chars = [0u8; 6];
        for i in 0..6 {
            let byte = token[i];
            ensure!(byte >= 'A' as u8 && byte <= 'Z' as u8, ErrorKind::InvalidToken);
            chars[i] = byte;
        }
        Ok(Token(chars))
    }
}
impl PartialEq for Token {
    fn eq(&self, other: &Token) -> bool {
        constant_time_eq(&self.0, &other.0)
    }
}
impl Eq for Token { }
impl Display for Token {
    fn fmt(&self, f: &mut Formatter) -> FmtResult {
        for &c in &self.0 {
            f.write_char(c as char)?;
        }
        Ok(())
    }
}

pub struct TokenContext(Vec<u8>, u64);
impl TokenContext {
    pub fn new(key: &[u8], time_increment: u64) -> TokenContext {
        TokenContext(key.to_owned(), time_increment)
    }

    fn sha256_token(&self, data: &str) -> Token {
        let mut mac = Hmac::<Sha256>::new(&self.0);
        mac.input(data.as_bytes());
        let result = mac.result();
        let code = result.code();

        let mut accum = 0;
        for i in 0..6 {
            accum *= 256;
            accum += code[i] as u64;
        }

        let mut chars = [0u8; 6];
        for i in 0..6 {
            chars[i] = CHARACTERS.as_bytes()[(accum % CHARACTERS.len() as u64) as usize];
            accum /= CHARACTERS.len() as u64;
        }
        Token::from_arr(chars)
    }

    pub fn make_token(&self, user_id: u64, time_increment_offset: i64) -> Result<Token> {
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let time_int = (unix_time / self.1) as i64 + time_increment_offset;
        Ok(self.sha256_token(&format!("{}|{}|{}", TOKEN_VERSION, time_int, user_id)))
    }
}