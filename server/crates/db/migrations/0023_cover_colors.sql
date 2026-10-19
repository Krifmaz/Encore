-- Schema 23: each album cover's main colour, painted while the cover loads.

-- each album cover's main colour ("#rrggbb"), painted while the cover loads; v is the cover's version
-- (covers.rs), so a new cover gets a new colour. Filled by the cover thumbnail pass.
CREATE TABLE IF NOT EXISTS cover_color (
    album_id TEXT PRIMARY KEY,
    v INTEGER NOT NULL,
    color TEXT NOT NULL
);
