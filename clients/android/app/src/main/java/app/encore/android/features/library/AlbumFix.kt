package app.encore.android.features.library

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Matrix
import android.media.ExifInterface
import android.net.Uri
import android.util.Base64
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.arr
import app.encore.android.core.long
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.ApiException
import app.encore.android.data.repo.AlbumMatch
import app.encore.android.data.repo.ArtCandidate
import app.encore.android.data.repo.MatchPreview
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.screens.Spinner
import app.encore.android.ui.screens.features
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream

// The album page's correcting tools for admins (web-rs/src/pages/library.rs `match_modal`, `edit_modal`, and
// artwork.rs): Tag / match metadata, Edit details (album, artist, songs), Change artwork…. Each is a sheet; after a
// change the page loads again (SongFixUi.changed), or moves to the album's new page when its id changed.


/** The album moved (its id changed): leave the old page for the new one; else the page loads again. */
private fun afterAlbumChange(nav: Nav, oldId: String, newId: String) {
    if (newId.isNotEmpty() && newId != oldId) {
        nav.back()
        nav.open(Screen.Album(newId))
    }
    SongFixUi.changed()
}

/** The providers of "Tag this album": (id, label). */
private val PROVIDERS = listOf(
    "musicbrainz" to "MusicBrainz",
    "deezer" to "Deezer",
    "apple" to "Apple Music",
    "discogs" to "Discogs",
    "fingerprint" to "By audio (AcoustID)",
)

private fun providerHelp(id: String): String = when (id) {
    "musicbrainz" -> "Full MusicBrainz tags (ids that Picard, Navidrome and Jellyfin use)."
    "deezer" -> "Titles, track numbers, ISRCs, label, barcode and cover from Deezer – for releases MusicBrainz lacks or gets wrong."
    "apple" -> "Titles, track numbers, genre and a large cover from Apple Music."
    "discogs" -> "Vinyl, cassettes, bootlegs and old releases (needs a Discogs token in Settings)."
    "fingerprint" -> "Ignores names: identifies each song by its audio, then finds the release most of them are on (needs an AcoustID key)."
    else -> ""
}


