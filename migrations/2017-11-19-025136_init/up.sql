CREATE TABLE roblox_verification_keys (
  id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
  key TEXT NOT NULL, time_increment INT NOT NULL CHECK (time_increment > 0)
);

CREATE TABLE roblox_verification_cooldown (
  discord_id BIGINT PRIMARY KEY NOT NULL, last_command_use TIMESTAMP NOT NULL, times_used INT NOT NULL
) WITHOUT ROWID;

CREATE TABLE roblox_verification (
  discord_id BIGINT UNIQUE NOT NULL, roblox_id BIGINT UNIQUE NOT NULL,
  time_verified TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
  PRIMARY KEY (discord_id, roblox_id)
) WITHOUT ROWID;