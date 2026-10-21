package app.encore.android.features.library

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.ApiException
import app.encore.android.data.repo.SharedPlaylistCheck
import app.encore.android.data.session.Session
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.MenuEntry
import app.encore.android.ui.components.MoreButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.ConfirmDialog
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.screens.features
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// Someone else's shared playlist in my playlists (PlaylistLinkRules): the Playlists page's "Add a shared playlist",
// the read-only playlist's ⋯ (Save a copy, Remove from my playlists), and an Encore playlist link shared into the app
// ("Add to my playlists?" instead of the requests box).

/** The playlist link shared to the app. */
object PlaylistLinkUi {
    /** An Encore playlist link shared to the app (ShareActivity → MainActivity), looked at once signed in (EncoreRoot). */
    val pendingLink = MutableStateFlow<String?>(null)
}

/** The fallback when an error has no words of its own (the phone's other actions say the same). */
private const val FAILED = "That didn't work"

/** "Add a shared playlist": a pasted link, then the entry opens. */
@Composable
fun AddSharedPlaylistDialog(nav: Nav, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var link by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    fun go() {
        val text = link.trim()
        if (busy || text.isEmpty()) return
        busy = true
        scope.launch {
            try {
                val r = withContext(Dispatchers.IO) { nav.features.addSharedPlaylist(text) }
                Toasts.show(PlaylistLinkRules.addToast(r.added), "ok")
                if (r.id > 0L) nav.open(Screen.Playlist(r.id, r.name))
                onDismiss()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // "That isn't a share link", "This link doesn't work any more", "This is your own playlist" …
                Toasts.show(e.message ?: FAILED, "err")
                busy = false
            }
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(PlaylistLinkRules.ADD_SHARED_TITLE) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(PlaylistLinkRules.ADD_SHARED_HINT, style = MaterialTheme.typography.bodyMedium)
                EncoreField(
                    link,
                    { link = it },
                    placeholder = PlaylistLinkRules.PLACEHOLDER,
                    keyboardType = KeyboardType.Uri,
                    imeAction = ImeAction.Go,
                    onAction = { go() },
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        },
        confirmButton = {
            val enabled = link.isNotBlank() && !busy
            TextButton(onClick = { go() }, enabled = enabled) {
                Text(PlaylistLinkRules.ADD, color = if (enabled) p.text else p.faint, fontWeight = FontWeight.SemiBold)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/**
 * The ⋯ of an added playlist ([PlaylistLinkRules.menuEntries]): Save a copy while the link works, and Remove from my
 * playlists, which asks first. [confirmRemove] is shared with the ended page's Remove button; the confirm lives here, so
 * the removal runs in a scope that stays while the page is open.
 */
@Composable
fun LinkedPlaylistMenu(nav: Nav, id: Long, name: String, active: Boolean, confirmRemove: MutableState<Boolean>) {
    val scope = rememberCoroutineScope()
    MoreButton(
        entries = PlaylistLinkRules.menuEntries(active).map { e ->
            when (e) {
                PlaylistLinkRules.LinkedEntry.SAVE_COPY -> MenuEntry(e.label, R.drawable.ic_content_copy) { saveCopy(nav, scope, id) }
                PlaylistLinkRules.LinkedEntry.REMOVE -> MenuEntry(e.label, R.drawable.ic_delete, danger = true) { confirmRemove.value = true }
            }
        },
    )
    if (confirmRemove.value) {
        ConfirmDialog(
            PlaylistLinkRules.REMOVE,
            PlaylistLinkRules.removeText(name),
            "Remove",
            danger = true,
            onDismiss = { confirmRemove.value = false },
        ) {
            scope.launch {
                try {
                    withContext(Dispatchers.IO) { nav.features.deletePlaylist(id) }
                    Toasts.show(PlaylistLinkRules.REMOVED, "ok")
                    nav.back()
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    Toasts.show(e.message ?: FAILED, "err")
                }
            }
        }
    }
}

/** Save a copy: an editable playlist of the songs as they are now, which then opens. */
private fun saveCopy(nav: Nav, scope: CoroutineScope, id: Long) {
    scope.launch {
        try {
            val (copy, name) = withContext(Dispatchers.IO) { nav.features.copyLinkedPlaylist(id) }
            Toasts.show(PlaylistLinkRules.SAVED, "ok")
            if (copy > 0L) nav.open(Screen.Playlist(copy, name))
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Toasts.show(e.message ?: FAILED, "err")
        }
    }
}

/** A link waiting for its "Add to my playlists?" answer. */
private class Prompt(val link: String, val check: SharedPlaylistCheck)

/**
 * An Encore playlist link shared into the app ([PlaylistLinkUi.pendingLink]), once signed in: the owner's own playlist
 * opens, one already added opens, otherwise "Add to my playlists?". A link of another site that only looks like one
 * (Deezer's `/s/`) goes to the requests box as before. Drawn once at the root.
 */
@Composable
fun PlaylistLinkHost(nav: Nav, session: Session) {
    val pending by PlaylistLinkUi.pendingLink.collectAsState()
    // the host's own scope: clearing the pending link restarts the effect, which must not cancel the check
    val scope = rememberCoroutineScope()
    var prompt by remember { mutableStateOf<Prompt?>(null) }
    LaunchedEffect(pending, session) {
        val link = pending ?: return@LaunchedEffect
        PlaylistLinkUi.pendingLink.value = null
        scope.launch { look(nav, link) { c -> prompt = Prompt(link, c) } }
    }
    prompt?.let { pr -> AddSharedPromptDialog(nav, pr.link, pr.check) { prompt = null } }
}

/** POST /api/playlists/shared/check, then what the answer asks for. */
private suspend fun look(nav: Nav, link: String, ask: (SharedPlaylistCheck) -> Unit) {
    try {
        val c = withContext(Dispatchers.IO) { nav.features.checkSharedPlaylist(link) }
        when {
            // the title, so Liked Songs opens as Liked Songs
            c.own -> if (c.id > 0L) nav.open(Screen.Playlist(c.id, c.title))
            c.added -> {
                Toasts.show(PlaylistLinkRules.ALREADY, "info")
                if (c.id > 0L) nav.open(Screen.Playlist(c.id, c.name))
            }
            else -> ask(c)
        }
    } catch (e: CancellationException) {
        throw e
    } catch (e: ApiException) {
        when (PlaylistLinkRules.onCheckFailed(e.code, link, nav.graph.api.base)) {
            PlaylistLinkRules.CheckFailure.TOAST -> Toasts.show(e.message ?: FAILED, "err")
            PlaylistLinkRules.CheckFailure.REQUEST -> request(nav, link)
        }
    } catch (e: Exception) {
        Toasts.show(e.message ?: FAILED, "err")
    }
}

/** The requests box (POST /api/requests/link), as every other shared link. */
private suspend fun request(nav: Nav, link: String) {
    val first = PlaylistLinkRules.firstLink(link) ?: return
    try {
        val line = withContext(Dispatchers.IO) { nav.graph.api.requestLink(first) }
        Toasts.show(line, "ok")
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        Toasts.show(e.message ?: FAILED, "err")
    }
}

/** "Add to my playlists?": the playlist's name and whose it is. */
@Composable
private fun AddSharedPromptDialog(nav: Nav, link: String, check: SharedPlaylistCheck, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(PlaylistLinkRules.ADD) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(check.name, color = p.text, fontWeight = FontWeight.Bold)
                Text(PlaylistLinkRules.tileLine(check.count, check.owner, true), style = MaterialTheme.typography.bodyMedium)
            }
        },
        confirmButton = {
            TextButton(enabled = !busy, onClick = {
                busy = true
                scope.launch {
                    try {
                        val r = withContext(Dispatchers.IO) { nav.features.addSharedPlaylist(link) }
                        Toasts.show(PlaylistLinkRules.addToast(r.added), "ok")
                        if (r.id > 0L) nav.open(Screen.Playlist(r.id, r.name))
                        onDismiss()
                    } catch (e: CancellationException) {
                        throw e
                    } catch (e: Exception) {
                        Toasts.show(e.message ?: FAILED, "err")
                        busy = false
                    }
                }
            }) { Text(PlaylistLinkRules.ADD, color = p.text, fontWeight = FontWeight.SemiBold) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}
