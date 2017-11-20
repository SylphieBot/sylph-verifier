CREATE TABLE roblox_verification_keys (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  key TEXT NOT NULL, time_increment INT NOT NULL CHECK (time_increment > 0)
);

CREATE TABLE roblox_verification_cooldown (
  discord_user_id BIGINT PRIMARY KEY NOT NULL, last_command_use TIMESTAMP NOT NULL, times_used INT NOT NULL
) WITHOUT ROWID;

CREATE TABLE roblox_verification (
  discord_user_id BIGINT UNIQUE NOT NULL, roblox_user_id BIGINT UNIQUE NOT NULL,
  last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (discord_user_id, roblox_user_id)
) WITHOUT ROWID;

CREATE TABLE discord_custom_roles (
  discord_guild_id BIGINT UNIQUE NOT NULL, role_name TEXT UNIQUE NOT NULL,
  condition TEXT NOT NULL, last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (discord_guild_id, role_name)
) WITHOUT ROWID;

CREATE TABLE discord_active_roles (
  discord_guild_id BIGINT UNIQUE NOT NULL, role_name TEXT UNIQUE NOT NULL,
  discord_role_id BIGINT NOT NULL, discord_role_name TEXT NOT NULL,
  last_updated TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (discord_guild_id, role_name)
)