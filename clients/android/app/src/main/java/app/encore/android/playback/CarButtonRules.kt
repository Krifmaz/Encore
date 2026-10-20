package app.encore.android.playback

/**
 * Settings → Playback → In the car → Car buttons, without Android in it (CarButtonRulesTest): which buttons Android
 * Auto gets and in what order, what a car with a given number of spots shows of them, the stored value, and the
 * texts of the Artist and Signal path buttons.
 *
 * Android Auto gets one ordered list of custom actions and decides by itself how many fit beside Previous, Play and
 * Next. When the list is longer than its spots, its last spot becomes ⋯ and holds the rest. An app can't send a
 * button to ⋯ while a spot is free: Media3 1.11.1 hands the platform session no slot, only the order
 * (CommandButton.getCustomLayoutFromMediaButtonPreferences, MediaSessionLegacyStub.createPlaybackStateCompat). So
 * the three groups work by order: the app sends "On the car screen", then "In the ⋯ menu", exactly in the listener's
 * order, and never the hidden ones. The page uses the car's number of spots only to show what the car will make of
 * that list, and says so when a button lands somewhere else than its group (Like in plain words).
 */
object CarButtonRules {
    /** A button of the car's row: its stored key and its name on the page. */
    enum class Id(val key: String, val label: String) {
        SHUFFLE("shuffle", "Shuffle"),
        REPEAT("repeat", "Repeat"),
        LIKE("like", "Like"),
        LYRICS("lyrics", "Lyrics"),
        ARTIST("artist", "Artist"),
        SIGNAL("signal", "Signal path"),
    }

    /** The page's three groups, in the order the app sends them. */
    enum class Group(val key: String, val title: String) {
        SCREEN("screen", "On the car screen"),
        MENU("menu", "In the ⋯ menu"),
        HIDDEN("hidden", "Hidden"),
    }

    /** Every [Id] exactly once, in one of the groups, in the listener's order. */
    data class Layout(val screen: List<Id>, val menu: List<Id>, val hidden: List<Id>) {
        fun of(g: Group): List<Id> = when (g) {
            Group.SCREEN -> screen
            Group.MENU -> menu
            Group.HIDDEN -> hidden
        }

        fun groupOf(id: Id): Group = when (id) {
            in screen -> Group.SCREEN
            in menu -> Group.MENU
            else -> Group.HIDDEN
        }
    }

    /** Shuffle, Repeat, Like, Lyrics on the screen; Artist and Signal path hidden: a 4-spot car shows all four, no ⋯. */
    val DEFAULT = Layout(listOf(Id.SHUFFLE, Id.REPEAT, Id.LIKE, Id.LYRICS), emptyList(), listOf(Id.ARTIST, Id.SIGNAL))

    /** The owner's car: four spots beside Previous, Play and Next. */
    const val DEFAULT_SPOTS = 4
    val SPOTS = 2..8

    /** How long a note stays on the subtitle line; the Signal path stays longer. */
    const val NOTE_MS = 3_000L
    const val SIGNAL_NOTE_MS = 10_000L

    // ---- the stored value ------------------------------------------------------------------------------------------

    private const val VERSION = "v1"

    /** "v1|screen=shuffle,repeat,like,lyrics|menu=|hidden=artist,signal" */
    fun encode(l: Layout): String =
        VERSION + Group.entries.joinToString("") { g -> "|${g.key}=" + l.of(g).joinToString(",") { it.key } }

    /**
     * What [encode] wrote. Nothing, another version or no group at all: [DEFAULT]. Unknown keys are dropped, a key
     * listed twice stays where it came first, and a button the string doesn't name (one added in a later version)
     * goes to the end of its group in [DEFAULT].
     */
    fun decode(s: String?): Layout {
        val parts = s?.trim()?.split('|') ?: return DEFAULT
        if (parts.first() != VERSION) return DEFAULT
        val groups = HashMap<Group, List<Id>>()
        for (part in parts.drop(1)) {
            if (!part.contains('=')) continue
            val g = Group.entries.firstOrNull { it.key == part.substringBefore('=').trim() } ?: continue
            if (groups.containsKey(g)) continue
            groups[g] = part.substringAfter('=').split(',').mapNotNull { k -> Id.entries.firstOrNull { it.key == k.trim() } }
        }
        if (groups.isEmpty()) return DEFAULT
        return complete(groups[Group.SCREEN].orEmpty(), groups[Group.MENU].orEmpty(), groups[Group.HIDDEN].orEmpty())
    }

