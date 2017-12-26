use core::config::*;
use database::*;
use errors::*;
use parking_lot::RwLock;
use serenity::model::prelude::*;
use std::collections::HashMap;
use std::mem::drop;
use std::time::SystemTime;
use roblox::{VerificationSet, VerificationRule, RobloxUserID};
use util::ConcurrentCache;

enum VerificationSetStatus {
    NotCompiled,
    Error(String),
    Compiled(VerificationSet, HashMap<String, (RoleId, String)>),
}
impl VerificationSetStatus {
    pub fn is_compiled(&self) -> bool {
        match *self {
            VerificationSetStatus::NotCompiled => false,
            _ => true,
        }
    }
}

pub struct AssignedRole {
    rule: String, role_id: RoleId, role_name: String, is_assigned: bool,
}

pub struct RoleManager {
    config: ConfigManager, database: Database,
    rule_cache: ConcurrentCache<GuildId, RwLock<VerificationSetStatus>>,
}
impl RoleManager {
    pub fn new(config: ConfigManager, database: Database) -> RoleManager {
        RoleManager { config, database, rule_cache: ConcurrentCache::new() }
    }

    fn build_for_guild(
        &self, conn: &DatabaseConnection, guild: GuildId
    ) -> Result<VerificationSetStatus> {
        let active_rules_list = conn.query_cached(
            "SELECT rule_name, discord_role_id, discord_role_name FROM discord_active_rules \
             WHERE discord_guild_id = ?1", guild
        ).get_all::<(String, RoleId, String)>()?;
        let custom_roles_list = conn.query_cached(
            "SELECT rule_name, condition FROM discord_custom_rules \
             WHERE discord_guild_id = ?1", guild,
        ).get_all::<(String, String)>()?;

        let limits_enabled = self.config.get(None, ConfigKeys::RolesEnableLimits)?;
        if limits_enabled {
            let max_assigned = self.config.get(None, ConfigKeys::RolesMaxAssigned)?;
            if active_rules_list.len() > max_assigned as usize {
                return Ok(VerificationSetStatus::Error(format!(
                    "Too many roles are configured to be assigned. \
                     ({} roles are active, maximum is {}.)",
                    active_rules_list.len(), max_assigned
                )))
            }
        }

        let mut active_rules = HashMap::new();
        for (rule_name, role_id, role_name) in active_rules_list {
            active_rules.insert(rule_name, (role_id, role_name));
        }

        let mut custom_rules = HashMap::new();
        for (rule_name, condition) in custom_roles_list {
            custom_rules.insert(rule_name, condition);
        }

        let mut active_rule_names = Vec::new();
        for role in active_rules.keys() {
            active_rule_names.push(role.as_str());
        }

        let max_active_custom_rules = self.config.get(None, ConfigKeys::RolesMaxActiveCustomRules)?;
        let mut active_custom_rules = 0;
        match VerificationSet::compile(&active_rule_names, |role| {
            active_custom_rules += 1;
            if limits_enabled && active_custom_rules > max_active_custom_rules {
                cmd_error!("Too many custom rules are loaded. (Maximum is {}.)",
                           active_custom_rules);
            }
            custom_rules.get(role).map_or(
                Ok(None), |condition| VerificationRule::from_str(condition).map(Some))
        }) {
            Ok(set) => {
                drop(active_rule_names);

                if limits_enabled {
                    let max_instructions = self.config.get(None, ConfigKeys::RolesMaxInstructions)?;
                    if set.instruction_count() > max_instructions as usize {
                        return Ok(VerificationSetStatus::Error(format!(
                            "Role configuration is too complex. (Complexity is {}, maximum is {}.)",
                            set.instruction_count(), max_instructions)))
                    }

                    let web_requests = set.max_web_requests();
                    let max_web_requests = self.config.get(None, ConfigKeys::RolesMaxWebRequests)?;
                    if web_requests > max_web_requests as usize {
                        return Ok(VerificationSetStatus::Error(format!(
                            "Role configuration makes to many web requests. \
                             (Configuration makes {} web requests, maximum is {}.)",
                            web_requests, max_web_requests
                        )))
                    }
                }

                Ok(VerificationSetStatus::Compiled(set, active_rules))
            }
            Err(Error(box (ErrorKind::CommandError(err), _))) =>
                Ok(VerificationSetStatus::Error(err)),
            Err(err) => Err(err),
        }
    }

