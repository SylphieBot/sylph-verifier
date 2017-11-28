use constant_time_eq::constant_time_eq;
use core::database::*;
use core::schema::*;
use diesel::prelude::*;
use errors::*;
use hmac::{Hmac, Mac};
use rand::{Rng, OsRng};
use roblox::*;
use sha2::Sha256;
use std::fmt::{Display, Formatter, Write, Result as FmtResult};
use std::time::*;

const TOKEN_CHARS: &'static [u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const KEY_CHARS: &'static [u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 \
      `~!@#$%^&*()-=_+[]\\{}|;':\",./<>?";
const TOKEN_VERSION: i32 = 1;
const HISTORY_COUNT: i64 = 5;

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

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum RekeyReason {
    InitialKey, ManualRekey, OutdatedVersion, TimeIncrementChanged, Unknown(String),
}
impl RekeyReason {
    fn from_str(reason: String) -> RekeyReason {
        match reason.as_ref() {
            "InitialKey"           => return RekeyReason::InitialKey,
            "ManualRekey"          => return RekeyReason::ManualRekey,
            "OutdatedVersion"      => return RekeyReason::OutdatedVersion,
            "TimeIncrementChanged" => return RekeyReason::TimeIncrementChanged,
            _                      => { }
        }
        RekeyReason::Unknown(reason)
    }
    fn into_str(self) -> String {
        match self {
            RekeyReason::InitialKey           => "InitialKey".to_owned(),
            RekeyReason::ManualRekey          => "ManualRekey".to_owned(),
            RekeyReason::OutdatedVersion      => "OutdatedVersion".to_owned(),
            RekeyReason::TimeIncrementChanged => "TimeIncrementChanged".to_owned(),
            RekeyReason::Unknown(s)           => s,
        }
    }
}

#[derive(Insertable)]
#[table_name="roblox_verification_keys"]
struct NewTokenContext<'a> {
    key: &'a str, time_increment: i32, version: i32, change_reason: String,
}

#[derive(Queryable)]
struct DatabaseTokenContext {
    _id: i32, key: String, time_increment: i32, version: i32, change_reason: String,
}

struct TokenParameters {
    key: String, time_increment: i32, version: i32, change_reason: RekeyReason,
}
impl TokenParameters {
    fn from_db_obj(ctx: DatabaseTokenContext) -> TokenParameters {
        TokenParameters {
            key: ctx.key, time_increment: ctx.time_increment, version: ctx.version,
            change_reason: RekeyReason::from_str(ctx.change_reason),
        }
    }

    fn add_config<'a>(&self, config: &mut Vec<LuaConfigEntry<'a>>) {
        config.push(LuaConfigEntry::new("shared_key", true, self.key.clone()));
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

    fn make_token(&self, user_id: u64, time_increment_offset: i64) -> Result<Token> {
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let time_int = (unix_time / self.time_increment as u64) as i64 + time_increment_offset;
        Ok(self.sha256_token(&format!("{}|{}|{}", TOKEN_VERSION, time_int, user_id)))
    }

    fn check_token(&self, user: RobloxUserID, token: Token) -> Result<bool> {
        let result =
            token == self.make_token(user.0,  0)? ||
            token == self.make_token(user.0, -1)? ||
            token == self.make_token(user.0,  1)?;
        Ok(result)
    }
}

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum TokenStatus {
    Verified, Outdated(RekeyReason), NotVerified
}

pub struct TokenContext {
    current: TokenParameters, history: Vec<TokenParameters>
}
impl TokenContext {
    fn from_db_internal(conn: &DatabaseConnection) -> Result<Option<TokenContext>> {
        use self::roblox_verification_keys::dsl::*;
        let mut results = roblox_verification_keys
            .order(id.desc()).limit(1 + HISTORY_COUNT)
            .load::<DatabaseTokenContext>(conn.deref())?;
        if results.len() == 0 {
            Ok(None)
        } else {
            let history = results.split_off(1);
            Ok(Some(TokenContext {
                current: TokenParameters::from_db_obj(results.pop().unwrap()),
                history: history.into_iter().map(TokenParameters::from_db_obj).collect(),
            }))
        }
    }
    fn new_in_db(conn: &DatabaseConnection, time_increment: i32,
                 change_reason_enum: RekeyReason) -> Result<TokenContext> {
        let mut rng = OsRng::new().chain_err(|| "OsRng creation failed")?;
        let mut new_key = [0u8; 64];
        for i in 0..64 {
            new_key[i] = *rng.choose(KEY_CHARS).unwrap();
        }
        let key = ::std::str::from_utf8(&new_key)?;
        let version = TOKEN_VERSION;

        let change_reason = change_reason_enum.into_str();
        ::diesel::insert(&NewTokenContext { key, time_increment, version, change_reason })
            .into(roblox_verification_keys::table)
            .execute(conn.deref())?;

        Ok(TokenContext::from_db_internal(conn)?.chain_err(|| "Could not get newly created key!")?)
    }
    pub fn rekey(conn: &DatabaseConnection, time_increment: i32) -> Result<TokenContext> {
        TokenContext::new_in_db(conn, time_increment, RekeyReason::ManualRekey)
    }
    pub fn from_db(conn: &DatabaseConnection, time_increment: i32) -> Result<TokenContext> {
        match TokenContext::from_db_internal(conn)? {
            Some(x) => {
                if x.current.time_increment != time_increment {
                    info!("Token key in database has a different time increment, regenerating...");
                    TokenContext::new_in_db(conn, time_increment,
                                            RekeyReason::TimeIncrementChanged)
                } else if x.current.version != TOKEN_VERSION {
                    info!("Token key in database is for an older version, regenerating...");
                    TokenContext::new_in_db(conn, time_increment,
                                            RekeyReason::OutdatedVersion)
                } else {
                    Ok(x)
                }
            },
            None => {
                info!("No token keys in database, generating new key...");
                TokenContext::new_in_db(conn, time_increment,
                                        RekeyReason::InitialKey)
            },
        }
    }

    pub fn add_config<'a>(&self, config: &'a mut Vec<LuaConfigEntry>) {
        self.current.add_config(config)
    }
    pub fn check_token(&self, user: RobloxUserID, token: &str) -> Result<TokenStatus> {
        let token = Token::from_str(token)?;
        if self.current.check_token(user, token)? {
            return Ok(TokenStatus::Verified)
        }
        for param in &self.history {
            if param.check_token(user, token)? {
                return Ok(TokenStatus::Outdated(self.current.change_reason.clone()))
            }
        }
        return Ok(TokenStatus::NotVerified)
    }
}