    /** Each id once (the first place wins); missing ones to the end of their [DEFAULT] group, in its order. */
    private fun complete(screen: List<Id>, menu: List<Id>, hidden: List<Id>): Layout {
        val seen = HashSet<Id>()
        val s = screen.filter { seen.add(it) }
        val m = menu.filter { seen.add(it) }
        val h = hidden.filter { seen.add(it) }
        return Layout(
            s + DEFAULT.screen.filter { it !in seen },
            m + DEFAULT.menu.filter { it !in seen },
            h + DEFAULT.hidden.filter { it !in seen },
        )
    }

    /** The number of spots, kept to [SPOTS]. */
    fun spots(n: Int): Int = n.coerceIn(SPOTS.first, SPOTS.last)

    // ---- moving buttons --------------------------------------------------------------------------------------------

    /** [id] into group [to] at [index] (past the end: last). */
    fun move(l: Layout, id: Id, to: Group, index: Int = Int.MAX_VALUE): Layout {
        val lists = Group.entries.associateWith { g -> l.of(g).filter { it != id }.toMutableList() }
        val target = lists.getValue(to)
        target.add(index.coerceIn(0, target.size), id)
        return Layout(lists.getValue(Group.SCREEN), lists.getValue(Group.MENU), lists.getValue(Group.HIDDEN))
    }

    /** One place up; the first of a group goes to the end of the group above. */
    fun up(l: Layout, id: Id): Layout {
        val g = l.groupOf(id)
        val i = l.of(g).indexOf(id)
        if (i > 0) return move(l, id, g, i - 1)
        val above = Group.entries.getOrNull(g.ordinal - 1) ?: return l
        return move(l, id, above)
    }

    /** One place down; the last of a group goes to the top of the group below. */
    fun down(l: Layout, id: Id): Layout {
        val g = l.groupOf(id)
        val list = l.of(g)
        val i = list.indexOf(id)
        if (i < list.size - 1) return move(l, id, g, i + 1)
        val below = Group.entries.getOrNull(g.ordinal + 1) ?: return l
        return move(l, id, below, 0)
    }

    /** False for the first button on the screen (nothing above it). */
    fun canUp(l: Layout, id: Id): Boolean = l.screen.firstOrNull() != id

    /** False for the last hidden button (nothing below it). */
    fun canDown(l: Layout, id: Id): Boolean = l.hidden.lastOrNull() != id

    /** A row of the page's list: a group's header or a button. */
    sealed interface Row {
        data class Header(val group: Group) : Row
        data class Button(val id: Id) : Row
    }

    /** The page's list: each group's header, then its buttons. */
    fun flatten(l: Layout): List<Row> = Group.entries.flatMap { g -> listOf(Row.Header(g)) + l.of(g).map { Row.Button(it) } }

    /**
     * A drag of [id] let go over row [flatIndex] of [flatten] (past the end: the last row). Over a button it takes
     * that button's place (the button moves up when [id] came from above it, down when from below); over a header it
     * goes first in that group.
     */
    fun dropAt(l: Layout, id: Id, flatIndex: Int): Layout {
        val rows = flatten(l)
        val from = rows.indexOf(Row.Button(id))
        if (from < 0) return l
        val t = flatIndex.coerceIn(0, rows.size - 1)
        if (t == from) return l
        val rest = rows.toMutableList().apply { removeAt(from) }
        val at = when {
            rows[t] is Row.Header -> if (t > from) t else t + 1
            else -> t
        }.coerceIn(1, rest.size)
        rest.add(at, Row.Button(id))
        var g = Group.SCREEN
        val lists = Group.entries.associateWith { ArrayList<Id>() }
        for (r in rest) when (r) {
            is Row.Header -> g = r.group
            is Row.Button -> lists.getValue(g).add(r.id)
        }
        return Layout(lists.getValue(Group.SCREEN), lists.getValue(Group.MENU), lists.getValue(Group.HIDDEN))
    }

    // ---- what the car gets and shows ---------------------------------------------------------------------------------

    /** What Android Auto gets, in this order: the screen group, then the menu group. Hidden buttons never. */
    fun sent(l: Layout): List<Id> = l.screen + l.menu

    /** What a car shows of the list: [shown] in its spots, [behind] behind ⋯ (empty: no ⋯). */
    data class Preview(val shown: List<Id>, val behind: List<Id>)

    /** With more buttons than [spots], the first spots − 1 show and the last spot becomes ⋯ for the rest. */
    fun preview(sent: List<Id>, spots: Int): Preview {
        val s = spots(spots)
        if (sent.size <= s) return Preview(sent, emptyList())
        return Preview(sent.take(s - 1), sent.drop(s - 1))
    }