    fn with_cached<F, R>(
        &self, guild: GuildId, f: F
    ) -> Result<R> where F: FnOnce(&RwLock<VerificationSetStatus>) -> Result<R> {
        self.rule_cache.get_cached(&guild,
                                   || Ok(RwLock::new(VerificationSetStatus::NotCompiled)), f)
    }
    fn update_cached_verification(
        &self, lock: &RwLock<VerificationSetStatus>, guild: GuildId, force: bool,
    ) -> Result<()> {
        if force || !lock.read().is_compiled() {
            let status = self.build_for_guild(&self.database.connect()?, guild)?;
            let mut write = lock.write();
            if force || !write.is_compiled() {
                *write = status;
            }
        }
        Ok(())
    }

    fn refresh_cache(&self, guild: GuildId) -> Result<()> {
        self.with_cached(guild, |lock| self.update_cached_verification(lock, guild, true))
    }
    pub fn set_active_role(
        &self, guild: GuildId, rule_name: &str, discord_role: Option<RoleId>
    ) -> Result<()> {
        let limits_enabled = self.config.get(None, ConfigKeys::RolesEnableLimits)?;
        let max_assigned = self.config.get(None, ConfigKeys::RolesMaxAssigned)?;

        let conn = self.database.connect()?;
        conn.transaction_immediate(|| {
            let role_exists = VerificationRule::has_builtin(rule_name) || conn.query_cached(
                "SELECT COUNT(*) FROM discord_custom_rules \
                 WHERE discord_guild_id = ?1 AND role_name = ?2",
                (guild, rule_name)
            ).get::<u32>()? != 0;
            if !role_exists {
                cmd_error!("No such rule '{}' exists.", rule_name);
            }

            if limits_enabled {
                let assigned_count = conn.query_cached(
                    "SELECT COUNT(*) FROM discord_active_rules \
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
                let rwlock = guild.find().chain_err(|| "Guild not found.")?;
                let lock = rwlock.read();
                let role_obj = lock.roles.get(&discord_role).chain_err(|| "Role not found.")?;
                let role_name = &role_obj.name;
                conn.execute_cached(
                    "REPLACE INTO discord_active_rules (\
                        discord_guild_id, rule_name, discord_role_id, discord_role_name, \
                        last_updated\
                    ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    (guild, rule_name, discord_role, role_name, SystemTime::now())
                )?;
            } else {
                conn.execute_cached(
                    "DELETE FROM discord_active_rules \
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
            match VerificationRule::from_str(condition) {
                Ok(_) => { }
                Err(err) => cmd_error!("Failed to parse custom rule: {}", err),
            }
            self.database.connect()?.execute_cached(
                "REPLACE INTO discord_custom_rules (\
                    discord_guild_id, rule_name, condition, last_updated\
                ) VALUES (?1, ?2, ?3, ?4)",
                (guild, rule_name, condition, SystemTime::now())
            )?;
        } else {
            self.database.connect()?.execute_cached(
                "DELETE FROM discord_custom_rules \
                 WHERE discord_guild_id = ?1 AND rule_name = ?2",
                (guild, rule_name)
            )?;
        }
        self.refresh_cache(guild)?;
        Ok(())
    }
    pub fn check_error(&self, guild: GuildId) -> Result<Option<String>> {
        self.with_cached(guild, |lock| {
            self.update_cached_verification(lock, guild, false)?;
            Ok(match *lock.read() {
                VerificationSetStatus::Error(ref str) => Some(str.clone()),
                _ => None,
            })
        })
    }

    pub fn get_assigned_roles(
        &self, guild: GuildId, roblox_id: RobloxUserID
    ) -> Result<Vec<AssignedRole>> {
        self.with_cached(guild, |lock| {
            self.update_cached_verification(lock, guild, false)?;
            Ok(match *lock.read() {
                VerificationSetStatus::Compiled(ref rule_set, ref role_info) => {
                    let mut vec = Vec::new();
                    for (rule_name, is_assigned) in rule_set.verify(roblox_id)? {
                        let (discord_role_id, ref discord_role_name) = role_info[rule_name];
                        vec.push(AssignedRole {
                            rule: rule_name.to_string(),
                            role_id: discord_role_id, role_name: discord_role_name.clone(),
                            is_assigned,
                        })
                    }
                    vec
                }
                VerificationSetStatus::Error(_) =>
                    cmd_error!("There is a problem with this server's role configuration. \
                                Please contact the server admins."),
                VerificationSetStatus::NotCompiled => unreachable!(),
            })
        })
    }
}