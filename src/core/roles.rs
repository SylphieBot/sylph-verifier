use chrono::Utc;
use core::config::*;
use core::tasks::*;
use core::verifier::*;
use database::*;
use errors::*;
use parking_lot::RwLock;
use serenity;
use serenity::model::prelude::*;
use std::borrow::Cow;
use std::collections::HashMap;
use std::mem::drop;
use std::sync::Arc;
use std::time::{SystemTime, Duration};
use roblox::{VerificationSet, VerificationRule, RuleResult, RobloxUserID};
use util;
use util::ConcurrentCache;
use error_report::catch_error;

// TODO: Prevent assigning the same role id to two rules.
// TODO: Allow unlinking of roles assigned to deleted rules.
// TODO: Add a error check for looking up username (handle same way as a role error?).

enum VerificationRulesStatus {
    NotCompiled,
    Error(Cow<'static, str>),
    Compiled(VerificationSet, HashMap<String, RoleId>),
}
impl VerificationRulesStatus {
    fn is_compiled(&self) -> bool {
        match self {
            VerificationRulesStatus::NotCompiled => false,
            _ => true,
        }
    }
}

pub struct ConfiguredRole {
    pub role_id: Option<RoleId>, pub custom_rule: Option<String>, pub last_updated: SystemTime,
}
pub struct AssignedRole {
    pub rule: String, pub role_id: RoleId, pub is_assigned: RuleResult,
}

#[derive(Copy, Clone)]
pub enum SetRolesStatus {
    Success {
        nickname_admin_error: bool, determine_roles_error: bool,
        set_roles_error: bool, was_unverified: bool,
    },
    NotVerified,
}

struct RoleManagerData {
    config: ConfigManager, database: Database, verifier: Verifier, tasks: TaskManager,
    rule_cache: ConcurrentCache<GuildId, Arc<RwLock<VerificationRulesStatus>>>,
    update_cache: ConcurrentCache<GuildId, Arc<ConcurrentCache<(UserId, bool), Option<SystemTime>>>>,
}
#[derive(Clone)]
pub struct RoleManager(Arc<RoleManagerData>);
impl RoleManager {
    pub fn new(
        config: ConfigManager, database: Database, verifier: Verifier, tasks: TaskManager,
    ) -> RoleManager {
        let db_ref_update = database.clone();
        RoleManager(Arc::new(RoleManagerData {
            config, database, verifier, tasks,
            rule_cache: ConcurrentCache::new(|_|
                Ok(Arc::new(RwLock::new(VerificationRulesStatus::NotCompiled)))
            ),
            update_cache: ConcurrentCache::new(move |&guild_id| {
                let db_ref_update = db_ref_update.clone();
                Ok(Arc::new(ConcurrentCache::new(move |&(user_id, is_manual)|
                    Self::get_cooldown_cache(&db_ref_update, guild_id, user_id, is_manual)
                )))
            }),
        }))
    }

    fn get_configuration_internal(
        &self, conn: &DatabaseConnection, guild: GuildId
    ) -> Result<HashMap<String, ConfiguredRole>> {
        // We don't have FULL OUTER JOIN in sqlite, so, we improvise a bit.
        let active_rules_list = conn.query(
            "SELECT rule_name, discord_role_id, last_updated \
             FROM guild_active_rules \
             WHERE discord_guild_id = ?1", guild
        ).get_all::<(String, RoleId, SystemTime)>()?;
        let custom_rules_list = conn.query(
            "SELECT rule_name, condition, last_updated \
             FROM guild_custom_rules \
             WHERE discord_guild_id = ?1", guild,
        ).get_all::<(String, String, SystemTime)>()?;

        let mut map = HashMap::new();
        for (rule_name, role_id, last_updated) in active_rules_list {
            map.insert(rule_name, ConfiguredRole {
                role_id: Some(role_id), custom_rule: None, last_updated,
            });
        }
        for (rule_name, condition, last_updated) in custom_rules_list {
            if let Some(role) = map.get_mut(&rule_name) {
                role.custom_rule = Some(condition);
                if role.last_updated < last_updated {
                    role.last_updated = last_updated
                }
            } else {
                map.insert(rule_name, ConfiguredRole {
                    role_id: None, custom_rule: Some(condition), last_updated,
                });
            }
        }
        Ok(map)
    }
    pub fn get_configuration(&self, guild: GuildId) -> Result<HashMap<String, ConfiguredRole>> {
        Ok(self.get_configuration_internal(&self.0.database.connect()?, guild)?)
    }
    fn build_for_guild(
        &self, conn: &DatabaseConnection, guild: GuildId
    ) -> Result<VerificationRulesStatus> {
        let configuration = self.get_configuration_internal(conn, guild)?;

        let mut active_rule_names = Vec::new();
        for (rule, config) in &configuration {
            if config.role_id.is_some() {
                active_rule_names.push(rule.as_str());
            }
        }

        match VerificationSet::compile(&active_rule_names, |role| {
            configuration.get(role).and_then(|x| x.custom_rule.as_ref()).map_or(
                Ok(None), |condition| VerificationRule::from_str(condition).map(Some))
        }) {
            Ok(set) => {
                let mut active_rules = HashMap::new();
                for (rule, config) in configuration {
                    if let Some(role_id) = config.role_id {
                        active_rules.insert(rule, role_id);
                    }
                }
                Ok(VerificationRulesStatus::Compiled(set, active_rules))
            }
            Err(match_err!(ErrorKind::CommandError(err))) =>
                Ok(VerificationRulesStatus::Error(err)),
            Err(e) => Err(e),
        }
    }

