BEGIN EXCLUSIVE;
  -- Stores information about a Discord user's verification history.
  CREATE TABLE user_history (
    discord_user_id BIGINT NOT NULL, roblox_user_id BIGINT NOT NULL, is_unverify BOOL NOT NULL,
    last_updated TIMESTAMP NOT NULL
  );
  CREATE INDEX user_history_discord_idx ON user_history (discord_user_id);
  CREATE INDEX user_history_roblox_idx ON user_history (roblox_user_id);

  -- Populate user history with current user verifications.
  INSERT INTO user_history (discord_user_id, roblox_user_id, is_unverify, last_updated)
  SELECT discord_user_id, roblox_user_id, 0, last_updated FROM discord_user_info
  WHERE discord_user_id NOT NULL AND roblox_user_id NOT NULL;

  -- Stores information about granted permissions.
  CREATE TABLE permissions (
    scope_1 BIGINT NOT NULL, scope_2 BIGINT NOT NULL, id BIGINT NOT NULL,
    permission_bits BIGINT NOT NULL,
    PRIMARY KEY (scope_1, scope_2, id)
  );
COMMIT;