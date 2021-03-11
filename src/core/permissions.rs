use database::*;
use enumset::*;
use errors::*;
use serenity::model::prelude::*;
use std::sync::Arc;
use util::*;

const SCOPE_GLOBAL_GUILD: u64 = 0;
const SCOPE_GLOBAL_USERS: u64 = 1;
const SCOPE_USER        : u64 = 2;
const SCOPE_GUILD       : u64 = 3;
const SCOPE_GUILD_USERS : u64 = 4;
const SCOPE_GUILD_ROLE  : u64 = 5;
const SCOPE_GUILD_USER  : u64 = 6;

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub enum Scope {
    GlobalAllGuilds, GlobalAllUsers,
    User(UserId),
    Guild(GuildId), GuildAllUsers(GuildId),
    GuildRole(GuildId, RoleId), GuildUser(GuildId, UserId),
}
impl Scope {
    fn to_sql(self) -> (u64, u64, u64) {
        match self {
            Scope::GlobalAllGuilds     => (SCOPE_GLOBAL_GUILD, 0, 0),
            Scope::GlobalAllUsers      => (SCOPE_GLOBAL_USERS, 0, 0),
            Scope::User(uid)           => (SCOPE_USER, 0, uid.0),
            Scope::Guild(gid)          => (SCOPE_GUILD, 0, gid.0),
            Scope::GuildAllUsers(gid)  => (SCOPE_GUILD_USERS, 0, gid.0),
            Scope::GuildRole(gid, rid) => (SCOPE_GUILD_ROLE, gid.0, rid.0),
            Scope::GuildUser(gid, uid) => (SCOPE_GUILD_USER, gid.0, uid.0),
        }
    }
}

// This enum's order is reflected in the database format!
// Replace permissions with dummy values rather than removing them.
#[derive(EnumSetType)]
pub enum BotPermission {
    // Bypass permissions
    BotAdmin, GuildAdmin,

    // Global permissions
    ManageBot, ManageGlobalSetings, ManageVerification,

    // Guild permissions
    BypassHierarchy, ManageGuildSettings, ManageRoles,

    // Command permissions
    Unverify, UnverifyOther, Whois, Whowas,

    // Guild permissions 2
    BypassNicknameUpdate,
}

use self::BotPermission::*;

const ALWAYS_GLOBAL_GUILD: EnumSet<BotPermission> =
    enum_set!(GuildAdmin | ManageGuildSettings | ManageRoles);
const DEFAULT_GLOBAL_ALL_GUILDS: EnumSet<BotPermission> =
    enum_set!();
const DEFAULT_GLOBAL_ALL_USERS: EnumSet<BotPermission> =
    enum_set!(Unverify | Whois | Whowas);
const GUILD_ONLY: EnumSet<BotPermission> =
    enum_set!();

pub struct PermissionManagerData {
    database: Database, scope_cache: ConcurrentCache<Scope, EnumSet<BotPermission>>,
}

#[derive(Clone)]
pub struct PermissionManager(Arc<PermissionManagerData>);
impl PermissionManager {
    pub fn new(database: Database) -> PermissionManager {
        let db_ref_scope = database.clone();
        PermissionManager(Arc::new(PermissionManagerData {
            database,
            scope_cache: ConcurrentCache::new(move |&scope|
                PermissionManager::get_scope_db(&db_ref_scope, scope)
            ),
        }))
    }

    fn get_scope_db(
        database: &Database, scope: Scope,
    ) -> Result<EnumSet<BotPermission>> {
        let permission_bits = database.connect()?.query(
            "SELECT permission_bits FROM permissions \
             WHERE scope_1 = ?1 AND scope_2 = ?2 AND id = ?3",
            scope.to_sql(),
        ).get_opt::<u64>()?;
        Ok(permission_bits.map_or(match scope {
            Scope::GlobalAllGuilds => DEFAULT_GLOBAL_ALL_GUILDS,
            Scope::GlobalAllUsers => DEFAULT_GLOBAL_ALL_USERS,
            _ => EnumSet::empty(),
        }, |bits| EnumSet::from_u64(bits)))
    }

    pub fn get_scope(&self, scope: Scope) -> Result<EnumSet<BotPermission>> {
        Ok(*self.0.scope_cache.read(&scope)?)
    }
    pub fn set_scope(
        &self, scope: Scope, permissions: EnumSet<BotPermission>,
    ) -> Result<()> {
        let (scope_1, scope_2, id) = scope.to_sql();
        self.0.database.connect()?.execute(
            "REPLACE INTO permissions (\
                scope_1, scope_2, id, permission_bits\
            ) VALUES (?1, ?2, ?3, ?4)",
            (scope_1, scope_2, id, permissions.as_u64()),
        )?;
        *self.0.scope_cache.write(&scope)? = permissions;
        Ok(())
    }
    pub fn get_guild_perms(&self, guild_id: GuildId) -> Result<EnumSet<BotPermission>> {
        let mut perms = self.get_scope(Scope::GlobalAllGuilds)?;
        perms |= self.get_scope(Scope::Guild(guild_id))?;
        perms |= ALWAYS_GLOBAL_GUILD;
        Ok(perms)
    }

    fn get_user_raw(
        &self, user: UserId, additional: EnumSet<BotPermission>,
    ) -> Result<EnumSet<BotPermission>> {
        let mut perms = self.get_scope(Scope::GlobalAllUsers)?;
        perms |= self.get_scope(Scope::User(user))?;
        perms |= additional;
        if perms.contains(BotPermission::BotAdmin) {
            Ok(!GUILD_ONLY)
        } else {
            Ok(perms - GUILD_ONLY)
        }
    }
    pub fn get_user_global_perms(&self, user: UserId) -> Result<EnumSet<BotPermission>> {
        self.get_user_raw(user, EnumSet::new())
    }
    pub fn get_user_perms(
        &self, guild_id: GuildId, user: UserId,
    ) -> Result<EnumSet<BotPermission>> {
        ensure_member_exists(guild_id, user)?;

        let guild_lock = guild_id.to_guild_cached()?;
        let guild = guild_lock.read();

        let mut guild_perms = self.get_scope(Scope::GuildAllUsers(guild_id))?;
        guild_perms |= self.get_scope(Scope::GuildUser(guild_id, user))?;

        if let Some(user) = guild.members.get(&user) {
            for &role in &user.roles {
                guild_perms |= self.get_scope(Scope::GuildRole(guild_id, role))?;
            }
        } else {
            cmd_error!("Could not get user roles!");
        }

        if guild_perms.contains(BotPermission::GuildAdmin) || guild.owner_id == user {
            guild_perms = self.get_guild_perms(guild_id)?;
        } else {
            guild_perms &= self.get_guild_perms(guild_id)?;
        }
        self.get_user_raw(user, guild_perms)
    }

    pub fn on_cleanup_tick(&self) {
        self.0.scope_cache.shrink_to_fit();
    }
    pub fn on_guild_remove(&self, _: GuildId) {
        // TODO: Implement
        self.0.scope_cache.shrink_to_fit();
    }
}