-- Schema 25: translated and romanized titles.

-- translated and romanized titles (translate/): one row per (text, source language, target language).
-- src_lang is read from the text's script (ja, ko, zh, cyrl, latn …). Each field keeps its source
-- (official beats model); the manual_* columns are corrections made in the apps (NULL: none, '': show
-- nothing) and win over both. official_at / model_at: when each source was last asked.
CREATE TABLE IF NOT EXISTS translations (
    text TEXT NOT NULL,
    src_lang TEXT NOT NULL,
    target TEXT NOT NULL,
    kind TEXT,                              -- title | album | artist
    translation TEXT,
    translation_source TEXT,                -- official | model
    romanization TEXT,
    romanization_source TEXT,
    manual_translation TEXT,
    manual_romanization TEXT,
    official_at REAL,
    model_at REAL,
    updated_at REAL NOT NULL,
    PRIMARY KEY (text, src_lang, target)
);
CREATE INDEX IF NOT EXISTS idx_translations_target ON translations(target, text);
