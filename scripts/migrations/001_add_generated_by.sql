-- Migration: Add generated_by column to extracted_items
-- This column tracks which macro generated an item (e.g., "derive(Debug)")
-- Run this after updating to the derive detection feature

-- Add generated_by column if it doesn't exist
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns 
        WHERE table_name = 'extracted_items' AND column_name = 'generated_by'
    ) THEN
        ALTER TABLE extracted_items ADD COLUMN generated_by TEXT;
        
        -- Add index for querying derive-generated items
        CREATE INDEX IF NOT EXISTS idx_extracted_items_generated_by ON extracted_items(generated_by);
        
        RAISE NOTICE 'Added generated_by column to extracted_items';
    ELSE
        RAISE NOTICE 'generated_by column already exists in extracted_items';
    END IF;
END $$;
