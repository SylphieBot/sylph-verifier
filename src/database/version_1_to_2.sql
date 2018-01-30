PRAGMA foreign_keys = false;
BEGIN EXCLUSIVE;
  -- Remove unused change_reason and last_updated fields of verification_keys, as well as an unnecessary CHECK.
  ALTER TABLE verification_keys RENAME TO verification_keys_old;
  CREATE TABLE verification_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL,
    key BLOB NOT NULL, time_increment INT NOT NULL, version INT NOT NULL
  );
  INSERT INTO verification_keys (id, key, time_increment, version)
      SELECT id, key, time_increment, version FROM verification_keys_old;
  DROP TABLE verification_keys_old;
COMMIT;
PRAGMA foreign_keys = true;
PRAGMA foreign_key_check;