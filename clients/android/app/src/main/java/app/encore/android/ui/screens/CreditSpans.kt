package app.encore.android.ui.screens

import java.text.Normalizer

/**
 * Where each credited artist sits in a song's artist text, so every one of them becomes its own link.
 *
 * The server keeps a credit (from the files' tags or MusicBrainz) when each of its names appears in the text as
 * words once folded (`artists.rs` `credit_for`): case, accents and punctuation don't count, and neither does
 * the order. A credit "Dr Dre" therefore fits the text "Dr. Dre", and a credit that lists a guest last fits a
 * text that names them second. Matching the names here only exactly and only left to right left such a name
 * unlinked (drawn in the muted colour, unlike its neighbours), so this finds them the way the server does.
 */
object CreditSpans {
    /**
     * The places of [names] in [text]: (index into [names], the characters of [text] it covers), in text order
     * and never overlapping. Each name is looked for as written first, then folded (see [fold]); a name found
     * nowhere (or only where another name already sits) has no entry.
     */
    fun find(text: String, names: List<String>): List<Pair<Int, IntRange>> {
        if (text.isEmpty()) return emptyList()
        val found = ArrayList<Pair<Int, IntRange>>()
        val taken = ArrayList<IntRange>()
        // the folded text and, per folded character, the text character it came from (built once, when needed)
        val folded by lazy { foldWithMap(text) }
        for ((k, name) in names.withIndex()) {
            if (name.isBlank()) continue
            val r = exact(text, name, taken) ?: loose(folded, name, taken) ?: continue
            taken.add(r)
            found.add(k to r)
        }
        return found.sortedBy { it.second.first }
    }

    /**
     * The first place [name] appears in [text] exactly and as whole words (no letter or digit right before or
     * after it: "Dre" is no part of "Dreams"), clear of the [taken] ranges, or null.
     */
    private fun exact(text: String, name: String, taken: List<IntRange>): IntRange? {
        var from = 0
        while (true) {
            val i = text.indexOf(name, from)
            if (i < 0) return null
            val r = i until i + name.length
            val end = i + name.length
            val whole = (i == 0 || !text[i - 1].isLetterOrDigit()) && (end == text.length || !text[end].isLetterOrDigit())
            if (whole && taken.none { it.overlaps(r) }) return r
            from = i + 1
        }
    }

    /**
     * The first place [name] appears in the folded text as whole words (bounded by spaces or the ends), clear of
     * the [taken] ranges, mapped back onto the original text, or null.
     */
    private fun loose(folded: Folded, name: String, taken: List<IntRange>): IntRange? {
        val f = fold(name)
        if (f.isEmpty()) return null
        // padded with a space each side, so " f " only matches whole words, also at the very start or end
        val hay = " ${folded.text} "
        val needle = " $f "
        var from = 0
        while (true) {
            val i = hay.indexOf(needle, from)
            if (i < 0) return null
            // i is the space before the name in `hay`, which is also the name's first character in `folded.text`
            val start = folded.source[i]
            val end = folded.source[i + f.length - 1] + 1
            val r = start until end
            if (taken.none { it.overlaps(r) }) return r
            from = i + 1
        }
    }

    /** Two character ranges share at least one character. */
    private fun IntRange.overlaps(o: IntRange): Boolean = first <= o.last && o.first <= last

    /** A folded text and, for each of its characters, the index of the original character it came from. */
    private class Folded(
        /** the folded text (see [fold]) */
        val text: String,
        /** `source[k]`: the index in the original text of `text[k]` */
        val source: IntArray,
    )

    /**
     * The server's `fold` (`artists.rs`): accents and apostrophes dropped, "&" read as "and", letters and digits
     * lower-cased, everything else a space, and runs of spaces made one (none at the ends).
     */
    fun fold(s: String): String = foldWithMap(s).text

    /** [fold], remembering where each folded character came from. */
    private fun foldWithMap(s: String): Folded {
        val out = StringBuilder(s.length)
        val src = ArrayList<Int>(s.length)
        /** Add one folded character from text index [at]; a space only after a non-space (no runs, none first). */
        fun put(c: Char, at: Int) {
            if (c == ' ' && (out.isEmpty() || out[out.length - 1] == ' ')) return
            out.append(c)
            src.add(at)
        }
        var i = 0
        while (i < s.length) {
            val cp = s.codePointAt(i)
            val n = Character.charCount(cp)
            // compatibility decomposition, as the server's `nfkd()`: "é" → "e" + a combining accent
            for (c in Normalizer.normalize(String(Character.toChars(cp)), Normalizer.Form.NFKD)) {
                when {
                    Character.getType(c) == Character.NON_SPACING_MARK.toInt() ||
                        Character.getType(c) == Character.COMBINING_SPACING_MARK.toInt() ||
                        Character.getType(c) == Character.ENCLOSING_MARK.toInt() -> {}
                    c == '\'' || c == '’' -> {}
                    c == '&' -> { put(' ', i); "and".forEach { put(it, i) }; put(' ', i) }
                    Character.isLetterOrDigit(c) -> c.lowercase().forEach { put(it, i) }
                    else -> put(' ', i)
                }
            }
            i += n
        }
        // no space at the end
        if (out.isNotEmpty() && out[out.length - 1] == ' ') {
            out.setLength(out.length - 1)
            src.removeAt(src.size - 1)
        }
        return Folded(out.toString(), src.toIntArray())
    }
}