    fn update_rules(
        &self, lock: &RwLock<VerificationRulesStatus>, guild: GuildId, force: bool,
    ) -> Result<()> {
        if force || !lock.read().is_compiled() {
            let status = self.build_for_guild(&self.0.database.connect()?, guild)?;
            let mut write = lock.write();
            if force || !write.is_compiled() {
                *write = status;
            }
        }
        Ok(())
    }

    fn refresh_cache(&self, guild: GuildId) -> Result<()> {
        let cache = self.0.rule_cache.read(&guild)?;
        self.update_rules(&cache, guild, true)
    }
    pub fn set_active_role(
        &self, guild: GuildId, rule_name: &str, discord_role: Option<RoleId>
    ) -> Result<()> {
        let conn = self.0.database.connect()?;
        conn.transaction_immediate(|| {
            let role_exists = VerificationRule::has_builtin(rule_name) || conn.query(
                "SELECT COUNT(*) FROM guild_custom_rules \
                 WHERE discord_guild_id = ?1 AND rule_name = ?2",
                (guild, rule_name)
            ).get::<u32>()? != 0;
            if !role_exists {
                cmd_error!("No rule name '{}' found.", rule_name);
            }

            if let Some(discord_role) = discord_role {
                conn.execute(
                    "REPLACE INTO guild_active_rules (\
                        discord_guild_id, rule_name, discord_role_id, last_updated\
                    ) VALUES (?1, ?2, ?3, ?4)",
                    (guild, rule_name, discord_role, SystemTime::now())
                )?;
            } else {
                conn.execute(
                    "DELETE FROM guild_active_rules \
                     WHERE discord_guild_id = ?1 AND rule_name = ?2",
                    (guild, rule_name)
                )?;
            }
            Ok(())
        })?;
        drop(conn);
        self.refresh_cache(guild)?;
        Ok(())
    }
    pub fn set_custom_rule(
        &self, guild: GuildId, rule_name: &str, condition: Option<&str>
    ) -> Result<()> {
        if let Some(condition) = condition {
            let conn = self.0.database.connect()?;
            match VerificationRule::from_str(condition) {
                Ok(_) => { }
                Err(err) => cmd_error!("Failed to parse custom rule: {}", err),
            }
            conn.execute(
                "REPLACE INTO guild_custom_rules (\
                    discord_guild_id, rule_name, condition, last_updated\
                ) VALUES (?1, ?2, ?3, ?4)",
                (guild, rule_name, condition, SystemTime::now())
            )?;
        } else {
            self.0.database.connect()?.execute(
                "DELETE FROM guild_custom_rules \
                 WHERE discord_guild_id = ?1 AND rule_name = ?2",
                (guild, rule_name)
            )?;
        }
        self.refresh_cache(guild)?;
        Ok(())
    }
    pub fn check_error(&self, guild: GuildId) -> Result<Option<Cow<'static, str>>> {
        let lock = self.0.rule_cache.read(&guild)?;
        self.update_rules(&lock, guild, false)?;
        let read = lock.read();
        Ok(match *read {
            VerificationRulesStatus::Error(ref str) => Some(str.clone()),
            _ => None,
        })
    }

