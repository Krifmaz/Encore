# How Encore recommends music

`how-recommendations-work.pdf` explains the taste engine (smart shuffle, taste scores, likes, skips and
dislikes, "For you" mixes, Start radio, Genre shuffle) for non-developers, with diagrams and worked examples.

The PDF is generated, so it can be rebuilt when the engine changes:

- `gen.py` recomputes every number from the same formulas as `server/crates/library/src/taste.rs` and fills
  `template.html` into `recommendations.html` (run it from this folder: `python3 gen.py`).
- `pdf.js` prints that page to PDF with Playwright's Chromium:
  `node pdf.js "$PWD/recommendations.html" "$PWD/how-recommendations-work.pdf"`.

If a constant in `taste.rs` changes, update the matching value in `gen.py` and the prose in `template.html`.
