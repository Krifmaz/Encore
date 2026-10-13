package app.encore.android.ui.components

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.EncoreApi
import app.encore.android.data.api.Playlist
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * The web's `addToPlaylistModal` (web/js/pages/playlists.js) as a bottom sheet: pick a playlist, or type a name
 * and create one. POST /api/playlists/{id}/tracks {track_ids}; POST /api/playlists {name} for a new one.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AddToPlaylistSheet(api: EncoreApi, trackIds: List<Long>, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val loader = rememberLoad("playlists-for-add") { api.playlists().filter { !it.isLiked } }
    var newName by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }

    fun add(playlistId: Long) {
        if (busy) return
        busy = true
        scope.launch {
            try {
                val body = buildJsonObject { put("track_ids", JsonArray(trackIds.map { JsonPrimitive(it) })) }
                val r = withContext(Dispatchers.IO) { api.post("/api/playlists/$playlistId/tracks", body) } as? JsonObject
                val added = r?.get("added")?.let { (it as? JsonPrimitive)?.content?.toDoubleOrNull()?.toInt() } ?: trackIds.size
                val dup = r?.get("duplicates_skipped")?.let { (it as? JsonPrimitive)?.content?.toDoubleOrNull()?.toInt() } ?: 0
                Toasts.show("Added $added" + if (dup > 0) " ($dup already there)" else "", "ok")
                onDismiss()
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't add to the playlist", "err")
                busy = false
            }
        }
    }

    fun create() {
        if (busy) return
        val name = newName.trim().ifEmpty { "New playlist" }
        busy = true
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { api.post("/api/playlists", buildJsonObject { put("name", name) }) } as? JsonObject
                val id = r?.get("id")?.let { (it as? JsonPrimitive)?.content?.toDoubleOrNull()?.toLong() }
                if (id == null) throw IllegalStateException("The server didn't create the playlist")
                busy = false
                add(id)
            } catch (e: Exception) {
                Toasts.show(e.message ?: "Couldn't create the playlist", "err")
                busy = false
            }
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheetState, containerColor = p.panel, contentColor = p.text) {
        Column(Modifier.fillMaxWidth().padding(bottom = 24.dp)) {
            Text("Add to playlist", style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(horizontal = 22.dp, vertical = 8.dp))
            LoadableContent(loader) { playlists ->
                LazyColumn(Modifier.fillMaxWidth().height(if (playlists.size > 5) 300.dp else (playlists.size.coerceAtLeast(1) * 56).dp)) {
                    items(playlists, key = { it.id }) { pl -> PlaylistPick(pl) { add(pl.id) } }
                    if (playlists.isEmpty()) {
                        item { Text("No playlists yet", color = p.muted, modifier = Modifier.padding(horizontal = 22.dp, vertical = 16.dp)) }
                    }
                }
            }
            Row(Modifier.fillMaxWidth().padding(horizontal = 22.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(
                    value = newName,
                    onValueChange = { newName = it },
                    placeholder = { Text("…or a new playlist name", color = p.faint) },
                    singleLine = true,
                    enabled = !busy,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                    colors = OutlinedTextFieldDefaults.colors(
                        focusedBorderColor = p.accent,
                        unfocusedBorderColor = p.border,
                        focusedContainerColor = p.bg2,
                        unfocusedContainerColor = p.bg2,
                    ),
                    modifier = Modifier.weight(1f),
                )
                Spacer(Modifier.width(10.dp))
                EncoreButton("Create", iconRes = R.drawable.ic_add, enabled = !busy, onClick = { create() })
            }
        }
    }
}

@Composable
private fun PlaylistPick(pl: Playlist, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 22.dp, vertical = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(painterResource(R.drawable.ic_queue_music), contentDescription = null, tint = p.muted, modifier = Modifier.width(20.dp))
        Spacer(Modifier.width(12.dp))
        Text(pl.name, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        Text(pl.total.toString(), style = MaterialTheme.typography.bodySmall, color = p.faint)
    }
}
