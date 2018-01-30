BEGIN EXCLUSIVE;
  -- Configuration
  CREATE TABLE global_config (
    key TEXT PRIMARY KEY, value BLOB
  ) WITHOUT ROWID;
  CREATE TABLE guild_config (
    discord_guild_id BIGINT, key TEXT, value BLOB,
    PRIMARY KEY (discord_guild_id, key)
  ) WITHOUT ROWID;

  -- Stores the keys used in the Roblox place file to verify users.
  CREATE TABLE verification_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    key BLOB NOT NULL, time_increment INT NOT NULL CHECK (time_increment > 0),
    version INT NOT NULL, change_reason INT NOT NULL, last_updated TIMESTAMP NOT NULL
  );

  -- Stores cooldown for verification.
  CREATE TABLE verification_cooldown (
    discord_user_id BIGINT PRIMARY KEY,
    last_attempt TIMESTAMP NOT NULL, attempt_count INT NOT NULL
  ) WITHOUT ROWID;

  -- Stores information about a Roblox user.
  CREATE TABLE roblox_user_info (
    roblox_user_id BIGINT PRIMARY KEY,
    last_key_id INTEGER NOT NULL, last_key_epoch BIGINT NOT NULL,
    last_updated TIMESTAMP NOT NULL,
    FOREIGN KEY (last_key_id) REFERENCES verification_keys (id)
  ) WITHOUT ROWID;

  -- Stores information about a Discord user.
  CREATE TABLE discord_user_info (
    discord_user_id BIGINT PRIMARY KEY, roblox_user_id BIGINT UNIQUE,
    last_updated TIMESTAMP NOT NULL,
    FOREIGN KEY (roblox_user_id) REFERENCES roblox_user_info (roblox_user_id)
  ) WITHOUT ROWID;

  -- Stores custom verifier rule definitions for Discord guilds.
  CREATE TABLE guild_custom_rules (
    discord_guild_id BIGINT, rule_name TEXT, condition TEXT NOT NULL,
    last_updated TIMESTAMP NOT NULL,
    PRIMARY KEY (discord_guild_id, rule_name)
  ) WITHOUT ROWID;

  -- Stores verifier rules that are mapped to Discord roles.
  CREATE TABLE guild_active_rules (
    discord_guild_id BIGINT, rule_name TEXT, discord_role_id BIGINT NOT NULL,
    last_updated TIMESTAMP NOT NULL,
    PRIMARY KEY (discord_guild_id, rule_name)
  ) WITHOUT ROWID;

  -- Stores the last time Discord roles were updated for a Discord user.
  CREATE TABLE roles_last_updated (
    discord_guild_id BIGINT, discord_user_id BIGINT, is_manual BOOL, last_updated TIMESTAMP NOT NULL,
    PRIMARY KEY (discord_guild_id, discord_user_id, is_manual)
  ) WITHOUT ROWID;

  -- Information about verification channels.
  CREATE TABLE verification_channel_info (
    discord_guild_id BIGINT PRIMARY KEY, discord_channel_id BIGINT NOT NULL, header_message_id BIGINT NOT NULL
  ) WITHOUT ROWID;
COMMIT;