-- Configuration
CREATE TABLE global_config (
  key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE guild_config (
  discord_guild_id BIGINT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
  PRIMARY KEY (discord_guild_id, key)
) WITHOUT ROWID;

-- Stores cooldowns for bot commands.
CREATE TABLE discord_command_cooldown (
  discord_user_id BIGINT NOT NULL, command_group TEXT NOT NULL,
  last_command_use TIMESTAMP NOT NULL, times_used INT NOT NULL,
  PRIMARY KEY (discord_user_id, command_group)
) WITHOUT ROWID;

-- Stores custom verifier role definitions for Discord guilds.
CREATE TABLE discord_custom_roles (
  discord_guild_id BIGINT NOT NULL, role_name TEXT NOT NULL, condition TEXT NOT NULL,
  last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (discord_guild_id, role_name)
) WITHOUT ROWID;

-- Stores verifier roles that are mapped to Discord roles.
CREATE TABLE discord_active_roles (
  discord_guild_id BIGINT NOT NULL, role_name TEXT NOT NULL,
  discord_role_id BIGINT NOT NULL, discord_role_name TEXT NOT NULL,
  last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (discord_guild_id, role_name)
) WITHOUT ROWID;

-- Stores the keys used in the Roblox place file to verify users.
CREATE TABLE roblox_verification_keys (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  key BLOB NOT NULL, time_increment INT NOT NULL CHECK (time_increment > 0),
  version INT NOT NULL, change_reason TEXT NOT NULL
);

-- Stores information about a Roblox user.
CREATE TABLE roblox_user_info (
  roblox_user_id BIGINT PRIMARY KEY NOT NULL,
  last_key_id INTEGER NOT NULL, last_key_epoch INTEGER NOT NULL,
  last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY (last_key_id) REFERENCES roblox_verification_keys (id)
) WITHOUT ROWID;

-- Stores information about a Discord user.
CREATE TABLE discord_user_info (
  discord_user_id BIGINT PRIMARY KEY NOT NULL,
  roblox_user_id BIGINT UNIQUE, is_bot_admin BOOLEAN NOT NULL DEFAULT false,
  last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  FOREIGN KEY (roblox_user_id) REFERENCES roblox_user_info (roblox_user_id)
) WITHOUT ROWID;

-- Stores information about which Discord users are verified as which Roblox users on which Discord guilds.
CREATE TABLE discord_active_verifications (
  discord_user_id BIGINT PRIMARY KEY NOT NULL, discord_guild_id BIGINT NOT NULL, roblox_user_id BIGINT NOT NULL,
  FOREIGN KEY (roblox_user_id) REFERENCES roblox_user_info (roblox_user_id),
  FOREIGN KEY (discord_user_id) REFERENCES discord_user_info (discord_user_id) ON DELETE CASCADE
) WITHOUT ROWID;

-- Stores terminal history
CREATE TABLE terminal_history (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, text TEXT NOT NULL
);