/** "Tag this album": search a catalogue for the release, see what each file becomes, and write the tags. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun AlbumMatchSheet(nav: Nav, albumId: String, artist: String, album: String, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var provider by remember { mutableIntStateOf(0) }
    var qArtist by remember { mutableStateOf(if (artist == "Unknown Artist") "" else artist) }
    var qAlbum by remember { mutableStateOf(album) }
    var link by remember { mutableStateOf("") }
    var loading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var found by remember { mutableStateOf<AlbumMatch?>(null) }
    var preview by remember { mutableStateOf<Pair<String, MatchPreview>?>(null) }
    var rename by remember { mutableStateOf(true) }
    var applying by remember { mutableStateOf(false) }

    fun search() {
        loading = true; error = null; preview = null
        val prov = PROVIDERS[provider].first
        scope.launch {
            try {
                found = withContext(Dispatchers.IO) { nav.features.albumMatch(albumId, prov, qArtist.trim(), qAlbum.trim()) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message ?: "That didn't work"
            }
            loading = false
        }
    }
    fun showPreview(rid: String) {
        loading = true; error = null
        scope.launch {
            try {
                preview = rid to withContext(Dispatchers.IO) { nav.features.albumMatchPreview(albumId, rid) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message ?: "That didn't work"
            }
            loading = false
        }
    }
    LaunchedEffect(provider) { search() }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
            Text("Tag this album", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 22.dp, vertical = 8.dp))
            ChoiceRow(PROVIDERS.map { it.second }, provider, onSelect = { provider = it })
            Column(Modifier.padding(horizontal = 22.dp)) {
                Text(providerHelp(PROVIDERS[provider].first), style = MaterialTheme.typography.bodySmall, color = p.muted)
                EncoreField(qArtist, { qArtist = it }, placeholder = "Artist", modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
                EncoreField(qAlbum, { qAlbum = it }, placeholder = "Album", modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
                EncoreField(link, { link = it }, placeholder = "…or paste a MusicBrainz / Deezer / Apple / Discogs link", modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
                Row(Modifier.padding(top = 8.dp)) {
                    EncoreButton("Search", iconRes = R.drawable.ic_search, enabled = !loading) {
                        val rid = LibraryFixRules.releaseIdOf(link.trim())
                        if (rid != null) showPreview(rid) else search()
                    }
                }
                val err = error
                val pv = preview
                val f = found
                when {
                    loading -> Row(Modifier.padding(vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                        Spinner()
                        if (PROVIDERS[provider].first == "fingerprint" && pv == null) {
                            Text(" Listening to every song… this takes a few seconds per song", style = MaterialTheme.typography.bodySmall, color = p.muted)
                        }
                    }
                    err != null -> Text(err, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err, modifier = Modifier.padding(vertical = 12.dp))
                    pv != null -> {
                        val (rid, prev) = pv
                        val r = prev.release
                        val providerName = r?.provider?.ifEmpty { null } ?: "MusicBrainz"
                        Row(Modifier.padding(top = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                            Cover(r?.cover?.ifEmpty { null }, nav.graph.imageLoader, size = 56.dp, corner = 8.dp, label = r?.title)
                            Spacer(Modifier.width(12.dp))
                            Column(Modifier.weight(1f)) {
                                Text(r?.title.orEmpty(), style = MaterialTheme.typography.titleMedium)
                                val info = listOfNotNull(r?.artist, r?.date, r?.country, r?.label).filter { it.isNotEmpty() }.joinToString(" · ")
                                if (info.isNotEmpty()) Text(info, style = MaterialTheme.typography.bodySmall, color = p.muted)
                            }
                            Badge("Confidence ${LibraryFixRules.pct(prev.confidence)}%", kind = LibraryFixRules.matchKind(prev.confidence))
                        }
                        // the web's table head: Current | → provider
                        Row(Modifier.padding(top = 12.dp, bottom = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text("Current", style = MaterialTheme.typography.labelMedium, color = p.muted)
                            Text("→ $providerName", style = MaterialTheme.typography.labelMedium, color = p.muted)
                        }
                        for (row in prev.rows) {
                            Column(Modifier.fillMaxWidth().padding(vertical = 6.dp)) {
                                Text("${row.oldTrack.ifEmpty { "–" }}. ${row.oldTitle}", style = MaterialTheme.typography.bodySmall)
                                if (row.oldArtist.isNotEmpty()) Text(row.oldArtist, style = MaterialTheme.typography.bodySmall, color = p.faint)
                                if (row.newTitle.isNotEmpty()) {
                                    val disc = if (row.newDisc > 1) "${row.newDisc}-" else ""
                                    Text(
                                        "→ $disc${row.newTrack}. ${row.newTitle}",
                                        style = MaterialTheme.typography.bodySmall,
                                        color = if (row.newTitle != row.oldTitle) p.accent else p.text,
                                    )
                                    if (row.newArtist.isNotEmpty()) Text(row.newArtist, style = MaterialTheme.typography.bodySmall, color = p.faint)
                                    if (row.newPath.isNotEmpty()) Text(row.newPath, style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                } else {
                                    Badge("no match – left untouched", kind = "err", modifier = Modifier.padding(top = 2.dp))
                                }
                            }
                        }
                        Row(Modifier.padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                            Switch(checked = rename, onCheckedChange = { rename = it })
                            Spacer(Modifier.width(8.dp))
                            Text("Rename & move files using naming template", style = MaterialTheme.typography.bodyMedium)
                        }
                        Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                            EncoreButton("Back") { preview = null; if (found == null) search() }
                            EncoreButton(if (applying) "Writing tags…" else "Apply tags", iconRes = R.drawable.ic_check, primary = true, enabled = !applying) {
                                applying = true
                                scope.launch {
                                    try {
                                        val newId = withContext(Dispatchers.IO) { nav.features.retagAlbum(albumId, rid, rename) }
                                        Toasts.show("Album tagged from $providerName", "ok")
                                        onDismiss()
                                        afterAlbumChange(nav, albumId, newId)
                                    } catch (e: CancellationException) {
                                        throw e
                                    } catch (e: Exception) {
                                        Toasts.show(e.message ?: "That didn't work", "err")
                                        applying = false
                                    }
                                }
                            }
                        }
                    }
                    f != null && f.candidates.isEmpty() -> EmptyState(R.drawable.ic_search, "No releases found", "Try editing the artist/album, or paste a release ID.")
                    f != null -> {
                        Text("Your files: ${f.trackCount} tracks. Pick the matching release:", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(top = 12.dp, bottom = 4.dp))
                        for (c in f.candidates) {
                            Row(Modifier.fillMaxWidth().clickable { showPreview(c.id) }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                                Cover(c.cover.ifEmpty { null }, nav.graph.imageLoader, size = 44.dp, corner = 6.dp, label = c.title)
                                Spacer(Modifier.width(10.dp))
                                Column(Modifier.weight(1f)) {
                                    Text("${c.title} · ${c.artist}", style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    if (c.info.isNotEmpty()) Text(c.info, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                }
                                Spacer(Modifier.width(6.dp))
                                Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                    if (c.trackCount > 0) Badge("${c.trackCount} tracks", kind = if (c.trackCount == f.trackCount) "ok" else "warn")
                                    Badge("${LibraryFixRules.pct(c.match)}%", kind = LibraryFixRules.matchKind(c.match))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/** One song row of "Edit details": its id and the fields as typed. */
