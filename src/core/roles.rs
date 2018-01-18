use core::config::*;
use core::verifier::*;
use database::*;
use errors::*;
use parking_lot::RwLock;
use serenity;
use serenity::model::prelude::*;
use std::collections::{HashMap, HashSet};
use std::mem::drop;
use std::sync::Arc;
use std::time::{SystemTime, Duration};
use roblox::{VerificationSet, VerificationRule, RobloxUserID};
use util;
use util::ConcurrentCache;

enum VerificationRulesStatus {
    NotCompiled,
    Error(String),
    Compiled(VerificationSet, HashMap<String, RoleId>),
}
impl VerificationRulesStatus {
    pub fn is_compiled(&self) -> bool {
        match *self {
            VerificationRulesStatus::NotCompiled => false,
            _ => true,
        }
    }
}

pub struct ConfiguredRole {
    pub role_id: Option<RoleId>, pub custom_rule: Option<String>, pub last_updated: SystemTime,
}
pub struct AssignedRole {
    pub rule: String, pub role_id: RoleId, pub is_assigned: bool,
}
pub enum SetRolesStatus {
    Success, IsAdmin,
}

struct RoleManagerData {
    config: ConfigManager, database: Database, verifier: Verifier,
    rule_cache: ConcurrentCache<GuildId, Arc<RwLock<VerificationRulesStatus>>>,
    update_cache: ConcurrentCache<GuildId, Arc<ConcurrentCache<(UserId, bool), Option<SystemTime>>>>,
}
#[derive(Clone)]
pub struct RoleManager(Arc<RoleManagerData>);
impl RoleManager {
    pub fn new(config: ConfigManager, database: Database, verifier: Verifier) -> RoleManager {
        RoleManager(Arc::new(RoleManagerData {
            config, database, verifier,
            rule_cache: ConcurrentCache::new(), update_cache: ConcurrentCache::new(),
        }))
    }

