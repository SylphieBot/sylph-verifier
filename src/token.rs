use constant_time_eq::constant_time_eq;
use database::*;
use database::schema::*;
use diesel::prelude::*;
use errors::*;
use hmac::{Hmac, Mac};
use rand::{Rng, OsRng};
use roblox::*;
use sha2::Sha256;
use std::fmt::{Display, Formatter, Write, Result as FmtResult};
use std::time::*;

const TOKEN_CHARS: &'static [u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const KEY_CHARS: &'static [u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
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

#[derive(Insertable)]
#[table_name="roblox_verification_keys"]
struct NewTokenContext<'a> {
    key: &'a str, time_increment: i32,
}

#[derive(Queryable)]
struct DatabaseTokenContext {
    _id: i32, key: String, time_increment: i32,
}

pub struct TokenContext {
    key: String, time_increment: i32,
}
impl TokenContext {
    fn from_db_internal(conn: &DatabaseConnection) -> Result<Option<TokenContext>> {
        use self::roblox_verification_keys::dsl::*;
        let mut results = roblox_verification_keys
            .order(id.desc()).limit(1)
            .load::<DatabaseTokenContext>(conn.deref())?;
        assert!(results.len() <= 1);
        Ok(results.pop().map(|ctx| TokenContext {
            key: ctx.key, time_increment: ctx.time_increment
        }))
    }
    pub fn new_in_db(conn: &DatabaseConnection, time_increment: i32) -> Result<TokenContext> {
        let mut rng = OsRng::new().chain_err(|| "OsRng creation failed")?;
        let mut new_key = [0u8; 64];
        for i in 0..64 {
            new_key[i] = *rng.choose(KEY_CHARS).unwrap();
        }
        let key = ::std::str::from_utf8(&new_key)?;

        ::diesel::insert(&NewTokenContext { key, time_increment })
            .into(roblox_verification_keys::table)
            .execute(conn.deref())?;

        Ok(TokenContext::from_db_internal(conn)?.chain_err(|| "Could not get newly created key!")?)
    }
    pub fn from_db(conn: &DatabaseConnection, time_increment: i32) -> Result<TokenContext> {
        match TokenContext::from_db_internal(conn)? {
            Some(x) => {
                if x.time_increment != time_increment {
                    info!("Token key in database has a different time increment, regenerating...");
                    TokenContext::new_in_db(conn, time_increment)
                } else {
                    Ok(x)
                }
            },
            None => {
                info!("No token keys in database, generating new key...");
                TokenContext::new_in_db(conn, time_increment)
            },
        }
    }

    pub fn add_config<'a>(&'a self, config: &mut Vec<LuaConfigEntry<'a>>) {
        config.push(LuaConfigEntry::new("shared_key", true, &self.key));
        config.push(LuaConfigEntry::new("time_increment", false, self.time_increment));
    }

    fn sha256_token(&self, data: &str) -> Token {
        let mut mac = Hmac::<Sha256>::new(self.key.as_bytes()).unwrap();
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
            chars[i] = TOKEN_CHARS[(accum % TOKEN_CHARS.len() as u64) as usize];
            accum /= TOKEN_CHARS.len() as u64;
        }
        Token::from_arr(chars)
    }

    pub fn make_token(&self, user_id: u64, time_increment_offset: i64) -> Result<Token> {
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let time_int = (unix_time / self.time_increment as u64) as i64 + time_increment_offset;
        Ok(self.sha256_token(&format!("{}|{}|{}", TOKEN_VERSION, time_int, user_id)))
    }
}