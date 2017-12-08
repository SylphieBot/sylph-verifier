use constant_time_eq::constant_time_eq;
use core::database::*;
use core::schema::*;
use diesel::dsl::count_star;
use diesel::prelude::*;
use diesel::types::*;
use errors::*;
use hmac::{Hmac, Mac};
use parking_lot::RwLock;
use rand::{Rng, OsRng};
use roblox::*;
use serenity::model::*;
use sha2::Sha256;
use std::borrow::Cow;
use std::fmt::{Display, Formatter, Write, Result as FmtResult};
use std::time::*;

const TOKEN_CHARS: &'static [u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const TOKEN_VERSION: i32 = 1;
const HISTORY_COUNT: i64 = 5;

#[derive(Clone, Hash, Debug, PartialOrd, Ord)]
struct Token([u8; 6]);
impl Token {
    fn from_arr(arr: [u8; 6]) -> Token {
        Token(arr)
    }

    fn from_str(token: &str) -> Result<Token> {
        let token = token.as_bytes();
        ensure!(token.len() == 6, "Token must be exactly 6 characters.");

        let mut chars = [0u8; 6];
        for i in 0..6 {
            let byte = token[i];
            if byte >= 'A' as u8 && byte <= 'Z' as u8 {
                chars[i] = byte
            } else if byte >= 'a' as u8 && byte <= 'z' as u8 {
                chars[i] = byte - 'a' as u8 + 'A' as u8
            } else {
                bail!("Token may only contain letters.")
            }
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
impl CustomDBType for RekeyReason {
    type Underlying_To = str;
    fn to_underlying(&self) -> Cow<str> {
        match self {
            &RekeyReason::InitialKey           => "InitialKey",
            &RekeyReason::ManualRekey          => "ManualRekey",
            &RekeyReason::OutdatedVersion      => "OutdatedVersion",
            &RekeyReason::TimeIncrementChanged => "TimeIncrementChanged",
            &RekeyReason::Unknown(ref s)       => s,
        }.into()
    }

    type Underlying_From = String;
    fn from_underlying(reason: String) -> Self {
        match reason.as_ref() {
            "InitialKey"           => return RekeyReason::InitialKey,
            "ManualRekey"          => return RekeyReason::ManualRekey,
            "OutdatedVersion"      => return RekeyReason::OutdatedVersion,
            "TimeIncrementChanged" => return RekeyReason::TimeIncrementChanged,
            _                      => { }
        }
        RekeyReason::Unknown(reason)
    }
}
custom_db_type!(RekeyReason, rekey_reason_mod, Text);

#[derive(Queryable)]
struct TokenParameters {
    id: i32, key: Vec<u8>, time_increment: i32, version: i32, change_reason: RekeyReason,
}
impl TokenParameters {
    fn add_config<'a>(&self, config: &mut Vec<LuaConfigEntry<'a>>) {
        config.push(LuaConfigEntry::new("shared_key", true, self.key.clone()));
        config.push(LuaConfigEntry::new("time_increment", false, self.time_increment));
    }

    fn sha256_token(&self, data: &str) -> Token {
        let mut mac = Hmac::<Sha256>::new(&self.key).unwrap();
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

    fn make_token(&self, user_id: u64, action: &str, epoch: i64) -> Result<Token> {
        Ok(self.sha256_token(&format!("{}|{}|{}|{}", TOKEN_VERSION, user_id, action, epoch)))
    }

    fn check_token(&self, user: RobloxUserID, action: &str, token: &Token) -> Result<Option<i64>> {
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let epoch = (unix_time / self.time_increment as u64) as i64;

        for i in &[1, 0, -1] {
            if token == &self.make_token(user.0, action, epoch + i)? {
                return Ok(Some(epoch + i))
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum TokenStatus {
    Verified { key_id: i32, epoch: i64 },
    TokenAlreadyUsed, Outdated(RekeyReason), NotVerified,
    DiscordIDAlreadyVerified, RobloxIDAlreadyVerified,
}

struct TokenContext {
    current: TokenParameters, history: Vec<TokenParameters>
}
impl TokenContext {
    fn from_db_internal(conn: &DatabaseConnection) -> Result<Option<TokenContext>> {
        let mut results = roblox_verification_keys::table
            .order(roblox_verification_keys::id.desc()).limit(1 + HISTORY_COUNT)
            .load::<TokenParameters>(conn.deref())?;
        if results.len() == 0 {
            Ok(None)
        } else {
            let history = results.split_off(1);
            Ok(Some(TokenContext { current: results.pop().unwrap(), history }))
        }
    }
    fn new_in_db(conn: &DatabaseConnection, time_increment: i32,
                 change_reason: RekeyReason) -> Result<TokenContext> {
        let mut rng = OsRng::new().chain_err(|| "OsRng creation failed")?;
        let mut key = Vec::new();
        for _ in 0..16 {
            let r = rng.next_u32();
            key.push((r >>  0) as u8);
            key.push((r >>  8) as u8);
            key.push((r >> 16) as u8);
            key.push((r >> 24) as u8);
        }

        ::diesel::insert_into(roblox_verification_keys::table).values((
            roblox_verification_keys::key           .eq(key),
            roblox_verification_keys::time_increment.eq(time_increment),
            roblox_verification_keys::version       .eq(TOKEN_VERSION),
            roblox_verification_keys::change_reason .eq(change_reason),
        )).execute(conn.deref())?;
        Ok(TokenContext::from_db_internal(conn)?.chain_err(|| "Could not get newly created key!")?)
    }
    fn rekey(conn: &DatabaseConnection, time_increment: i32) -> Result<TokenContext> {
        info!("Regenerating token key due to user request.");
        TokenContext::new_in_db(conn, time_increment, RekeyReason::ManualRekey)
    }
    fn from_db(conn: &DatabaseConnection, time_increment: i32) -> Result<TokenContext> {
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

    fn check_token(&self, user: RobloxUserID, action: &str, token: &str) -> Result<TokenStatus> {
        let token = Token::from_str(token)?;
        if let Some(epoch) = self.current.check_token(user, action, &token)? {
            return Ok(TokenStatus::Verified { key_id: self.current.id, epoch })
        }
        for param in &self.history {
            if param.check_token(user, action, &token)?.is_some() {
                return Ok(TokenStatus::Outdated(self.current.change_reason.clone()))
            }
        }
        return Ok(TokenStatus::NotVerified)
    }
}

impl CustomDBType for RobloxUserID {
    type Underlying_To = i64;
    fn to_underlying(&self) -> Cow<i64> {
        Cow::Owned(self.0 as i64)
    }

    type Underlying_From = i64;
    fn from_underlying(reason: i64) -> Self {
        RobloxUserID(reason as u64)
    }
}
custom_db_type!(RobloxUserID, roblox_user_id_mod, BigInt);

pub struct Verifier {
    database: Database, token_ctx: RwLock<TokenContext>,
}
impl Verifier {
    pub fn new(database: Database) -> Result<Verifier> {
        let token_ctx = RwLock::new(TokenContext::from_db(&database.connect()?, 300)?);
        Ok(Verifier { database, token_ctx })
    }

    pub fn get_verified_user(&self, user: UserId) -> Result<Option<RobloxUserID>> {
        let conn = self.database.connect()?;
        Ok(discord_user_info::table
            .filter(discord_user_info::discord_user_id.eq(user.0 as i64))
            .select(discord_user_info::roblox_user_id)
            .load(conn.deref())?.into_iter().next().and_then(|x| x))
    }
    pub fn attempt_verification(
        &self, discord_id: UserId, roblox_id: RobloxUserID, token: &str
    ) -> Result<TokenStatus> {
        let conn = self.database.connect()?;
        conn.transaction_immediate(|| {
            unimplemented!()
        })
    }

    pub fn add_config<'a>(&self, config: &'a mut Vec<LuaConfigEntry>) {
        self.token_ctx.read().current.add_config(config)
    }

    pub fn rekey(&self) -> Result<()> {
        let db = self.database.connect()?;
        db.transaction_immediate(|| {
            let mut token_context = self.token_ctx.write();
            *token_context = TokenContext::rekey(&db, 300)?;
            Ok(())
        })
    }
}