    fn get_configuration_internal(
        &self, conn: &DatabaseConnection, guild: GuildId
    ) -> Result<(HashMap<String, ConfiguredRole>, usize, usize)> {
        // We don't have FULL OUTER JOIN in sqlite, so, we improvise a bit.
        let active_rules_list = conn.query_cached(
            "SELECT rule_name, discord_role_id, last_updated \
             FROM guild_active_rules \
             WHERE discord_guild_id = ?1", guild
        ).get_all::<(String, RoleId, SystemTime)>()?;
        let custom_rules_list = conn.query_cached(
            "SELECT rule_name, condition, last_updated \
             FROM guild_custom_rules \
             WHERE discord_guild_id = ?1", guild,
        ).get_all::<(String, String, SystemTime)>()?;

        let active_rules_count = active_rules_list.len();
        let custom_rules_count = custom_rules_list.len();

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
        Ok((map, active_rules_count, custom_rules_count))
    }
    pub fn get_configuration(&self, guild: GuildId) -> Result<HashMap<String, ConfiguredRole>> {
        Ok(self.get_configuration_internal(&self.0.database.connect()?, guild)?.0)
    }
    fn build_for_guild(
        &self, conn: &DatabaseConnection, guild: GuildId
    ) -> Result<VerificationRulesStatus> {
        let (configuration, active_count, custom_count) =
            self.get_configuration_internal(conn, guild)?;

        let limits_enabled = self.0.config.get(None, ConfigKeys::RolesEnableLimits)?;
        if limits_enabled {
            let max_assigned = self.0.config.get(None, ConfigKeys::RolesMaxAssigned)?;
            if active_count > max_assigned as usize {
                return Ok(VerificationRulesStatus::Error(format!(
                    "Too many roles are configured to be assigned. \
                     ({} roles are active, maximum is {}.)",
                    active_count, max_assigned
                )))
            }

            let max_custom = self.0.config.get(None, ConfigKeys::RolesMaxCustomRules)?;
            if custom_count > max_custom as usize {
                return Ok(VerificationRulesStatus::Error(format!(
                    "Too many custom roles exist. \
                     ({} custom roles exist, maximum is {}.)",
                    custom_count, max_custom
                )))
            }
        }

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
                if limits_enabled {
                    let max_instructions =
                        self.0.config.get(None, ConfigKeys::RolesMaxInstructions)?;
                    if set.instruction_count() > max_instructions as usize {
                        return Ok(VerificationRulesStatus::Error(format!(
                            "Role configuration is too complex. (Complexity is {}, maximum is {}.)",
                            set.instruction_count(), max_instructions)))
                    }

                    let web_requests = set.max_web_requests();
                    let max_web_requests =
                        self.0.config.get(None, ConfigKeys::RolesMaxWebRequests)?;
                    if web_requests > max_web_requests as usize {
                        return Ok(VerificationRulesStatus::Error(format!(
                            "Role configuration makes to many web requests. \
                             (Configuration makes {} web requests, maximum is {}.)",
                            web_requests, max_web_requests
                        )))
                    }
                }

                let mut active_rules = HashMap::new();
                for (rule, config) in configuration {
                    if let Some(role_id) = config.role_id {
                        active_rules.insert(rule, role_id);
                    }
                }

                Ok(VerificationRulesStatus::Compiled(set, active_rules))
            }
            Err(Error(box (ErrorKind::CommandError(err), _))) =>
                Ok(VerificationRulesStatus::Error(err)),
            Err(err) => Err(err),
        }
    }

    fn get_rule_cache(
        &self, guild: GuildId
    ) -> Result<Arc<RwLock<VerificationRulesStatus>>> {
        Ok(self.0.rule_cache.read(
            &guild, || Ok(Arc::new(RwLock::new(VerificationRulesStatus::NotCompiled)))
        )?.clone())
    }
    fn update_cached_rules(
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
        let cache = self.get_rule_cache(guild)?;
        self.update_cached_rules(&cache, guild, true)
    }
    pub fn set_active_role(
        &self, guild: GuildId, rule_name: &str, discord_role: Option<RoleId>
    ) -> Result<()> {
        let conn = self.0.database.connect()?;
        conn.transaction_immediate(|| {
            let role_exists = VerificationRule::has_builtin(rule_name) || conn.query_cached(
                "SELECT COUNT(*) FROM guild_custom_rules \
                 WHERE discord_guild_id = ?1 AND rule_name = ?2",
                (guild, rule_name)
            ).get::<u32>()? != 0;
            if !role_exists {
                cmd_error!("No rule name '{}' found.", rule_name);
            }

            let limits_enabled = self.0.config.get(None, ConfigKeys::RolesEnableLimits)?;
            if limits_enabled {
                let max_assigned = self.0.config.get(None, ConfigKeys::RolesMaxAssigned)?;
                let assigned_count = conn.query_cached(
                    "SELECT COUNT(*) FROM guild_active_rules \
                     WHERE discord_guild_id = ?1 AND rule_name != ?2",
                    (guild, rule_name)
                ).get::<u32>()?;
                if assigned_count + 1 > max_assigned {
                    cmd_error!("Too many roles are configured to be assigned. \
                                (Including '{}', {} roles are active, maximum is {}.)",
                               rule_name, assigned_count + 1, max_assigned);
                }
            }

            if let Some(discord_role) = discord_role {
                conn.execute_cached(
                    "REPLACE INTO guild_active_rules (\
                        discord_guild_id, rule_name, discord_role_id, last_updated\
                    ) VALUES (?1, ?2, ?3, ?4)",
                    (guild, rule_name, discord_role, SystemTime::now())
                )?;
            } else {
                conn.execute_cached(
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
            let conn =  self.0.database.connect()?;

            let limits_enabled = self.0.config.get(None, ConfigKeys::RolesEnableLimits)?;
            if limits_enabled {
                let max_custom = self.0.config.get(None, ConfigKeys::RolesMaxCustomRules)?;
                let custom_count = conn.query_cached(
                    "SELECT COUNT(*) FROM guild_custom_rules \
                     WHERE discord_guild_id = ?1 AND rule_name != ?2",
                    (guild, rule_name)
                ).get::<u32>()?;
                if custom_count + 1 > max_custom {
                    cmd_error!("Too many custom rules exist. \
                                (Including '{}', {} rules exist, maximum is {}.)",
                               rule_name, custom_count + 1, max_custom);
                }
            }

            match VerificationRule::from_str(condition) {
                Ok(_) => { }
                Err(err) => cmd_error!("Failed to parse custom rule: {}", err),
            }
            conn.execute_cached(
                "REPLACE INTO guild_custom_rules (\
                    discord_guild_id, rule_name, condition, last_updated\
                ) VALUES (?1, ?2, ?3, ?4)",
                (guild, rule_name, condition, SystemTime::now())
            )?;
        } else {
            self.0.database.connect()?.execute_cached(
                "DELETE FROM guild_custom_rules \
                 WHERE discord_guild_id = ?1 AND rule_name = ?2",
                (guild, rule_name)
            )?;
        }
        self.refresh_cache(guild)?;
        Ok(())
    }
    pub fn check_error(&self, guild: GuildId) -> Result<Option<String>> {
        let lock = self.get_rule_cache(guild)?;
        self.update_cached_rules(&lock, guild, false)?;
        let read = lock.read();
        Ok(match *read {
            VerificationRulesStatus::Error(ref str) => Some(str.clone()),
            _ => None,
        })
    }

    pub fn get_assigned_roles(
        &self, guild: GuildId, roblox_id: RobloxUserID
    ) -> Result<Vec<AssignedRole>> {
        let lock = self.get_rule_cache(guild)?;
        self.update_cached_rules(&lock, guild, false)?;
        let read = lock.read();
        Ok(match *read {
            VerificationRulesStatus::Compiled(ref rule_set, ref role_info) => {
                let mut vec = Vec::new();
                for (rule_name, is_assigned) in rule_set.verify(roblox_id)? {
                    let discord_role_id = role_info[rule_name];
                    vec.push(AssignedRole {
                        rule: rule_name.to_string(), role_id: discord_role_id, is_assigned,
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
        let member = guild.member(discord_id)?;
        let me_member = guild.member(serenity::CACHE.read().user.id)?;
        let can_access_user = util::can_member_access_member(&me_member, &member)?;
        let do_set_nickname = self.0.config.get(None, ConfigKeys::SetNickname)?;

        let set_nickname = if can_access_user && do_set_nickname {
            let target_nickname = if let Some(roblox_id) = roblox_id {
                Some(roblox_id.lookup_username()?)
            } else {
                None
            };
            if target_nickname != member.nick {
                Some(target_nickname.unwrap_or_else(|| "".to_string()))
            } else {
                None
            }
        } else {
            None
        };

        let orig_roles: HashSet<RoleId> = member.roles.iter().map(|x| *x).collect();
        let mut roles = orig_roles.clone();
        if let Some(roblox_id) = roblox_id {
            let assigned_roles = self.get_assigned_roles(guild, roblox_id)?;
            for role in assigned_roles {
                if role.is_assigned {
                    roles.insert(role.role_id);
                } else {
                    roles.remove(&role.role_id);
                }
            }
        } else {
            let config = self.get_configuration(guild)?;
            for (_, role) in config {
                if let Some(id) = role.role_id {
                    roles.remove(&id);
                }
            }
        }
        let set_roles: Option<Vec<RoleId>> = if orig_roles != roles {
            Some(roles.drain().collect())
        } else {
            None
        };

        trace!("Assigning nickname to {}: {:?}", member.distinct(), set_nickname);
        trace!("Assigning roles to {}: {:?}", member.distinct(), set_roles);

        if set_nickname.is_some() || set_roles.is_some() {
            member.edit(|mut edit| {
                if let Some(nickname) = set_nickname {
                    edit = edit.nickname(&nickname);
                }
                if let Some(roles) = set_roles {
                    edit = edit.roles(&roles)
                }
                edit
            })?;
        }
        Ok(if !can_access_user && do_set_nickname {
            SetRolesStatus::IsAdmin
        } else {
            SetRolesStatus::Success
        })
    }

    pub fn update_user(&self, guild: GuildId, discord_id: UserId) -> Result<SetRolesStatus> {
        self.assign_roles(guild, discord_id, self.0.verifier.get_verified_roblox_user(discord_id)?)
    }

    fn with_cooldown_cache<T, F>(
        &self, guild_id: GuildId, user_id: UserId, is_manual: bool, f: F,
    ) -> Result<T> where F: FnOnce(&mut Option<SystemTime>) -> Result<T> {
        let cache =
            self.0.update_cache.read(&guild_id, || Ok(Arc::new(ConcurrentCache::new())))?.clone();
        let mut last_update = cache.write(&(user_id, is_manual), || {
            self.0.database.connect()?.query_cached(
                "SELECT last_updated FROM roles_last_updated \
                 WHERE discord_guild_id = ?1 AND discord_user_id = ?2 AND is_manual = ?3",
                (guild_id, user_id, is_manual)
            ).get_opt::<SystemTime>()
        })?;
        f(&mut last_update)
    }
    pub fn update_user_with_cooldown(
        &self, guild_id: GuildId, user_id: UserId, cooldown: u64, is_manual: bool,
    ) -> Result<SetRolesStatus> {
        let now = SystemTime::now();
        if cooldown != 0 {
            let last_updated =
                self.with_cooldown_cache(guild_id, user_id, is_manual, |x| Ok(*x))?;
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

        let result = self.update_user(guild_id, user_id)?;
        self.0.database.connect()?.execute_cached(
            "INSERT INTO roles_last_updated (\
                discord_guild_id, discord_user_id, is_manual, last_updated\
            ) VALUES (?1, ?2, ?3, ?4)", (guild_id, user_id, is_manual, now),
        )?;
        self.with_cooldown_cache(guild_id, user_id, is_manual, |x| {
            *x = Some(now);
            Ok(())
        })?;
        Ok(result)
    }

    pub fn explain_rule_set(&self, guild: GuildId) -> Result<String> {
        let lock = self.get_rule_cache(guild)?;
        self.update_cached_rules(&lock, guild, false)?;
        let read = lock.read();
        match *read {
            VerificationRulesStatus::Compiled(ref set, _) => Ok(format!("{}", set)),
            VerificationRulesStatus::Error(ref err) => cmd_error!("Could not compile: {}", err),
            VerificationRulesStatus::NotCompiled => unimplemented!(),
        }
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
    pub fn clear_rule_cache(&self) {
        self.0.rule_cache.clear_cache()
    }
}