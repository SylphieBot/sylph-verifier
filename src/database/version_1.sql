-- Configuration
CREATE TABLE global_config (
  key TEXT PRIMARY KEY, value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE guild_config (
  discord_guild_id BIGINT, key TEXT, value TEXT NOT NULL,
  PRIMARY KEY (discord_guild_id, key)
) WITHOUT ROWID;

-- Stores custom verifier role definitions for Discord guilds.
CREATE TABLE discord_custom_roles (
  discord_guild_id BIGINT, role_name TEXT, condition TEXT NOT NULL,
  last_updated TIMESTAMP NOT NULL,
  PRIMARY KEY (discord_guild_id, role_name)
) WITHOUT ROWID;

-- Stores verifier roles that are mapped to Discord roles.
CREATE TABLE discord_active_roles (
  discord_guild_id BIGINT, role_name TEXT,
  discord_role_id BIGINT NOT NULL, discord_role_name TEXT NOT NULL,
  last_updated TIMESTAMP NOT NULL,
  PRIMARY KEY (discord_guild_id, role_name)
) WITHOUT ROWID;

-- Stores the keys used in the Roblox place file to verify users.
CREATE TABLE roblox_verification_keys (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  key BLOB NOT NULL, time_increment INT NOT NULL CHECK (time_increment > 0),
  version INT NOT NULL, change_reason TEXT NOT NULL, last_updated TIMESTAMP NOT NULL
);

-- Stores cooldown for verification.
CREATE TABLE roblox_verification_cooldown (
  discord_user_id BIGINT PRIMARY KEY,
  last_attempt TIMESTAMP NOT NULL, attempt_count INT NOT NULL
) WITHOUT ROWID;

-- Stores information about a Roblox user.
CREATE TABLE roblox_user_info (
  roblox_user_id BIGINT PRIMARY KEY,
  last_key_id INTEGER NOT NULL, last_key_epoch BIGINT NOT NULL,
  last_updated TIMESTAMP NOT NULL,
  FOREIGN KEY (last_key_id) REFERENCES roblox_verification_keys (id)
) WITHOUT ROWID;

-- Stores information about a Discord user.
CREATE TABLE discord_user_info (
  discord_user_id BIGINT PRIMARY KEY, roblox_user_id BIGINT UNIQUE,
  last_updated TIMESTAMP NOT NULL,
  FOREIGN KEY (roblox_user_id) REFERENCES roblox_user_info (roblox_user_id)
) WITHOUT ROWID;

-- Stores information about which Discord users are verified as which Roblox users on which Discord guilds.
CREATE TABLE discord_assigned_roles (
  discord_user_id BIGINT, discord_guild_id BIGINT,
  roblox_user_id BIGINT NOT NULL, discord_role_id BIGINT NOT NULL,
  is_active BOOL NOT NULL, assigned_at TIMESTAMP NOT NULL, unassigned_at TIMESTAMP,
  FOREIGN KEY (roblox_user_id) REFERENCES roblox_user_info (roblox_user_id),
  FOREIGN KEY (discord_user_id) REFERENCES discord_user_info (discord_user_id) ON DELETE CASCADE
);
CREATE INDEX discord_assigned_roles_idx ON discord_assigned_roles (discord_user_id, discord_guild_id, is_active);