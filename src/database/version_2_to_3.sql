BEGIN EXCLUSIVE;
  -- Stores information about a Discord user's verification history.
  CREATE TABLE discord_user_history (
    discord_user_id BIGINT NOT NULL, roblox_user_id BIGINT NOT NULL, last_updated TIMESTAMP NOT NULL,
    FOREIGN KEY (roblox_user_id) REFERENCES roblox_user_info (roblox_user_id)
  );

  -- Stores information about granted permissions.
  CREATE TABLE permissions (
    scope_1 BIGINT NOT NULL, scope_2 BIGINT NOT NULL, id BIGINT NOT NULL,
    permission_bits BIGINT NOT NULL,
    PRIMARY KEY (scope_1, scope_2, id)
  );
COMMIT;