private class EditRow(val id: Long, val disc: String, val track: String, val title: String, val artist: String)

/**
 * "Edit details": type what the album really is (album artist, album, year, genre) and each song's disc, number,
 * title and artist; the names are written into the files, optionally moving them to match the naming template.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AlbumEditSheet(nav: Nav, albumId: String, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    // album fields: albumartist, album, date, genre
    val head = remember { mutableStateListOf("", "", "", "") }
    val rows = remember { mutableStateListOf<EditRow>() }
    var original by remember { mutableStateOf<List<EditRow>>(emptyList()) }
    var loaded by remember { mutableStateOf(false) }
    var rename by remember { mutableStateOf(true) }
    var saving by remember { mutableStateOf(false) }
    LaunchedEffect(albumId) {
        try {
            val a = withContext(Dispatchers.IO) { nav.features.albumRaw(albumId) }
            head[0] = a.str("albumartist"); head[1] = a.str("album"); head[2] = a.str("year"); head[3] = a.str("genre")
            val list = a.arr("tracks").objectsOf().map { t ->
                EditRow(t.long("id"), t.str("disc").ifEmpty { "1" }.let { if (it == "0") "1" else it }, t.str("track").let { if (it == "0") "" else it }, t.str("title"), t.str("artist"))
            }
            original = list
            rows.clear(); rows.addAll(list)
            loaded = true
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Toasts.show(e.message ?: "That didn't work", "err")
            onDismiss()
        }
    }
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Text("Edit details", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(vertical = 8.dp))
            Text("Type what this album really is. The names are written into the files, and automatic tagging won't change them back.", style = MaterialTheme.typography.bodySmall, color = p.muted)
            if (!loaded) {
                Row(Modifier.padding(vertical = 16.dp)) { Spinner() }
                return@Column
            }
            listOf("Album artist", "Album", "Year / date", "Genre").forEachIndexed { i, label ->
                EncoreField(head[i], { head[i] = it }, label = label, modifier = Modifier.fillMaxWidth().padding(top = 8.dp))
            }
            rows.forEachIndexed { i, r ->
                Column(Modifier.padding(top = 14.dp)) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        EncoreField(r.disc, { rows[i] = EditRow(r.id, it, r.track, r.title, r.artist) }, label = "Disc", modifier = Modifier.width(84.dp))
                        EncoreField(r.track, { rows[i] = EditRow(r.id, r.disc, it, r.title, r.artist) }, label = "#", modifier = Modifier.width(84.dp))
                    }
                    EncoreField(r.title, { rows[i] = EditRow(r.id, r.disc, r.track, it, r.artist) }, label = "Title", modifier = Modifier.fillMaxWidth().padding(top = 4.dp))
                    EncoreField(r.artist, { rows[i] = EditRow(r.id, r.disc, r.track, r.title, it) }, label = "Artist", modifier = Modifier.fillMaxWidth().padding(top = 4.dp))
                }
            }
            Row(Modifier.padding(top = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                Switch(checked = rename, onCheckedChange = { rename = it })
                Spacer(Modifier.width(8.dp))
                Text("Move and rename the files to match (naming template)", style = MaterialTheme.typography.bodyMedium)
            }
            Row(Modifier.fillMaxWidth().padding(top = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End)) {
                EncoreButton("Cancel") { onDismiss() }
                EncoreButton("Save", primary = true, enabled = !saving) {
                    saving = true
                    val ids = rows.map { it.id }
                    val album = mapOf("albumartist" to head[0].trim(), "album" to head[1].trim(), "date" to head[2].trim(), "genre" to head[3].trim())
                    val changed = rows.filter { r ->
                        val o = original.firstOrNull { it.id == r.id } ?: return@filter true
                        o.title != r.title || o.artist != r.artist || o.track != r.track || o.disc != r.disc
                    }
                    scope.launch {
                        try {
                            var newId = withContext(Dispatchers.IO) { nav.features.writeTags(ids, album) }
                            for (r in changed) {
                                val v = mapOf("discnumber" to r.disc.trim(), "tracknumber" to r.track.trim(), "title" to r.title.trim(), "artist" to r.artist.trim())
                                newId = withContext(Dispatchers.IO) { nav.features.writeTags(listOf(r.id), v) }
                            }
                            if (rename) newId = withContext(Dispatchers.IO) { nav.features.writeTags(ids, emptyMap(), rename = true) }
                            Toasts.show("Saved", "ok")
                            onDismiss()
                            afterAlbumChange(nav, albumId, newId)
                        } catch (e: CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            Toasts.show(e.message ?: "That didn't work", "err")
                            saving = false
                        }
                    }
                }
            }
        }
    }
}

/** The longest side of an uploaded picture, in pixels (plenty for a cover; the web's art is 1200–1500 px). */
private const val ART_MAX_PX = 1500

