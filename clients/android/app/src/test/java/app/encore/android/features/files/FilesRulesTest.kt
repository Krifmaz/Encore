package app.encore.android.features.files

import app.encore.android.core.EncoreJson
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The file manager's rules and the reading of the server's answers. The JSON below is what the 0.33 server writes
 * (server/crates/files: list.rs, roots.rs, trash.rs, upload.rs, import.rs, ops.rs); the rules and words are the
 * web's (web-rs/src/pages/files/logic.rs tests).
 */
class FilesRulesTest {
    private fun json(s: String): JsonElement = EncoreJson.parseToJsonElement(s)
    private fun obj(s: String): JsonObject = json(s) as JsonObject

    @Test
    fun pathsJoinSplitAndReadAsPlaces() {
        assertEquals("A", FilesRules.join("", "A"))
        assertEquals("A/B", FilesRules.join("A", "B"))
        assertEquals("A/B", FilesRules.parentOf("A/B/C"))
        assertEquals("", FilesRules.parentOf("A"))
        assertEquals("Library › A › B", FilesRules.place("Library", "A/B"))
        assertEquals("Library", FilesRules.place("Library", ""))
        assertEquals("From Library › Artist", FilesRules.trashFrom("Library", "Artist/Album"))
        assertEquals("From Staging", FilesRules.trashFrom("Staging", "Album"))
    }

    /** The folder's filter, a search answer (server/crates/files/src/search.rs) and where a result goes. */
    @Test
    fun searchFiltersReadsResultsAndJumps() {
        assertTrue(FilesRules.matches("Blur – Parklife", "  PARK  blur "))
        assertFalse(FilesRules.matches("Blur – Parklife", "blur nope"))
        assertTrue("a blank search lets everything through", FilesRules.matches("anything", "  "))
        val hits = FileHits.from(json("""{"root": "library", "path": "", "q": "teardrop", "truncated": false, "scanned": 12, "results": [
            {"name": "Mezzanine", "kind": "dir", "path": "Massive Attack/Mezzanine", "parent": "Massive Attack", "size": 0, "mtime": 1, "image": false},
            {"name": "03 Teardrop.flac", "kind": "file", "path": "Massive Attack/Mezzanine/03 Teardrop.flac", "parent": "Massive Attack/Mezzanine", "size": 9, "mtime": 1, "image": false, "audio": {"format": "flac"}},
            {"name": "to elsewhere", "kind": "link", "target_kind": "dir", "path": "to elsewhere", "parent": ""},
            {"name": "", "path": "nameless"}]}"""))
        assertEquals(3, hits.hits.size)
        assertFalse(hits.truncated)
        assertEquals(Triple("Massive Attack/Mezzanine", false, null), FilesRules.hitTarget(hits.hits[0]))
        assertEquals(Triple("Massive Attack/Mezzanine", false, "03 Teardrop.flac"), FilesRules.hitTarget(hits.hits[1]))
        assertTrue(hits.hits[1].entry.isAudio)
        assertEquals(Triple("to elsewhere", true, null), FilesRules.hitTarget(hits.hits[2]))
        assertEquals("1 found in Library", FilesRules.foundLine(1, false, "Library"))
        assertEquals("3 found in Library", FilesRules.foundLine(3, false, "Library"))
        assertEquals("200+ found in Library (the nearest first; type more to narrow it)", FilesRules.foundLine(200, true, "Library"))
        assertEquals("Nothing in Staging is called that", FilesRules.foundLine(0, false, "Staging"))
    }

    @Test
    fun theFirstLinkOnTheWayIsKept() {
        // opening a link to a folder: it is the link from now on
        assertEquals("Music", FilesRules.linkAfter(null, "Music", true))
        // below it (another link too) it stays the first one
        assertEquals("Music", FilesRules.linkAfter("Music", "Music/A", false))
        assertEquals("Music", FilesRules.linkAfter("Music", "Music/A/Other", true))
        assertEquals("Music", FilesRules.linkAfter("Music", "Music", false))
        // up and out of it, or into a folder that only starts like it: none
        assertNull(FilesRules.linkAfter("Music", "", false))
        assertNull(FilesRules.linkAfter("Music", "Music2", false))
        assertEquals("Music2", FilesRules.linkAfter("Music", "Music2", true))
        assertNull(FilesRules.linkAfter(null, "A/B", false))
    }