    fun preview(l: Layout, spots: Int): Preview = preview(sent(l), spots)

    /** Where a button lands when that is not simply its group, or (for any group) when it is behind ⋯. */
    enum class Tag(val text: String) {
        /** a ⋯ menu button on the screen: a spot was free */
        ON_SCREEN("On the screen in your car – there's a free spot"),
        /** behind ⋯ in the car */
        BEHIND("Behind ⋯ in your car"),
        /** Like behind ⋯: said in plain words */
        LIKE_BEHIND("Behind ⋯ in your car, not on the screen"),
    }

    /** The tag of each button that has one. */
    fun tags(l: Layout, spots: Int): Map<Id, Tag> {
        val p = preview(l, spots)
        val out = LinkedHashMap<Id, Tag>()
        for (id in l.menu) if (id in p.shown) out[id] = Tag.ON_SCREEN
        for (id in p.behind) out[id] = if (id == Id.LIKE) Tag.LIKE_BEHIND else Tag.BEHIND
        return out
    }

    private fun labels(ids: List<Id>) = ids.map { it.label }

    /** "In your car: Shuffle · Repeat · Like · ⋯"; no buttons: Previous, Play and Next alone. */
    fun previewLine(p: Preview): String {
        if (p.shown.isEmpty() && p.behind.isEmpty()) return "No buttons in the car – only Previous, Play and Next"
        return "In your car: " + (labels(p.shown) + if (p.behind.isEmpty()) emptyList() else listOf("⋯")).joinToString(" · ")
    }

    /** "Behind ⋯: Lyrics, Artist, Signal path", "No ⋯ – everything fits"; "" with no buttons at all. */
    fun behindLine(p: Preview): String = when {
        p.behind.isNotEmpty() -> "Behind ⋯: " + labels(p.behind).joinToString(", ")
        p.shown.isEmpty() -> ""
        else -> "No ⋯ – everything fits"
    }

    /** Like behind ⋯, said plainly under the preview; null when Like is on the screen or not sent. */
    fun likeLine(p: Preview): String? =
        if (Id.LIKE in p.behind) "Like is behind ⋯ in your car, not on the screen. Move it higher in the list to keep it on the screen." else null

    /** The Settings row: "Shuffle · Repeat · Like · Lyrics", "Shuffle · Repeat · ⋯ (Like, Lyrics)". */
    fun summary(l: Layout, spots: Int): String {
        val p = preview(l, spots)
        if (p.shown.isEmpty() && p.behind.isEmpty()) return "No buttons – only Previous, Play and Next"
        val line = labels(p.shown).joinToString(" · ")
        if (p.behind.isEmpty()) return line
        return (if (line.isEmpty()) "⋯" else "$line · ⋯") + " (" + labels(p.behind).joinToString(", ") + ")"
    }

    /** The screen group's header: "4 of 4 spots", "3 of 3 spots + ⋯" (the last spot is ⋯). */
    fun capacityLine(l: Layout, spots: Int): String {
        val s = spots(spots)
        val p = preview(l, s)
        if (p.behind.isEmpty()) return "${p.shown.size} of ${CarRules.plural(s, "spot")}"
        return "${p.shown.size} of ${CarRules.plural(s - 1, "spot")} + ⋯"
    }

    /**
     * Where [id] is after a move, said by TalkBack and shown under the list: its group and place in it, then its tag
     * when it has one. "Lyrics: in the ⋯ menu, 1 of 1. Behind ⋯ in your car", "Shuffle: on the car screen, 2 of 4".
     */
    fun movedLine(l: Layout, id: Id, spots: Int): String {
        val g = l.groupOf(id)
        val list = l.of(g)
        val where = "${id.label}: ${g.title.replaceFirstChar { it.lowercaseChar() }}, ${list.indexOf(id) + 1} of ${list.size}"
        val tag = tags(l, spots)[id] ?: return where
        return "$where. ${tag.text}"
    }

    /**
     * A text of the page as TalkBack says it. A screen reader says ⋯ by its symbol name or not at all, so the car's
     * ⋯ is said as "More" ("In the More menu", "Behind the More button in your car", "No More button – everything
     * fits"), and the page's ⋮ by its button's name.
     */
    fun spoken(text: String): String =
        text.replace("No ⋯", "No More button")
            .replace("the ⋯ menu", "the More menu")
            .replace("the ⋮ menu", "the Move to another group button")
            .replace("⋯", "the More button")

    // ---- notes on the subtitle line ------------------------------------------------------------------------------------

