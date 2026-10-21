package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import app.encore.android.data.api.OneOffs
import app.encore.android.ui.components.MenuEntry
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Library types, Select / Delete… and who added it (0.36): the same words and rules as the web and the desktop. */
class AlbumTypesRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json) as JsonObject

    /** §2.4 of the contract, verbatim (DAY passed in as a string). */
    @Test
    fun whoAddedItInWords() {
        val day = "DAY"
        val table = listOf(
            listOf("request", "alex", false, day, "Requested by alex on DAY"),
            listOf("link", "alex", false, day, "Requested by alex on DAY"),
            listOf("soulseek", "alex", false, day, "Requested by alex on DAY"),
            listOf("request", "alex", false, null, "Requested by alex"),
            listOf("soulseek", "alex", false, null, "Requested by alex"),
            listOf("request", null, false, day, "Requested by someone – record cleared"),
            listOf("link", null, false, null, "Requested by someone – record cleared"),
            listOf("request", null, true, day, "Requested by a deleted user on DAY"),
            listOf("upload", "sam", false, null, "Uploaded by sam"),
            listOf("upload", null, true, null, "Uploaded by a deleted user"),
            listOf("import", "sam", false, null, "Imported by sam"),
            listOf("import", null, true, null, "Imported by a deleted user"),
            listOf("scan", null, false, null, "Found in the music folder"),
            listOf("", null, false, null, ""),
            listOf("something", "x", false, null, ""),
        )
        for (row in table) {
            val (via, by, deleted, at, want) = row
            assertEquals("$row", want, AlbumTypesRules.addedText(via as String, by as String?, deleted as Boolean, at as String?))
        }
        // the en dash with spaces, as the other Encore texts
        assertTrue(AlbumTypesRules.addedText("request", null, false, null).contains(" – "))
        // from the server's answer
        val v = obj("""{"via": "request", "by": "alex", "by_id": 3, "by_deleted": false, "requested_at": 1729000000.0, "added_at": 1729000100.0}""")
        assertEquals("Requested by alex on Oct 13, 2026", AlbumTypesRules.addedOf(v) { "Oct 13, 2026" })
        assertEquals("", AlbumTypesRules.addedOf(obj("""{"via": null}""")) { "" })
        assertEquals("Requested by a deleted user on D", AlbumTypesRules.addedOf(obj("""{"via": "request", "by": null, "by_deleted": true, "requested_at": 5}""")) { "D" })
        assertEquals("Requested by someone – record cleared", AlbumTypesRules.addedOf(obj("""{"via": "link", "by": null, "by_deleted": false, "requested_at": null}""")) { "D" })
    }

    @Test
    fun theHistoryLine() {
        assertEquals("Added DAY", AlbumTypesRules.historyLine(emptyList(), "DAY"))
        assertEquals("Added DAY", AlbumTypesRules.historyLine(listOf(""), "DAY"))
        assertEquals(
            "Requested by alex on Oct 13, 2026 · added Oct 14, 2026",
            AlbumTypesRules.historyLine(listOf("Requested by alex on Oct 13, 2026"), "Oct 14, 2026"),
        )
        assertEquals("A · B · C · and 1 more · added DAY", AlbumTypesRules.historyLine(listOf("A", "B", "C", "D"), "DAY"))
        assertEquals("A · B", AlbumTypesRules.historyLine(listOf("A", "B"), null))
        // the album answer: the server's groups win; an older server's requesters are joined here
        val a = obj(
            """{"history": [{"via": "request", "by": "alex", "requested_at": 1, "songs": 12}, {"via": "upload", "by": "sam", "songs": 1}],
               "added_at": 2, "tracks": []}""",
        )
        assertEquals("Requested by alex on D1.0 · Uploaded by sam · added D2.0", AlbumTypesRules.albumHistory(a) { "D$it" })
        val old = obj("""{"tracks": [{"requested_by": "ann", "requested_at": 3, "added_at": 4}, {"requested_by": "bo", "added_at": 5}]}""")
        assertEquals("Requested by ann, bo on D3.0 · added D4.0", AlbumTypesRules.albumHistory(old) { "D$it" })
        assertEquals("", AlbumTypesRules.albumHistory(obj("""{"tracks": []}""")) { "D$it" })
    }

    @Test
    fun aSongsNoteNamesWhoAsked() {
        val kept = obj("""{"added_via": "request", "added_by_name": "alex", "requested_at": 9, "requested_by": "x"}""")
        assertEquals("alex" to 9.0, AlbumTypesRules.requester(kept))
        assertEquals("requested by alex on D", AlbumTypesRules.requesterNote(kept) { "D" })
        assertEquals("" to 0.0, AlbumTypesRules.requester(obj("""{"added_via": "upload", "added_by_name": "sam", "requested_by": "sam"}""")))
        assertEquals("", AlbumTypesRules.requesterNote(obj("""{"added_via": "scan"}""")) { "D" })
        // an older server: the request's user
        assertEquals("requested by ann", AlbumTypesRules.requesterNote(obj("""{"requested_by": "ann"}""")) { "D" })
    }

    @Test
    fun typeOptionsAndTheirCounts() {
        assertEquals("Live (12)", AlbumTypesRules.typeOptionText("Live", 12))
        val types = AlbumTypesRules.typeCounts(
            obj(
                """{"total": 7, "types": [{"key": "album", "label": "Studio albums", "count": 5}, {"key": "live", "label": "Live", "count": 0},
                   {"key": "one_off", "label": "One-offs", "count": 2}]}""",
            ),
        )
        assertEquals(3, types.size)
        assertEquals(listOf("" to "All types", "album" to "Studio albums (5)", "one_off" to "One-offs (2)"), AlbumTypesRules.typeOptions(types, ""))
        assertTrue("the picked one stays", AlbumTypesRules.typeOptions(types, "live").contains("live" to "Live (0)"))
        assertEquals("Type", AlbumTypesRules.typeChipText(types, ""))
        assertEquals("One-offs", AlbumTypesRules.typeChipText(types, "one_off"))
        assertTrue(AlbumTypesRules.typeCounts(obj("""{"detail": "Not found"}""")).isEmpty())
    }

    @Test
    fun theAlbumsAddress() {
        assertEquals(
            "/api/library/albums?sort=added&limit=120&offset=0&dir=desc",
            AlbumTypesRules.albumsUrl("added", "desc", "", "", "", 0, 120),
        )
        assertEquals(
            "/api/library/albums?sort=added&limit=120&offset=120&dir=desc&q=crazy%20gb&type=live",
            AlbumTypesRules.albumsUrl("added", "desc", "crazy gb", "", "live", 120, 120),
        )
        assertEquals(
            "/api/library/albums?sort=added&dir=desc&artist=Beta%20Band&type=one_off&ids_only=1",
            AlbumTypesRules.idsUrl("added", "desc", "", "Beta Band", "one_off"),
        )
        assertEquals("/api/library/album-types?artist=&filter=", AlbumTypesRules.typesUrl())
        val ids = AlbumTypesRules.AlbumIds.from(obj("""{"total": 6000, "ids": ["a", "b"], "capped": true}"""))
        assertEquals(listOf("a", "b"), ids.ids)
        assertTrue(ids.capped)
        assertEquals(6000, ids.total)
    }

    @Test
    fun theSelection() {
        var picked = AlbumTypesRules.toggle(emptyList(), "a")
        picked = AlbumTypesRules.toggle(picked, "b")
        picked = AlbumTypesRules.toggle(picked, "a")
        assertEquals(listOf("b"), picked)
        assertEquals("1 selected", AlbumTypesRules.selectedText(picked.size))
        val all = (0 until AlbumTypesRules.SELECT_MAX + 7).map { "id$it" }
        val (capped, more) = AlbumTypesRules.selectAll(all)
        assertEquals(AlbumTypesRules.SELECT_MAX, capped.size)
        assertTrue("more than 5000: capped", more)
        assertFalse(AlbumTypesRules.selectAll(all.take(10)).second)
        assertEquals("Select all 340", AlbumTypesRules.selectAllText(340))
        // 340 ids → 4 calls of at most 100, in order
        val chunks = AlbumTypesRules.chunks((0 until 340).map { it.toString() })
        assertEquals(listOf(100, 100, 100, 40), chunks.map { it.size })
        assertEquals("300", chunks[3][0])
    }

    @Test
    fun theTrashTexts() {
        assertEquals("Moving to the Trash… 3 of 12", AlbumTypesRules.movingText(3, 12))
        assertEquals("Moved 1 album to the Trash", AlbumTypesRules.movedText(1, 1, ""))
        assertEquals("Moved 12 albums to the Trash", AlbumTypesRules.movedText(12, 12, ""))
        assertEquals(
            "Moved 10 of 12 albums to the Trash – 2 couldn't be moved: Its files are outside the folders Encore manages",
            AlbumTypesRules.movedText(10, 12, "Its files are outside the folders Encore manages"),
        )
        assertEquals(AlbumTypesRules.TRASH_BODY, AlbumTypesRules.confirmBody(false, ""))
        assertEquals(
            AlbumTypesRules.TRASH_BODY + AlbumTypesRules.OTHER_DISK + "\n1 of them can't be moved to the Trash and stays: X",
            AlbumTypesRules.confirmBody(true, "1 of them can't be moved to the Trash and stays: X"),
        )
        assertTrue(AlbumTypesRules.OTHER_DISK.startsWith(" "))
        // the sums of the real runs: moved albums, the first reason; a call that moved fewer with no reason
        var sums = AlbumTypesRules.Moved()
        sums = sums.plus(obj("""{"albums": 99, "failed": [{"id": "x", "album": "X", "reason": "Its files are outside the folders Encore manages"}]}"""))
        sums = sums.failed("Server error")
        sums = sums.plus(obj("""{"albums": 40, "failed": []}"""))
        assertEquals("Moved 139 of 240 albums to the Trash – 101 couldn't be moved: Its files are outside the folders Encore manages", sums.text(240))
        assertEquals("Moved 2 albums to the Trash", AlbumTypesRules.Moved(2).text(2))
        assertEquals("Moved 1 of 2 albums to the Trash – 1 couldn't be moved: It is no longer in the library", AlbumTypesRules.Moved(1).text(2))
        // the dialog of a dry run: the server's title; no room → its words and Close only
        val d = AlbumTypesRules.trashDialog(obj("""{"text": "Delete 2 albums (1.5 MB)?", "other_disk": true, "skipped_text": "", "room": true, "room_text": ""}"""))
        assertEquals("Delete 2 albums (1.5 MB)?", d.title)
        assertEquals(AlbumTypesRules.TRASH_BODY + AlbumTypesRules.OTHER_DISK, d.text)
        assertTrue(d.canGo)
        val full = AlbumTypesRules.trashDialog(obj("""{"text": "Delete 2 albums (1.5 MB)?", "room": false, "room_text": "There isn't room in the Trash"}"""))
        assertEquals("There isn't room in the Trash", full.text)
        assertFalse(full.canGo)
    }

    @Test
    fun theAlbumMenuForAdminsOnly() {
        assertTrue(AlbumTypesRules.albumAdminItems(false, true).isEmpty())
        val items = AlbumTypesRules.albumAdminItems(true, true)
        assertTrue(items.indexOf("Edit details (album, artist, songs)") < items.indexOf(AlbumTypesRules.CHANGE_TYPE))
        assertTrue(items.indexOf(AlbumTypesRules.CHANGE_TYPE) < items.indexOf("Change artwork…"))
        assertEquals(AlbumTypesRules.MOVE_ALBUM, items.last())
        assertFalse("a Surround album has no type", AlbumTypesRules.albumAdminItems(true, false).contains(AlbumTypesRules.CHANGE_TYPE))
    }

    @Test
    fun changeTypeWords() {
        assertEquals("What is “Now 99”?", AlbumTypesRules.whatIs("Now 99"))
        val t = obj("""{"label": "Live · EP", "chosen": "ep", "why_text": "You chose this", "auto": {"label": "Live · EP", "why_text": "From MusicBrainz"}}""")
        assertEquals(
            listOf("Album", "EP", "Single", "One-off", "Automatic (Live · EP – From MusicBrainz)"),
            AlbumTypesRules.typeChoices(t).map { it.second },
        )
        assertEquals("", AlbumTypesRules.typeChoices(t)[4].first)
        assertEquals("ep", AlbumTypesRules.chosen(t))
        assertEquals("", AlbumTypesRules.chosen(obj("""{"chosen": null}""")))
        assertEquals("Show all 12", AlbumTypesRules.showAllText(12, 10))
        assertEquals("", AlbumTypesRules.showAllText(10, 10))
        assertEquals("Now 99 · Various artists", AlbumTypesRules.oneOffSub("Now 99", "Various artists"))
        assertEquals("Now 99", AlbumTypesRules.oneOffSub("Now 99", ""))
        val shelf = OneOffs.from(obj("""{"total": 3, "offset": 0, "tracks": [{"id": 7, "title": "Song", "album": "Now 99", "release_label": "Compilation"}]}"""))
        assertEquals(3, shelf.total)
        assertEquals(listOf("Compilation"), shelf.labels)
    }

    /** A menu's info line (who added a song) is text, not an action. */
    @Test
    fun anInfoLineIsNotClickable() {
        val info = MenuEntry("Requested by alex on Oct 13, 2026", info = true, onClick = {})
        assertTrue(info.info)
        assertFalse(info.isAction)
        assertTrue(MenuEntry("Play next") {}.isAction)
        assertFalse(MenuEntry("-", onClick = null).isAction)
    }
}