    @Test
    fun namesFollowTheServersRule() {
        assertEquals("A name can't be empty", FilesRules.nameProblem(""))
        assertEquals("A name can't be empty", FilesRules.nameProblem("  "))
        assertEquals("A name can't contain / or \\", FilesRules.nameProblem("a/b"))
        assertEquals("A name can't contain / or \\", FilesRules.nameProblem("a\\b"))
        assertEquals("That name isn't allowed", FilesRules.nameProblem("."))
        assertEquals("That name isn't allowed", FilesRules.nameProblem(".."))
        assertEquals("That name is too long", FilesRules.nameProblem("x".repeat(256)))
        // 255 bytes are fine, 256 bytes of two-byte letters are not
        assertNull(FilesRules.nameProblem("x".repeat(255)))
        assertEquals("That name is too long", FilesRules.nameProblem("é".repeat(128)))
        for (bad in listOf("song‮calf.exe", "tab\there", "a\u0000b", "a\u007fb", "a\u0085b", "a b", "a⁦b")) {
            assertEquals(bad, "A name can't contain invisible or control characters", FilesRules.nameProblem(bad))
        }
        for (good in listOf("01 Crazy.flac", "Björk – Début", ".hidden", "Album (2006)", "emoji 👨‍👩‍👧", "AC⧸DC - T.N.T.opus")) {
            assertNull(good, FilesRules.nameProblem(good))
        }
    }

    @Test
    fun uploadedNamesAreWhatTheServerTakes() {
        assertEquals("01 Song.flac", FilesRules.uploadPart("01 Song.flac"))
        assertEquals("a_b_c.flac", FilesRules.uploadPart("a/b\\c.flac"))
        // what the server would refuse goes: control characters (C0, DEL, C1), the separators, the bidi controls
        assertEquals("songcalf.exe", FilesRules.uploadPart("song‮calf.exe"))
        assertEquals("ab.flac", FilesRules.uploadPart("a\u0007\u007f\u0085 b.flac"))
        assertEquals("Song.flac", FilesRules.uploadPart("  Song.flac\n"))
        assertEquals("Ünïcödé 歌.flac", FilesApi.safeName("Ünïcödé 歌.flac"))
        for (n in listOf("x.flac", "song‮calf.exe", "a\u0007b", "Dir\\x")) assertNull(n, FilesRules.nameProblem(FilesApi.safeName(n)))
    }

    @Test
    fun sortingReadsAndTurnsRound() {
        assertEquals("Sort: Name ↑", FilesRules.sortButton("name", "asc"))
        assertEquals("Sort: Date modified ↓", FilesRules.sortButton("mtime", "desc"))
        assertEquals("smallest first", FilesRules.sortOrderText("size", "asc"))
        assertEquals("newest first", FilesRules.sortOrderText("mtime", "desc"))
        assertEquals("Z–A", FilesRules.sortOrderText("name", "desc"))
        assertEquals("name" to "desc", FilesRules.nextSort("name", "asc", "name"))
        assertEquals("size" to "desc", FilesRules.nextSort("name", "asc", "size"))
        assertEquals("name" to "asc", FilesRules.nextSort("size", "desc", "name"))
        assertEquals(listOf("name", "size", "mtime"), FilesRules.SORTS.map { it.first })
    }

    @Test
    fun resultsReadAsOneLine() {
        assertEquals("", FilesRules.failureLine(json("""{"moved": 2, "failed": []}""")))
        assertEquals("", FilesRules.failureLine(null))
        // move / trash: {path, reason}
        assertEquals(
            "01.flac: A file with that name is already there",
            FilesRules.failureLine(json("""{"moved": 0, "failed": [{"path": "A/01.flac", "reason": "A file with that name is already there"}]}""")),
        )
        // restore: {id, reason}
        assertEquals(
            "2 weren't done. 1792300000-1: Its folder is no longer one Encore may use",
            FilesRules.failureLine(json("""{"restored": 0, "failed": [{"id": "1792300000-1", "reason": "Its folder is no longer one Encore may use"}, {"id": "x", "reason": "y"}]}""")),
        )
        // import apply: {folder, reason}
        assertEquals("CD2: No songs in this folder", FilesRules.failureLine(json("""{"failed": [{"folder": "Album/CD2", "reason": "No songs in this folder"}]}""")))
        assertEquals("Moved too far", FilesRules.failureLine(json("""{"failed": [{"reason": "Moved too far"}]}""")))
    }

    @Test
    fun anUploadAnswerSaysWhatHappenedToTheFile() {
        assertEquals(
            UploadOutcome("uploaded", "Album/01.flac", ""),
            FilesRules.uploadOutcome(obj("""{"uploaded": [{"name": "Album/01.flac", "path": "Album/01.flac", "size": 10}], "renamed": [], "skipped": []}"""), "Album/01.flac"),
        )
        assertEquals(
            UploadOutcome("renamed", "Album/01 (2).flac", ""),
            FilesRules.uploadOutcome(obj("""{"uploaded": [{"name": "Album/01.flac", "path": "Album/01 (2).flac", "size": 10}], "renamed": [{"from": "Album/01.flac", "to": "Album/01 (2).flac"}], "skipped": []}"""), "Album/01.flac"),
        )
        assertEquals(
            UploadOutcome("skipped", "02.flac", "A folder with that name is already there"),
            FilesRules.uploadOutcome(obj("""{"uploaded": [], "renamed": [], "skipped": [{"name": "02.flac", "reason": "A folder with that name is already there"}]}"""), "02.flac"),
        )
        assertEquals(UploadOutcome("skipped", "x", "Skipped"), FilesRules.uploadOutcome(obj("""{"skipped": [{"name": "x"}]}"""), "x"))
        assertEquals(UploadOutcome("uploaded", "x", ""), FilesRules.uploadOutcome(null, "x"))
    }

