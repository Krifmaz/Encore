-- Schema 29 (0.33): the translation tiers – built-in romanization and DeepL beside the official names and the
-- local model (encore_library::translate::tiers).

-- when the built-in romanization and DeepL last looked at a row (NULL: not yet), like official_at and model_at:
-- a tier switched on later, or new in this version, takes the rows it has not seen, so a stronger tier replaces
-- the text of a weaker one (manual > official > romanized > deepl > model). translation_source and
-- romanization_source gain the values 'romanized' and 'deepl'.
ALTER TABLE translations ADD COLUMN romanized_at REAL;
ALTER TABLE translations ADD COLUMN deepl_at REAL;

-- characters sent to a paid translation service per calendar month (UTC), for its monthly budget
-- (`deepl_char_budget`): service 'deepl', month '2026-10'
CREATE TABLE IF NOT EXISTS translate_usage (
    service TEXT NOT NULL,
    month TEXT NOT NULL,
    chars INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (service, month)
);
