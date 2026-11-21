-- Backup previous versions remember the source file they were a copy of, so restore can
-- default to putting the file back where it came from.
ALTER TABLE backup_versions ADD COLUMN source_path TEXT;