    @Test
    fun theEndOfAnUploadBatch() {
        assertEquals("Uploaded 1 file" to "ok", FilesRules.uploadEnd(1, 1, false, ""))
        assertEquals("Uploaded 12 files" to "ok", FilesRules.uploadEnd(12, 12, false, ""))
        assertEquals("Upload stopped after 3 of 12" to "info", FilesRules.uploadEnd(3, 12, true, ""))
        assertEquals(
            "Uploaded 11 of 12. 02.flac: A folder with that name is already there" to "warn",
            FilesRules.uploadEnd(11, 12, false, "02.flac: A folder with that name is already there"),
        )
        assertEquals("Import 1 album", FilesRules.importButton(1))
        assertEquals("Import 4 albums", FilesRules.importButton(4))
    }

    @Test
    fun theRootsAsTheServerListsThem() {
        val roots = (json(
            """[{"id": "library", "name": "Library", "path": "/music", "writable": true, "free_bytes": 1000000000},
                {"id": "torrent-1", "name": "Torrents (qBittorrent)", "path": "/downloads/torrents", "writable": false, "free_bytes": 0},
                {"id": "trash", "name": "Trash", "path": "/config/trash", "writable": false, "free_bytes": 5}]""",
        ) as JsonArray).map { FileRoot.from(it as JsonObject)!! }
        assertEquals(listOf("library", "torrent-1", "trash"), roots.map { it.id })
        assertTrue(roots[0].writable && !roots[0].isTrash && roots[0].freeBytes == 1000000000L)
        assertFalse(roots[1].writable)
        // Trash is never written into but by Move to trash: the server says not writable
        assertTrue(roots[2].isTrash && !roots[2].writable)
        assertNull(FileRoot.from(obj("""{"name": "No id"}""")))
        assertTrue("an older shape without writable", FileRoot.from(obj("""{"id": "staging"}"""))!!.writable)
    }

    @Test
    fun aFolderAsTheServerListsIt() {
        val page = FileListing.from(json(
            """{"root": "library", "path": "Artist", "total": 812, "offset": 0, "via_link": true, "entries": [
                 {"name": "Album", "kind": "dir", "size": 0, "mtime": 1792300000, "image": false},
                 {"name": "Elsewhere", "kind": "link", "size": 0, "mtime": 1792300001, "image": false, "target": "/srv/other", "target_kind": "dir"},
                 {"name": "gone.flac", "kind": "link", "size": 0, "mtime": 1792300002, "image": false, "target": "../x.flac", "target_kind": "missing"},
                 {"name": "01 Crazy.flac", "kind": "file", "size": 31457280, "mtime": 1792300003, "image": false,
                  "audio": {"format": "flac", "duration": 178.4, "title": "Crazy", "artist": "Gnarls Barkley", "album": "St. Elsewhere", "track_id": 812}},
                 {"name": "02.flac", "kind": "file", "size": 100, "mtime": 1792300004, "image": false, "audio": {"format": "flac"}},
                 {"name": "cover.jpg", "kind": "file", "size": 5, "mtime": 1792300005, "image": true},
                 {"kind": "file"}]}""",
        ), 0)
        assertEquals(812, page.total)
        assertTrue(page.viaLink)
        assertEquals(listOf("Album", "Elsewhere", "gone.flac", "01 Crazy.flac", "02.flac", "cover.jpg"), page.entries.map { it.name })
        val (dir, link, gone, song, loose, cover) = page.entries
        assertTrue(dir.isDir && !dir.isAudio)
        assertTrue(link.isDir && link.kind == "link" && link.target == "/srv/other")
        assertFalse(gone.isDir)
        assertEquals(812L, song.audio?.trackId)
        assertEquals("Gnarls Barkley", song.audio?.artist)
        assertEquals(178.4, song.audio?.durationSec ?: 0.0, 1e-9)
        // an audio file the library doesn't have: audio has only its format, no track id (no Play)
        assertTrue(loose.isAudio)
        assertEquals(0L, loose.audio?.trackId)
        assertTrue(cover.image && !cover.isAudio)
        // a folder reached plainly; the total is never less than what is there
        val plain = FileListing.from(json("""{"total": 1, "offset": 500, "entries": [{"name": "a", "kind": "file"}, {"name": "b", "kind": "file"}]}"""), 500)
        assertFalse(plain.viaLink)
        assertEquals(502, plain.total)
    }