    /** The subtitle line now: [note] until [noteUntil] (elapsed realtime ms), then the [normal] line. */
    fun subtitleNow(note: String?, noteUntil: Long, now: Long, normal: String): String =
        if (note != null && now < noteUntil) note else normal

    // ---- Signal path ---------------------------------------------------------------------------------------------------

    /**
     * What the Signal path button shows, from the song's file to what this phone plays (the phone's Signal path sheet
     * in one line). [file]: the file's detail ("FLAC 24/96", CarQuality.ofFile); [transportKind]: what Encore sends
     * ("original" | "flac" | "aac-96" | "cadenza-192000" | "local", "" not known yet); [dataSaver]: an AAC stream
     * because Data saver is on (else adaptive quality); [localLabel]: the saved copy's quality; [phoneRate]: Cadenza on
     * this phone upsampled to this rate (0: it did not); [effects]: the phone's sound effects process the song;
     * [outputRate]: the rate Android gets (0: not known); [normDb]: volume normalization's gain.
     */
    data class Signal(
        val file: String = "",
        val transportKind: String = "",
        val dataSaver: Boolean = false,
        val localLabel: String = "",
        val phoneRate: Int = 0,
        val usbBitPerfect: Boolean = false,
        val usbDevice: String = "",
        val effects: Boolean = false,
        val outputRate: Int = 0,
        val normDb: Double = 0.0,
    )

    const val SIGNAL_UNKNOWN = "Quality not known yet"

    /**
     * "FLAC 24/96 → as is → Android 48 kHz", "FLAC 16/44.1 → AAC 96k (Data saver) → Android 48 kHz",
     * "Saved FLAC 24/96 → bit-perfect to iFi Zen DAC", " · −3.2 dB" after it with normalization. Kept short: a car
     * shows one line.
     */
    fun signalLine(s: Signal): String {
        val kind = s.transportKind
        if (kind.isEmpty()) return SIGNAL_UNKNOWN
        val hops = ArrayList<String>()
        if (kind == "local") {
            hops += "Saved " + s.localLabel.ifBlank { s.file }.ifBlank { "copy" }
        } else {
            if (s.file.isNotEmpty()) hops += s.file
            when {
                kind == "original" -> hops += "as is"
                kind == "flac" -> hops += "converted to FLAC"
                kind.startsWith("aac-") -> {
                    val kbps = kind.removePrefix("aac-").toIntOrNull() ?: 0
                    hops += (if (kbps > 0) "AAC ${kbps}k" else "AAC") + if (s.dataSaver) " (Data saver)" else " (adaptive)"
                }
                kind.startsWith("cadenza-") -> {
                    val rate = kind.removePrefix("cadenza-").toIntOrNull() ?: 0
                    hops += (if (rate > 0) "Cadenza 24/${CarQuality.khz(rate)}" else "Cadenza") + " (server)"
                }
            }
        }
        when {
            s.usbBitPerfect -> hops += "bit-perfect to " + s.usbDevice.ifBlank { "the USB DAC" }
            s.phoneRate > 0 -> hops += "Cadenza ${CarQuality.khz(s.phoneRate)} kHz on this phone"
            else -> {
                if (s.effects) hops += "effects"
                hops += if (s.outputRate > 0) "Android ${CarQuality.khz(s.outputRate)} kHz" else "Android"
            }
        }
        val line = hops.joinToString(" → ")
        return if (Math.round(s.normDb * 10.0) != 0L) "$line · ${Normalization.dbText(s.normDb)}" else line
    }

    // ---- Artist: "More by <artist>" after this song ------------------------------------------------------------------

    /** The Artist button's first note, while the songs load. */
    fun addingMessage(name: String): String = "Adding songs by $name…"

    /** A second press for the same song and artist. */
    fun alreadyAddedMessage(name: String, group: Boolean): String =
        if (group) "Already added songs by $name to the group's Up next" else "Already added songs by $name after this song"

    const val NO_ARTIST = "No artist for this song"

    /**
     * The songs to add, in the order given: not the one playing, none already coming up next, each once, at most
     * [max] (CarRules.MORE_BY_SONGS).
     */
    fun moreByPick(candidates: List<Long>, playing: Long?, upcoming: Set<Long>, max: Int): List<Long> =
        candidates.asSequence().distinct().filter { it != playing && it !in upcoming }.take(max.coerceAtLeast(0)).toList()

    /** The songs of artist [key] were already added while [song] plays (the last press that added any). */
    fun alreadyAdded(lastKey: String?, lastSong: String?, key: String, song: String?): Boolean =
        song != null && lastKey == key && lastSong == song
}
