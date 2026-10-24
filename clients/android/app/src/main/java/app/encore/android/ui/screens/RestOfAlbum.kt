package app.encore.android.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.selection.toggleable
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CheckboxDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.core.str
import app.encore.android.data.repo.ExtRelease
import app.encore.android.data.repo.ExtTrack
import app.encore.android.data.repo.PickerOpts
import app.encore.android.features.library.RestOfAlbumRules
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.EncoreButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// "Get the rest of the album" for albums MusicBrainz doesn't list (the web's web-rs/src/pages/rest_of_album.rs): another
// catalogue's track list opens in [ExtTracksSheet] (Request on each missing song, and all of them at once); when no
// catalogue has the album, [RestTypedDialog] takes the songs typed in, or asks the download sources for the album by name.

/** What "Get the rest of the album" asks about a library album no catalogue lists: its artist, title and the server's words. */
data class RestAsk(val artist: String, val album: String, val message: String)

/** The release a catalogue's lookup answer [r] opens as in [ExtTracksSheet] (its track list is loaded there by `ext`). */
fun restRelease(r: JsonObject): ExtRelease = ExtRelease(
    title = r.str("title"),
    year = r.str("year"),
    cover = r.str("cover"),
    tracks = RestOfAlbumRules.trackCount(r),
    ext = r["ext"] ?: JsonNull,
)

/**
 * POST /api/discover/album-rest/request with [body], its answer as a toast; the answer, or null when it failed.
 * Runs the call on the IO dispatcher.
 */
suspend fun requestRestSongs(nav: Nav, body: JsonObject): JsonObject? {
    return try {
        val r = withContext(Dispatchers.IO) { nav.features.requestRest(body) }
        Toasts.show(RestOfAlbumRules.requestedText(r), "ok")
        r
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        Toasts.show(e.message ?: "Request failed", "err")
        null
    }
}

/** Request every song of [missing] (songs of the release [m] by [artistName]) in one go, their badges marked as requested. */
suspend fun requestMissingSongs(nav: Nav, m: ExtRelease, artistName: String, missing: List<ExtTrack>, statuses: RequestStatuses) {
    val body = buildJsonObject {
        put("artist", artistName)
        put("album", m.title)
        if (m.year.isNotEmpty()) put("year", m.year)
        if (m.cover.isNotEmpty()) put("cover", m.cover)
        put("tracks", buildJsonArray {
            for (t in missing) add(buildJsonObject {
                put("title", t.title)
                put("artist", t.artist.ifEmpty { artistName })
                if (t.lengthSec > 0) put("length", t.lengthSec)
                if (t.isrc.isNotEmpty()) put("isrc", t.isrc)
            })
        })
    }
    val r = requestRestSongs(nav, body) ?: return
    val status = r.str("status").ifEmpty { "queued" }
    for (t in missing) statuses.mark(listOf(songNameKey(t.artist.ifEmpty { artistName }, t.title)), status)
}

/**
 * No catalogue has the album [ask]: type its songs in (each missing one is requested), ask the download sources for the
 * whole album by name, or – for admins – pick a download by hand.
 */
@Composable
fun RestTypedDialog(nav: Nav, ask: RestAsk, isAdmin: Boolean, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    var artistLines by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text("Get the rest of ${ask.album}") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(RestOfAlbumRules.notFound(ask.album, ask.message), style = MaterialTheme.typography.bodyMedium)
                EncoreField(
                    text, { text = it }, placeholder = "1. First song", label = "The album's songs, one per line",
                    singleLine = false, minLines = 6, maxLines = 12, imeAction = ImeAction.Default, modifier = Modifier.fillMaxWidth(),
                )
                Row(
                    Modifier.fillMaxWidth().toggleable(value = artistLines, role = Role.Checkbox, onValueChange = { artistLines = it }),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Checkbox(
                        checked = artistLines,
                        onCheckedChange = null,
                        colors = CheckboxDefaults.colors(checkedColor = p.accent, checkmarkColor = p.onAccent, uncheckedColor = p.muted),
                    )
                    Spacer(Modifier.width(8.dp))
                    Text("Lines are “Artist - Title”", color = p.text, style = MaterialTheme.typography.bodyMedium)
                }
                // the whole album by name: each download source searches for it, and the songs already here don't stop it
                EncoreButton("Search downloads for the album", iconRes = R.drawable.ic_download, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
                    val body = buildJsonObject {
                        put("kind", "album")
                        put("artist", ask.artist)
                        put("title", ask.album)
                        put("album", ask.album)
                        put("fill_in", true)
                    }
                    scope.launch {
                        busy = true
                        if (submitRequest(nav.features, body) != null) onDismiss()
                        busy = false
                    }
                }
                if (isAdmin) {
                    EncoreButton("Pick a download", iconRes = R.drawable.ic_search, enabled = !busy, modifier = Modifier.fillMaxWidth()) {
                        onDismiss()
                        nav.open(Screen.Picker(PickerOpts(kind = "album", artist = ask.artist, title = ask.album, album = ask.album)))
                    }
                }
            }
        },
        confirmButton = {
            TextButton(enabled = !busy, onClick = {
                if (text.isBlank()) {
                    Toasts.show("Type the album's songs, one per line", "warn")
                    return@TextButton
                }
                val body = buildJsonObject {
                    put("artist", ask.artist)
                    put("album", ask.album)
                    put("text", text)
                    put("artist_lines", artistLines)
                }
                scope.launch {
                    busy = true
                    if (requestRestSongs(nav, body) != null) onDismiss()
                    busy = false
                }
            }) { Text("Request the missing songs", color = p.text) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}