    pub fn get_assigned_roles(
        &self, guild: GuildId, roblox_id: RobloxUserID
    ) -> Result<Vec<AssignedRole>> {
        let lock = self.0.rule_cache.read(&guild)?;
        self.update_rules(&lock, guild, false)?;
        let read = lock.read();
        Ok(match *read {
            VerificationRulesStatus::Compiled(ref rule_set, ref role_info) => {
                let mut vec = Vec::new();
                for (rule_name, is_assigned) in rule_set.verify(roblox_id)? {
                    let role_id = role_info[rule_name];
                    vec.push(AssignedRole {
                        rule: rule_name.to_string(), role_id, is_assigned,
                    })
                }
                vec
            }
            VerificationRulesStatus::Error(_) =>
                cmd_error!("There is a problem with this server's role configuration. \
                            Please contact the server admins."),
            VerificationRulesStatus::NotCompiled => unreachable!(),
        })
    }

    pub fn assign_roles(
        &self, guild: GuildId, discord_id: UserId, roblox_id: Option<RobloxUserID>
    ) -> Result<SetRolesStatus> {
        let mut member = guild.member(discord_id)?;
        let my_id = serenity::CACHE.read().user.id;
        let can_access_user = util::can_member_access_member(guild, my_id, discord_id)?;
        let do_set_nickname = self.0.config.get(None, ConfigKeys::SetNickname)?;

        if can_access_user && do_set_nickname {
            let target_nickname = if let Some(roblox_id) = roblox_id {
                Some(format!("{}\u{17B5}", roblox_id.lookup_username()?))
            } else {
                None
            };
            if target_nickname != member.nick {
                trace!("Assigning nickname to {}: {:?}", member.distinct(), target_nickname);
                member.edit(|x| x.nickname(target_nickname.as_ref().map_or("", |x| x.as_str())))?;
            }
        }

        let mut determine_roles_error = false;
        let mut set_roles_error = false;
        let mut was_unverified = false;
        if let Some(roblox_id) = roblox_id {
            for role in self.get_assigned_roles(guild, roblox_id)? {
                match role.is_assigned {
                    RuleResult::True  => if !member.roles.contains(&role.role_id) {
                        trace!("Adding role to {}: {}", member.distinct(), role.role_id.0);
                        set_roles_error |= member.add_role(role.role_id).is_err();
                    },
                    RuleResult::False => if member.roles.contains(&role.role_id) {
                        trace!("Removing role from {}: {}", member.distinct(), role.role_id.0);
                        set_roles_error |= member.remove_role(role.role_id).is_err();
                    },
                    RuleResult::Error => {
                        determine_roles_error = true;
                    }
                }
            }
        } else {
            let config = self.get_configuration(guild)?;
            for (_, role) in config {
                if let Some(id) = role.role_id {
                    if member.roles.contains(&id) {
                        trace!("Removing role from {}: {}", member.distinct(), id.0);
                        set_roles_error |= member.remove_role(id).is_err();
                        was_unverified = true;
                    }
                }
            }
        }

        Ok(SetRolesStatus::Success {
            nickname_admin_error: !can_access_user && do_set_nickname,
            determine_roles_error, set_roles_error, was_unverified,
        })
    }

    pub fn update_user(
        &self, guild: GuildId, discord_id: UserId, update_unverified: bool,
    ) -> Result<SetRolesStatus> {
        if let Some(roblox_id) = self.0.verifier.get_verified_roblox_user(discord_id)? {
            self.assign_roles(guild, discord_id, Some(roblox_id))
        } else if update_unverified {
            self.assign_roles(guild, discord_id, None)
        } else {
            let member = guild.member(discord_id)?;
            trace!("User {} is not verified. Not changing roles.", member.distinct());
            Ok(SetRolesStatus::NotVerified)
        }
    }

    fn get_cooldown_cache(
        database: &Database, guild_id: GuildId, user_id: UserId, is_manual: bool,
    ) -> Result<Option<SystemTime>> {
        database.connect()?.query(
            "SELECT last_updated FROM roles_last_updated \
             WHERE discord_guild_id = ?1 AND discord_user_id = ?2 AND is_manual = ?3",
            (guild_id, user_id, is_manual)
        ).get_opt::<SystemTime>()
    }
    pub fn update_user_with_cooldown(
        &self, guild_id: GuildId, user_id: UserId, cooldown: u64, is_manual: bool,
        update_unverified: bool,
    ) -> Result<SetRolesStatus> {
        let now = SystemTime::now();
        let guild_cache = self.0.update_cache.read(&guild_id)?;
        if cooldown != 0 {
            let last_updated = *guild_cache.read(&(user_id, is_manual))?;
            if let Some(last_updated) = last_updated {
                let cooldown_ends = last_updated + Duration::from_secs(cooldown);
                if now < cooldown_ends {
                    cmd_error!("You can only update your roles once every {}. Try again in {}.",
                               util::to_english_time(cooldown),
                               util::english_time_diff(now, cooldown_ends))
                }
            }
        }

        // TODO: Resolve this stuff.
        if !is_manual {
            debug!("Automatically updating roles for <@{}> in {}.", user_id, guild_id);
        } else {
            debug!("Manually updating roles for <@{}> in {}.", user_id, guild_id);
        }

        let result = self.update_user(guild_id, user_id, update_unverified)?;
        self.0.database.connect()?.execute(
            "REPLACE INTO roles_last_updated (\
                discord_guild_id, discord_user_id, is_manual, last_updated\
            ) VALUES (?1, ?2, ?3, ?4)", (guild_id, user_id, is_manual, now),
        )?;
        *guild_cache.write(&(user_id, is_manual))? = Some(now);
        Ok(result)
    }

