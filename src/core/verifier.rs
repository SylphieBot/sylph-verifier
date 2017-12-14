use chrono::{Utc, DateTime, NaiveDateTime, Duration};
use constant_time_eq::constant_time_eq;
use core::config::*;
use core::database::*;
use core::database::schema::*;
use diesel;
use diesel::dsl::count_star;
use diesel::prelude::*;
use errors::*;
use hmac::{Hmac, Mac};
use parking_lot::RwLock;
use rand::{Rng, OsRng};
use roblox::*;
use serenity::model::*;
use sha2::Sha256;
use std::borrow::Cow;
use std::fmt::{Display, Formatter, Write, Result as FmtResult};
use std::time::{SystemTime, UNIX_EPOCH};
use util;

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
        cmd_ensure!(token.len() == 6,
                    "Verification token must be exactly 6 characters. Please check your \
                     command and try again");

        let mut chars = [0u8; 6];
        for i in 0..6 {
            let byte = token[i];
            if byte >= 'A' as u8 && byte <= 'Z' as u8 {
                chars[i] = byte
            } else if byte >= 'a' as u8 && byte <= 'z' as u8 {
                chars[i] = byte - 'a' as u8 + 'A' as u8
            } else {
                cmd_error!("Verification tokens may only contain letters. Please check your \
                            command and try again.")
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

    fn current_epoch(&self) -> Result<i64> {
        let unix_time = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        Ok((unix_time / self.time_increment as u64) as i64)
    }

    fn make_token(&self, user_id: u64, epoch: i64) -> Result<Token> {
        Ok(self.sha256_token(&format!("{}|{}|{}", TOKEN_VERSION, user_id, epoch)))
    }

    fn check_token(&self, user: RobloxUserID, token: &Token) -> Result<Option<i64>> {
        let epoch = self.current_epoch()?;
        for i in &[1, 0, -1] {
            if token == &self.make_token(user.0, epoch + i)? {
                return Ok(Some(epoch + i))
            }
        }
        Ok(None)
    }
}

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum TokenStatus {
    Verified { key_id: i32, epoch: i64 }, Outdated(RekeyReason), NotVerified,
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

        diesel::insert_into(roblox_verification_keys::table).values((
            roblox_verification_keys::key           .eq(key),
            roblox_verification_keys::time_increment.eq(time_increment),
            roblox_verification_keys::version       .eq(TOKEN_VERSION),
            roblox_verification_keys::change_reason .eq(change_reason),
        )).execute(conn.deref())?;
        Ok(TokenContext::from_db_internal(conn)?.chain_err(|| "Could not get newly created key!")?)
    }
    fn rekey(conn: &DatabaseConnection, time_increment: i32) -> Result<TokenContext> {
        info!("Regenerating token key due to user request.");
        conn.transaction_immediate(|| {
            TokenContext::new_in_db(conn, time_increment, RekeyReason::ManualRekey)
        })
    }
    fn from_db(conn: &DatabaseConnection, time_increment: i32) -> Result<TokenContext> {
        conn.transaction_immediate(|| {
            match TokenContext::from_db_internal(conn)? {
                Some(x) => {
                    if x.current.time_increment != time_increment {
                        info!("Token key in database has a different time increment, \
                               regenerating...");
                        TokenContext::new_in_db(conn, time_increment,
                                                RekeyReason::TimeIncrementChanged)
                    } else if x.current.version != TOKEN_VERSION {
                        info!("Token key in database is for an older version, \
                               regenerating...");
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
        })
    }

    fn check_token(&self, user: RobloxUserID, token: &str) -> Result<TokenStatus> {
        let token = Token::from_str(token)?;
        if let Some(epoch) = self.current.check_token(user, &token)? {
            return Ok(TokenStatus::Verified { key_id: self.current.id, epoch })
        }
        for param in &self.history {
            if param.check_token(user, &token)?.is_some() {
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
    config: ConfigManager, database: Database, token_ctx: RwLock<TokenContext>,
}
impl Verifier {
    pub fn new(config: ConfigManager, database: Database) -> Result<Verifier> {
        let ctx = TokenContext::from_db(&database.connect()?,
                                        config.get(None, ConfigKeys::TokenValiditySeconds)?)?;
        Ok(Verifier { config, database, token_ctx: RwLock::new(ctx), })
    }

    pub fn rekey(&self, force: bool) -> Result<bool> {
        let mut lock = self.token_ctx.write();
        let cur_id = lock.current.id;
        *lock = if force {
            TokenContext::rekey(&self.database.connect()?,
                                self.config.get(None, ConfigKeys::TokenValiditySeconds)?)?
        } else {
            TokenContext::from_db(&self.database.connect()?,
                                  self.config.get(None, ConfigKeys::TokenValiditySeconds)?)?
        };
        Ok(cur_id != lock.current.id)
    }

    pub fn get_verified_roblox_user(&self, user: UserId) -> Result<Option<RobloxUserID>> {
        let conn = self.database.connect()?;
        Ok(discord_user_info::table
            .filter(discord_user_info::discord_user_id.eq(user.0 as i64))
            .select(discord_user_info::roblox_user_id)
            .get_result(conn.deref()).optional()?.and_then(|x| x))
    }
    pub fn get_verified_discord_user(&self, user: RobloxUserID) -> Result<Option<UserId>> {
        let conn = self.database.connect()?;
        Ok(discord_user_info::table
            .filter(discord_user_info::roblox_user_id.eq(user))
            .select(discord_user_info::discord_user_id)
            .get_result::<i64>(conn.deref()).optional()?.map(|x| UserId(x as u64)))
    }
    pub fn try_verify(
        &self, discord_id: UserId, roblox_id: RobloxUserID, token: &str,
    ) -> Result<()> {
        let conn = self.database.connect()?;

        // Check cooldown
        conn.transaction_immediate(|| {
            let attempt_info = roblox_verification_cooldown::table
                .filter(roblox_verification_cooldown::discord_user_id.eq(discord_id.0 as i64))
                .select((roblox_verification_cooldown::attempt_count,
                         roblox_verification_cooldown::last_attempt))
                .get_result::<(i32, NaiveDateTime)>(conn.deref()).optional()?;
            let mut new_attempt_count = 1;
            if let Some((attempt_count, last_attempt)) = attempt_info {
                let max_attempts = self.config.get(None, ConfigKeys::VerificationAttemptLimit)?;
                let cooldown = self.config.get(None, ConfigKeys::VerificationCooldownSeconds)?;
                let cooldown_ends = DateTime::from_utc(last_attempt, Utc) +
                                    Duration::seconds(cooldown as i64);
                let now = Utc::now();
                if attempt_count as u32 >= max_attempts && now < cooldown_ends {
                    let time_left = cooldown_ends.signed_duration_since(now);
                    cmd_error!("You cannot make made more than {} verification attempts \
                                within {}. Please try again in {}.",
                               max_attempts,
                               util::to_english_time(cooldown),
                               util::to_english_time(time_left.num_seconds() as u64));
                }
                new_attempt_count = attempt_count + 1;
            }
            diesel::replace_into(roblox_verification_cooldown::table).values((
                roblox_verification_cooldown::discord_user_id.eq(discord_id.0 as i64),
                roblox_verification_cooldown::attempt_count  .eq(new_attempt_count),
            )).execute(conn.deref())?;
            Ok(())
        })?;

        // Check token
        conn.transaction_immediate(|| {
            let token_ctx = self.token_ctx.read();
            match token_ctx.check_token(roblox_id, token)? {
                TokenStatus::Verified { key_id, epoch } => {
                    let last_key = roblox_user_info::table
                        .filter(roblox_user_info::roblox_user_id.eq(roblox_id))
                        .select((roblox_user_info::last_key_id, roblox_user_info::last_key_epoch))
                        .get_result::<(i32, i64)>(conn.deref()).optional()?;
                    if let Some((last_id, last_epoch)) = last_key {
                        if last_id >= key_id && last_epoch >= epoch {
                            cmd_error!("An verfication attempt has already been made with the \
                                        token you used. Please wait for a new key to be generated \
                                        to try again.")
                        }
                    }
                    diesel::replace_into(roblox_user_info::table).values((
                        roblox_user_info::roblox_user_id.eq(roblox_id),
                        roblox_user_info::last_key_id   .eq(key_id),
                        roblox_user_info::last_key_epoch.eq(epoch),
                    )).execute(conn.deref())?;
                }
                TokenStatus::Outdated(rekey_reason) => {
                    cmd_error!("The verification place has not been updated with the verification \
                                bot, and verifications cannot be completed at this time moment. \
                                Please ask the bot owner to fix this problem.")
                }
                TokenStatus::NotVerified => {
                    cmd_error!("That token is not valid or has already expired. Please check your \
                                command and try again.")
                }
            }
            Ok(())
        })?;

        // Attempt to verify user
        conn.transaction_immediate(|| {
            let allow_reverification = self.config.get(None, ConfigKeys::AllowReverification)?;

            if !allow_reverification {
                let verified_as = discord_user_info::table
                    .filter(discord_user_info::discord_user_id.eq(discord_id.0 as i64))
                    .select(discord_user_info::roblox_user_id)
                    .get_result::<Option<RobloxUserID>>(conn.deref()).optional()?.and_then(|x| x);
                if let Some(roblox_id) = verified_as {
                    cmd_error!("You are already verified as {}.",
                               roblox_id.lookup_username()?);
                }

                let roblox_count = discord_user_info::table
                    .filter(discord_user_info::roblox_user_id.eq(roblox_id))
                    .select(count_star()).get_result::<i64>(conn.deref())?;
                if roblox_count != 0 {
                    cmd_error!("Someone else is already verified as {}.",
                               roblox_id.lookup_username()?);
                }
            } else {
                let last_verify = discord_user_info::table
                    .filter(discord_user_info::discord_user_id.eq(discord_id.0 as i64))
                    .select(discord_user_info::last_updated)
                    .get_result::<NaiveDateTime>(conn.deref()).optional()?;
                if let Some(last_updated) = last_verify {
                    let now = Utc::now();
                    let reverify_timeout =
                        self.config.get(None, ConfigKeys::ReverificationTimeoutSeconds)?;
                    let cooldown_ends = DateTime::<Utc>::from_utc(last_updated, Utc) +
                                        Duration::seconds(reverify_timeout as i64);
                    if now < cooldown_ends {
                        let time_left = cooldown_ends.signed_duration_since(now);
                        cmd_error!("You cannot reverify more than once every {}. Please wait {} \
                                    before trying again.",
                                   util::to_english_time(reverify_timeout),
                                   util::to_english_time(time_left.num_seconds() as u64))
                    }

                    diesel::update(discord_user_info::table
                        .filter(discord_user_info::roblox_user_id.eq(roblox_id))
                    ).set(
                        discord_user_info::roblox_user_id.eq(None::<RobloxUserID>)
                    ).execute(conn.deref())?;
                }
            }

            diesel::replace_into(discord_user_info::table).values((
                discord_user_info::discord_user_id.eq(discord_id.0 as i64),
                discord_user_info::roblox_user_id .eq(roblox_id)
            )).execute(conn.deref())?;

            Ok(())
        })?;

        Ok(())
    }

    pub fn add_config<'a>(&self, config: &'a mut Vec<LuaConfigEntry>) {
        self.token_ctx.read().current.add_config(config)
    }
}