/** A picture the server takes as it is: up to 1 MB (as base64 in JSON, well under its 2 MB request limit). */
private const val ART_KEEP_BYTES = 1024 * 1024

/**
 * A picked picture as the data: URL "Change artwork" sends: as it is when it is small, otherwise scaled to at most
 * [ART_MAX_PX] and saved as JPEG (quality 90), so a 2–6 MB camera photo fits the server's 2 MB request limit. Turned
 * upright from its EXIF orientation. null when it can't be read; "" when it is over the web's 15 MB limit.
 */
private fun artworkDataUrl(context: Context, uri: Uri): String? {
    val cr = context.contentResolver
    val bytes = cr.openInputStream(uri)?.use { it.readBytes() } ?: return null
    if (bytes.size > 15 * 1024 * 1024) return ""
    val mime = cr.getType(uri) ?: "image/jpeg"
    if (bytes.size <= ART_KEEP_BYTES && (mime == "image/jpeg" || mime == "image/png")) {
        return "data:$mime;base64," + Base64.encodeToString(bytes, Base64.NO_WRAP)
    }
    // the size first, to decode at a power-of-two step near the target (a 50 MP photo would not fit in memory)
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    val longest = maxOf(bounds.outWidth, bounds.outHeight)
    if (longest <= 0) return null
    var sample = 1
    while (longest / (sample * 2) >= ART_MAX_PX) sample *= 2
    val decoded = BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample }) ?: return null
    val scale = minOf(1f, ART_MAX_PX.toFloat() / maxOf(decoded.width, decoded.height))
    val matrix = Matrix()
    if (scale < 1f) matrix.postScale(scale, scale)
    val rotation = try {
        when (ExifInterface(ByteArrayInputStream(bytes)).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)) {
            ExifInterface.ORIENTATION_ROTATE_90 -> 90f
            ExifInterface.ORIENTATION_ROTATE_180 -> 180f
            ExifInterface.ORIENTATION_ROTATE_270 -> 270f
            else -> 0f
        }
    } catch (_: Exception) {
        0f
    }
    if (rotation != 0f) matrix.postRotate(rotation)
    val shaped = if (matrix.isIdentity) decoded else Bitmap.createBitmap(decoded, 0, 0, decoded.width, decoded.height, matrix, true)
    val out = ByteArrayOutputStream()
    shaped.compress(Bitmap.CompressFormat.JPEG, 90, out)
    if (shaped !== decoded) shaped.recycle()
    decoded.recycle()
    return "data:image/jpeg;base64," + Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)
}