    pub fn explain_rule_set(&self, guild: GuildId) -> Result<String> {
        let lock = self.0.rule_cache.read(&guild)?;
        self.update_rules(&lock, guild, false)?;
        let read = lock.read();
        match *read {
            VerificationRulesStatus::Compiled(ref set, _) => Ok(format!("{}", set)),
            VerificationRulesStatus::Error(ref err) => cmd_error!("Could not compile: {}", err),
            VerificationRulesStatus::NotCompiled => unimplemented!(),
        }
    }

    fn on_unverified_update(
        result: SetRolesStatus, user_id: UserId, 
        unverified_msg: Option<String>, log_channel: Option<u64>,
    ) -> Result<()> {
        if let SetRolesStatus::Success { was_unverified: true, .. } = result {
            if let Some(unverified_msg) = unverified_msg {
                user_id.create_dm_channel()?.send_message(|m| m.content(unverified_msg))?;
            }
            if let Some(log_channel_id) = log_channel {
                let log_channel_id = ChannelId(log_channel_id);
                let discord_username = user_id.to_user()?.tag();
                log_channel_id.send_message(|m|
                    m.content(format_args!("`[{}]` 🇫 {} (`{}`) has been unverified due to having \
                                            been a legacy verification.",
                                           Utc::now().format("%H:%M:%S"),
                                           discord_username, user_id.0))
                )?;
            }
        }
        Ok(())
    }
    pub fn check_roles_update_msg(&self, guild_id: GuildId, user_id: UserId) -> Result<()> {
        if user_id != serenity::CACHE.read().user.id {
            if self.0.config.get(Some(guild_id), ConfigKeys::EnableAutoUpdate)? {
                let auto_update_cooldown =
                    self.0.config.get(Some(guild_id), ConfigKeys::AutoUpdateCooldownSeconds)?;
                let update_unverified =
                    self.0.config.get(Some(guild_id), ConfigKeys::EnableAutoUpdateUnverified)?;
                let unverified_msg =
                    self.0.config.get(Some(guild_id),
                                      ConfigKeys::EnableAutoUpdateUnverifiedMessage)?;
                let log_channel = 
                    self.0.config.get(None, ConfigKeys::GlobalVerificationLogChannel)?;
                let roles = self.clone();
                self.0.tasks.dispatch_task(move |_| {
                    let result = roles.update_user_with_cooldown(
                        guild_id, user_id, auto_update_cooldown, false, update_unverified,
                    );
                    if let Ok(result) = &result {
                        catch_error(||
                            Self::on_unverified_update(
                                *result, user_id, unverified_msg, log_channel,
                            ).drop_nonfatal()
                        ).ok();
                    }
                    result.drop_nonfatal()
                })
            }
        }
        Ok(())
    }
    pub fn check_roles_update_join(&self, guild_id: GuildId, member: Member) -> Result<()> {
        if member.user.read().id != serenity::CACHE.read().user.id {
            if self.0.config.get(Some(guild_id), ConfigKeys::SetRolesOnJoin)? {
                let roles = self.clone();
                let user_id = member.user.read().id;
                self.0.tasks.dispatch_task(move |_| {
                    roles.update_user_with_cooldown(
                        guild_id, user_id, 0, false, false
                    ).drop_nonfatal()
                })
            }
        }
        Ok(())
    }

    pub fn on_cleanup_tick(&self) {
        let outdated_threshold = SystemTime::now() - Duration::from_secs(60 * 60 * 4);
        self.0.update_cache.for_each(|cache| {
            cache.retain(|_, value| value.map_or(false, |time| time > outdated_threshold));
            cache.shrink_to_fit();
        });
        self.0.rule_cache.shrink_to_fit();
        self.0.update_cache.shrink_to_fit();
    }
    pub fn on_guild_remove(&self, guild: GuildId) {
        self.0.rule_cache.remove(&guild);
        self.0.update_cache.remove(&guild);
    }
}
