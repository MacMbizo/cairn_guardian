-- D1 Encryption & sqlite-vec foundation (Plan 6A): stores non-secret vector sync metadata
-- alongside chunks inside the SQLCipher-encrypted database. Vector virtual tables land in 6B.
CREATE TABLE IF NOT EXISTS chunk_embeddings_meta (
    chunk_id INTEGER PRIMARY KEY REFERENCES chunks(id) ON DELETE CASCADE,
    file_id TEXT NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    model_id TEXT NOT NULL,
    dim INTEGER NOT NULL,
    content_hash BLOB NOT NULL,
    embedded_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_chunk_emb_file ON chunk_embeddings_meta(file_id);
CREATE INDEX IF NOT EXISTS idx_chunk_emb_model ON chunk_embeddings_meta(model_id);