/** "Change artwork": search every catalogue for the album's picture, or upload one; it is embedded in every song. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalLayoutApi::class)
@Composable
fun AlbumArtworkSheet(nav: Nav, albumId: String, album: String, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    var q by remember { mutableStateOf(album) }
    var loading by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var found by remember { mutableStateOf<List<ArtCandidate>?>(null) }
    var applying by remember { mutableStateOf(false) }

    fun apply(url: String? = null, data: String? = null) {
        if (applying) return
        applying = true
        scope.launch {
            try {
                val n = withContext(Dispatchers.IO) { nav.features.setAlbumArt(albumId, url, data) }
                Toasts.show("Artwork embedded in $n file(s)", "ok")
                onDismiss()
                SongFixUi.changed()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // 413: over the server's request limit (2 MB); the picture is scaled down first, so this is rare
                Toasts.show(if ((e as? ApiException)?.code == 413) "Picture is too large" else e.message ?: "That didn't work", "err")
                applying = false
            }
        }
    }
    fun search() {
        loading = true; error = null
        scope.launch {
            try {
                found = withContext(Dispatchers.IO) { nav.features.artCandidates(albumId, q.trim()) }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = e.message ?: "That didn't work"
            }
            loading = false
        }
    }
    val upload = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        scope.launch {
            try {
                val data = withContext(Dispatchers.IO) { artworkDataUrl(context, uri) }
                when (data) {
                    null -> Toasts.show("That didn't work", "err")
                    "" -> Toasts.show("Picture is larger than 15 MB", "err")
                    else -> apply(data = data)
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Toasts.show(e.message ?: "That didn't work", "err")
            }
        }
    }
    LaunchedEffect(albumId) { search() }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 22.dp).padding(bottom = 24.dp)) {
            Text("Change artwork", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(vertical = 8.dp))
            EncoreField(q, { q = it }, placeholder = "Album title to search for", onAction = { search() }, modifier = Modifier.fillMaxWidth())
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                EncoreButton("Search", primary = true, enabled = !loading) { search() }
                EncoreButton("Upload…", enabled = !applying) { upload.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }
            }
            Text(
                "Cover Art Archive, Deezer, Apple Music, YouTube Music and Discogs. The picture is embedded in every song and saved as cover.jpg.",
                style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(top = 8.dp),
            )
            val err = error
            val list = found
            when {
                loading -> Row(Modifier.padding(vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                    Spinner(); Spacer(Modifier.width(8.dp))
                    Text("Searching every catalogue…", style = MaterialTheme.typography.bodySmall, color = p.muted)
                }
                err != null -> Text(err, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err, modifier = Modifier.padding(vertical = 12.dp))
                list != null && list.isEmpty() -> Text("No artwork found – try another title, or upload a picture.", style = MaterialTheme.typography.bodySmall, color = p.muted, modifier = Modifier.padding(vertical = 12.dp))
                list != null -> FlowRow(Modifier.padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    for (x in list) {
                        Column(Modifier.width(100.dp).clickable(enabled = !applying) { apply(url = x.url) }) {
                            Cover(x.thumb.ifEmpty { x.url }, nav.graph.imageLoader, size = 100.dp, corner = 8.dp, label = x.label)
                            Text(x.source + if (x.size.isNotEmpty()) " · ${x.size}" else "", style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (x.label.isNotEmpty()) Text(x.label, style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
            if (applying) Row(Modifier.padding(top = 10.dp)) { Spinner() }
        }
    }
}
