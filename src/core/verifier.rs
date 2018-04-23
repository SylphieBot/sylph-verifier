use constant_time_eq::constant_time_eq;
use core::config::*;
use database::*;
use errors::*;
use hmac::{Hmac, Mac};
use parking_lot::RwLock;
use rand::{Rng, OsRng};
use roblox::*;
use serenity::model::prelude::*;
use sha2::Sha256;
use std::fmt::{Display, Formatter, Write, Result as FmtResult};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use util::MultiMutex;

const TOKEN_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const TOKEN_VERSION: u32 = 1;
const HISTORY_COUNT: u32 = 5;

// TODO: Add caching to this module. Extensive caching.

#[derive(Clone, Hash, Debug, PartialOrd, Ord)]
struct Token([u8; 6]);
impl Token {
    fn from_arr(arr: [u8; 6]) -> Token {
        Token(arr)
    }

    fn from_str(token: &str) -> Result<Token> {
        let token = token.as_bytes();
        cmd_ensure!(token.len() == 6,
                    "Verification token must be exactly 6 characters. Please check the code \
                     you entered and try again");

        let mut chars = [0u8; 6];
        for i in 0..6 {
            let byte = token[i];
            if byte >= b'A' && byte <= b'Z' {
                chars[i] = byte
            } else if byte >= b'a' && byte <= b'z' {
                chars[i] = byte - b'a' + b'A'
            } else {
                cmd_error!("Verification tokens may only contain letters. Please check the code \
                            you entered and try again.")
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

struct TokenParameters {
    id: u64, key: Vec<u8>, time_increment: u32, version: u32,
}
impl TokenParameters {
    fn add_config<'a>(&self, config: &mut Vec<LuaConfigEntry<'a>>) {
        config.push(LuaConfigEntry::new("shared_key", true, self.key.clone()));
        config.push(LuaConfigEntry::new("time_increment", false, self.time_increment));
    }

    fn sha256_token(&self, data: &str) -> Token {
        let mut mac = Hmac::<Sha256>::new_varkey(&self.key).unwrap();
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
impl FromSqlRow for TokenParameters {
    fn from_sql_row(row: Row) -> Result<Self> {
        let (
            id, key, time_increment, version
        ): (u64, Vec<u8>, u32, u32) = FromSqlRow::from_sql_row(row)?;
        Ok(TokenParameters { id, key, time_increment, version })
    }
}

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq, Hash, Debug)]
pub enum TokenStatus {
    Verified { key_id: u64, epoch: i64 }, Outdated, NotVerified,
}

struct TokenContext {
    current: TokenParameters, history: Vec<TokenParameters>
}
impl TokenContext {
    fn from_db_internal(conn: &DatabaseConnection) -> Result<Option<TokenContext>> {
        let mut results = conn.query(
            "SELECT id, key, time_increment, version FROM verification_keys \
             ORDER BY id DESC LIMIT ?1",
            1 + HISTORY_COUNT,
        ).get_all::<TokenParameters>()?;
        if results.is_empty() {
            Ok(None)
        } else {
            let history = results.split_off(1);
            Ok(Some(TokenContext { current: results.pop().unwrap(), history }))
        }
    }
    fn new_in_db(conn: &DatabaseConnection, time_increment: u32) -> Result<TokenContext> {
        let mut rng = OsRng::new()?;
        let mut key = Vec::new();
        for _ in 0..16 {
            let r = rng.next_u32();
            key.push((r >>  0) as u8);
            key.push((r >>  8) as u8);
            key.push((r >> 16) as u8);
            key.push((r >> 24) as u8);
        }

        conn.execute(
            "INSERT INTO verification_keys (key, time_increment, version) VALUES (?1, ?2, ?3)",
            (key, time_increment, TOKEN_VERSION)
        )?;
        Ok(TokenContext::from_db_internal(conn)??)
    }
    fn rekey(conn: &DatabaseConnection, time_increment: u32) -> Result<TokenContext> {
        info!("Regenerating token key due to user request.");
        conn.transaction_immediate(|| {
            TokenContext::new_in_db(conn, time_increment)
        })
    }
    fn from_db(conn: &DatabaseConnection, time_increment: u32) -> Result<TokenContext> {
        conn.transaction_immediate(|| {
            match TokenContext::from_db_internal(conn)? {
                Some(x) => {
                    if x.current.time_increment != time_increment {
                        info!("Token key in database has a different time increment, \
                               regenerating...");
                        TokenContext::new_in_db(conn, time_increment)
                    } else if x.current.version != TOKEN_VERSION {
                        info!("Token key in database is for an older version, \
                               regenerating...");
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
        })
    }

    fn check_token(&self, user: RobloxUserID, token: &str) -> Result<TokenStatus> {
        let token = Token::from_str(token)?;
        if let Some(epoch) = self.current.check_token(user, &token)? {
            return Ok(TokenStatus::Verified { key_id: self.current.id, epoch })
        }
        for param in &self.history {
            if param.check_token(user, &token)?.is_some() {
                return Ok(TokenStatus::Outdated)
            }
        }
        Ok(TokenStatus::NotVerified)
    }
}

#[derive(Copy, Clone, Debug)]
pub enum VerifyResult {
    VerificationOk, TokenAlreadyUsed, VerificationPlaceOutdated, InvalidToken,
    TooManyAttempts { max_attempts: u32, cooldown: u64, cooldown_ends: SystemTime },
    SenderVerifiedAs { other_roblox_id: RobloxUserID },
    RobloxAccountVerifiedTo { other_discord_id: UserId },
    ReverifyOnCooldown { cooldown: u64, cooldown_ends: SystemTime }
}

struct VerifierData {
    config: ConfigManager, database: Database, token_ctx: RwLock<TokenContext>,
    discord_lock: MultiMutex<UserId>, roblox_lock: MultiMutex<RobloxUserID>,
}
#[derive(Clone)]
pub struct Verifier(Arc<VerifierData>);
impl Verifier {
    pub fn new(config: ConfigManager, database: Database) -> Result<Verifier> {
        let ctx = TokenContext::from_db(&database.connect()?,
                                        config.get(None, ConfigKeys::TokenValiditySeconds)?)?;
        Ok(Verifier(Arc::new(VerifierData {
            config, database, token_ctx: RwLock::new(ctx),
            discord_lock: MultiMutex::new(), roblox_lock: MultiMutex::new(),
        })))
    }

    pub fn rekey(&self, force: bool) -> Result<bool> {
        let mut lock = self.0.token_ctx.write();
        let cur_id = lock.current.id;
        *lock = if force {
            TokenContext::rekey(&self.0.database.connect()?,
                                self.0.config.get(None, ConfigKeys::TokenValiditySeconds)?)?
        } else {
            TokenContext::from_db(&self.0.database.connect()?,
                                  self.0.config.get(None, ConfigKeys::TokenValiditySeconds)?)?
        };
        Ok(cur_id != lock.current.id)
    }

    pub fn get_verified_roblox_user(&self, user: UserId) -> Result<Option<RobloxUserID>> {
        let conn = self.0.database.connect()?;
        Ok(conn.query(
            "SELECT roblox_user_id FROM discord_user_info WHERE discord_user_id = ?1", user
        ).get_opt::<Option<RobloxUserID>>()?.and_then(|x| x))
    }
    pub fn get_verified_discord_user(&self, user: RobloxUserID) -> Result<Option<UserId>> {
        let conn = self.0.database.connect()?;
        conn.query(
            "SELECT discord_user_id FROM discord_user_info WHERE roblox_user_id = ?1", user
        ).get_opt()
    }

    pub fn try_verify(
        &self, discord_id: UserId, roblox_id: RobloxUserID, token: &str,
    ) -> Result<VerifyResult> {
        let conn = self.0.database.connect()?;

        debug!("Starting verification attempt: discord id {} -> roblox id {}",
               discord_id.0, roblox_id.0);

        let discord_lock = self.0.discord_lock.lock(discord_id);
        cmd_ensure!(discord_lock.is_some(),
                    "Please wait for your last verification attempt to finish.");

        let roblox_lock = self.0.roblox_lock.lock(roblox_id);
        cmd_ensure!(roblox_lock.is_some(),
                    "Someone else is currently trying to verify as that Roblox account. \
                     Please wait for their attempt to finish.");

        // Check cooldown
        let attempt_info = conn.query(
            "SELECT attempt_count, last_attempt FROM verification_cooldown \
             WHERE discord_user_id = ?1", discord_id
        ).get_opt::<(u32, SystemTime)>()?;
        let new_attempt_count = if let Some((attempt_count, last_attempt)) = attempt_info {
            let max_attempts = self.0.config.get(None, ConfigKeys::VerificationAttemptLimit)?;
            let cooldown = self.0.config.get(None, ConfigKeys::VerificationCooldownSeconds)?;
            let cooldown_ends = last_attempt + Duration::from_secs(cooldown);
            if attempt_count >= max_attempts && SystemTime::now() < cooldown_ends {
                return Ok(VerifyResult::TooManyAttempts { max_attempts, cooldown, cooldown_ends })
            }
            attempt_count + 1
        } else {
            1
        };
        conn.execute(
            "REPLACE INTO verification_cooldown (\
                discord_user_id, last_attempt, attempt_count\
            ) VALUES (?1, ?2, ?3)", (discord_id, SystemTime::now(), new_attempt_count)
        )?;

        // Check token
        let token_ctx = self.0.token_ctx.read();
        match token_ctx.check_token(roblox_id, token)? {
            TokenStatus::Verified { key_id, epoch } => {
                let last_key = conn.query(
                    "SELECT last_key_id, last_key_epoch FROM roblox_user_info \
                     WHERE roblox_user_id = ?1", roblox_id
                ).get_opt::<(u64, i64)>()?;
                if let Some((last_id, last_epoch)) = last_key {
                    if last_id >= key_id && last_epoch >= epoch {
                        return Ok(VerifyResult::TokenAlreadyUsed)
                    }
                }
                conn.execute(
                    "REPLACE INTO roblox_user_info \
                         (roblox_user_id, last_key_id, last_key_epoch, last_updated) \
                     VALUES (?1, ?2, ?3, ?4)", (roblox_id, key_id, epoch, SystemTime::now()),
                )?;
            }
            TokenStatus::Outdated =>
                return Ok(VerifyResult::VerificationPlaceOutdated),
            TokenStatus::NotVerified =>
                return Ok(VerifyResult::InvalidToken),
        }

        // Attempt to verify user
        let allow_reverify_discord = self.0.config.get(None, ConfigKeys::AllowReverifyDiscord)?;
        let allow_reverify_roblox = self.0.config.get(None, ConfigKeys::AllowReverifyRoblox)?;
        let check_discord = conn.query(
            "SELECT roblox_user_id, last_updated FROM discord_user_info \
             WHERE discord_user_id = ?1", discord_id
        ).get_opt::<(Option<RobloxUserID>, SystemTime)>()?;
        if let Some((current_id, last_updated)) = check_discord {
            if !allow_reverify_discord {
                if let Some(current_id) = current_id {
                    return Ok(VerifyResult::SenderVerifiedAs { other_roblox_id: current_id })
                }
            }
            if current_id == Some(roblox_id) {
                return Ok(VerifyResult::SenderVerifiedAs { other_roblox_id: roblox_id })
            }

            let cooldown =
                self.0.config.get(None, ConfigKeys::ReverificationCooldownSeconds)?;
            let cooldown_ends = last_updated + Duration::from_secs(cooldown);
            if SystemTime::now() < cooldown_ends {
                return Ok(VerifyResult::ReverifyOnCooldown { cooldown, cooldown_ends })
            }
        }

        let check_roblox = conn.query(
            "SELECT discord_user_id FROM discord_user_info \
             WHERE roblox_user_id = ?1", roblox_id,
        ).get_opt::<UserId>()?;
        if let Some(current_id) = check_roblox {
            // TODO: Add some locking here in case the current_id is verifying currently.
            if current_id != discord_id {
                if !allow_reverify_roblox {
                    return Ok(VerifyResult::RobloxAccountVerifiedTo {
                        other_discord_id: current_id
                    })
                }

                // TODO: Forcefully update this other person's roles somehow.
                conn.execute(
                    "UPDATE discord_user_info SET roblox_user_id = NULL \
                     WHERE roblox_user_id = ?1", roblox_id,
                )?;
            }
        }

        conn.execute(
            "REPLACE INTO discord_user_info (discord_user_id, roblox_user_id, last_updated) \
             VALUES (?1, ?2, ?3)", (discord_id, roblox_id, SystemTime::now()),
        )?;

        Ok(VerifyResult::VerificationOk)
    }

    pub fn add_config<'a>(&self, config: &'a mut Vec<LuaConfigEntry>) {
        self.0.token_ctx.read().current.add_config(config)
    }

    pub fn on_cleanup_tick(&self) {
        self.0.discord_lock.shrink_to_fit();
        self.0.roblox_lock.shrink_to_fit();
    }
}