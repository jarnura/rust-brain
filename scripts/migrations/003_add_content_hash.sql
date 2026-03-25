-- Add content_hash column for incremental change detection
-- This column stores a hash of the file content to detect changes

ALTER TABLE source_files ADD COLUMN IF NOT EXISTS content_hash TEXT;

-- Create index for faster lookups by content hash
CREATE INDEX IF NOT EXISTS idx_source_files_content_hash ON source_files(content_hash);

-- Add comment for documentation
COMMENT ON COLUMN source_files.content_hash IS 'SHA-256 hash of original_source for incremental change detection';