    @Test
    fun theTrashAsTheServerListsIt() {
        val items = (json(
            """[{"id": "1792300100-1", "name": "Album", "root": "library", "path": "Artist/Album", "at": 1792300100, "by": "alex", "size": 4096, "kind": "dir"},
                {"id": "1792300000-2", "name": "", "root": "staging", "path": "x/01.flac", "at": 1792300000, "by": "sam", "size": 0, "kind": "file"}]""",
        ) as JsonArray).mapNotNull { TrashItem.from(it as JsonObject) }
        assertEquals(listOf("1792300100-1", "1792300000-2"), items.map { it.id })
        assertEquals("dir", items[0].kind)
        assertEquals("alex", items[0].by)
        assertEquals(1792300100.0, items[0].at, 0.0)
        assertEquals("01.flac", items[1].name)
    }

    @Test
    fun anImportAsTheServerAnswersIt() {
        val preview = (obj(
            """{"albums": [
                 {"folder": "Gnarls Barkley/St. Elsewhere", "artist": "Gnarls Barkley", "album": "St. Elsewhere", "files": 14,
                  "release": {"id": "mbid-1", "title": "St. Elsewhere", "artist": "Gnarls Barkley", "date": "2006-05-08", "provider": "MusicBrainz", "track_count": 14, "score": 92, "match": 0.92},
                  "confidence": 0.92,
                  "rows": [{"path": "Gnarls Barkley/St. Elsewhere/02 crazy.flac", "old_title": "crazy", "new_title": "Crazy", "new_track": 2, "new_disc": 1, "new_path": "Gnarls Barkley/St. Elsewhere (2006)/02 Crazy.flac"}]},
                 {"folder": "Loose", "artist": "", "album": "Loose", "files": 2, "release": null, "confidence": 0.0,
                  "rows": [{"path": "Loose/a.flac", "old_title": "a", "new_title": "a", "new_track": null, "new_disc": null, "new_path": "Unknown Artist/Loose/a.flac"}]}]}""",
        )["albums"] as JsonArray).mapNotNull { ImportAlbum.from(it as JsonObject) }
        val (matched, loose) = preview
        assertEquals("mbid-1", matched.release?.id)
        assertEquals(0.92, matched.confidence, 1e-9)
        assertEquals(0.92, matched.release?.score ?: 0.0, 1e-9)
        assertEquals("Gnarls Barkley – St. Elsewhere (2006) · MusicBrainz", matched.release?.line)
        assertEquals(2, matched.rows[0].newTrack)
        assertEquals(14, matched.files)
        assertNull(loose.release)
        assertEquals(0, loose.rows[0].newTrack)
        // without its own confidence the album has its release's (an older shape: score 0…100 only)
        val older = ImportAlbum.from(obj("""{"folder": "A", "release": {"id": "r", "score": 80}}"""))!!
        assertEquals(0.8, older.confidence, 1e-9)
        assertEquals("A", older.album)

        // Other matches: candidates with the search's score and the match (0…1) the list is sorted by
        val cands = (obj(
            """{"provider": "deezer", "candidates": [
                 {"id": "deezer:302127", "title": "Discovery", "artist": "Daft Punk", "date": "2001-03-07", "country": "", "track_count": 14, "formats": "Digital", "type": "Album", "label": "", "score": 90, "cover": null, "provider": "Deezer", "match": 0.95},
                 {"id": "", "title": "No id"}]}""",
        )["candidates"] as JsonArray).mapNotNull { ImportRelease.from(it as JsonObject) }
        assertEquals(1, cands.size)
        assertEquals(0.95, cands[0].score, 1e-9)
        assertEquals(14, cands[0].trackCount)
        assertEquals("Daft Punk – Discovery (2001) · Deezer", cands[0].line)
        assertEquals(listOf("musicbrainz", "deezer", "apple", "discogs", "fingerprint"), FilesApi.PROVIDERS.map { it.first })

        // apply
        assertEquals(ImportResult("Imported 12 songs", 0), ImportResult.from(obj("""{"imported": 12, "albums": ["a1"], "message": "Imported 12 songs", "failed": []}""")))
        assertEquals(
            ImportResult("Imported 10 songs. 1 weren't imported. Album/CD2: No songs in this folder", 1),
            ImportResult.from(obj("""{"imported": 10, "albums": ["a1"], "message": "Imported 10 songs. 1 weren't imported. Album/CD2: No songs in this folder", "failed": [{"folder": "Album/CD2", "reason": "No songs in this folder"}]}""")),
        )
        assertEquals(ImportResult("Imported 1 song", 0), ImportResult.from(obj("""{"imported": 1}""")))
    }
}

private operator fun <T> List<T>.component6(): T = this[5]
