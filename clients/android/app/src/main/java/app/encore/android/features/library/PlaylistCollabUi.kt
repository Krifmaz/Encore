package app.encore.android.features.library

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.features.settings.copyToClipboard
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.screens.EncoreField
import app.encore.android.ui.screens.features
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// Collaborative playlists (PlaylistCollabRules): the owner's Invite window (by user name, the invite link, who's in
// it) or a member's Members window, and the invites waiting on the Playlists page.

private const val FAILED = "That didn't work"

/**
 * Invite collaborators (the owner) or Members (a member). [onChanged] runs after each change, so the playlist page
 * shows the new count; a member's Leave closes the page through [onLeft].
 */
@Composable
fun CollabMembersDialog(nav: Nav, id: Long, onChanged: () -> Unit, onLeft: () -> Unit, onDismiss: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    var data by remember { mutableStateOf<PlaylistCollabRules.Members?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var username by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var version by remember { mutableStateOf(0) }
    LaunchedEffect(id, version) {
        try {
            data = withContext(Dispatchers.IO) { PlaylistCollabRules.Members.from(nav.features.collaborators(id)) }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = e.message ?: FAILED
        }
    }
    fun run(call: () -> Unit, ok: String = "") {
        if (busy) return
        busy = true
        scope.launch {
            try {
                withContext(Dispatchers.IO) { call() }
                if (ok.isNotEmpty()) Toasts.show(ok, "ok")
                error = null
                version++
                onChanged()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // "There's no account named “x” on this server", "This is your own playlist" …
                error = e.message ?: FAILED
            }
            busy = false
        }
    }
    fun invite() {
        val name = username.trim()
        if (name.isEmpty()) return
        run({ nav.features.inviteCollaborator(id, name) }, "Invited $name")
        username = ""
    }
    val d = data
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(if (d != null && !d.isOwner) PlaylistCollabRules.MEMBERS_TITLE else PlaylistCollabRules.INVITE_TITLE) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (d == null) {
                    Text(error ?: "Loading…", style = MaterialTheme.typography.bodyMedium)
                    return@Column
                }
                if (d.isOwner) {
                    Text(PlaylistCollabRules.OWNER_HINT, style = MaterialTheme.typography.bodyMedium)
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        EncoreField(
                            username,
                            { username = it },
                            placeholder = PlaylistCollabRules.USER_PLACEHOLDER,
                            imeAction = ImeAction.Go,
                            onAction = { invite() },
                            modifier = Modifier.weight(1f),
                        )
                        SmallButton("Invite", primary = true, enabled = !busy && username.isNotBlank()) { invite() }
                    }
                    error?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = EncoreColors.Err) }
                    if (d.link.isNotEmpty()) {
                        Text(nav.graph.api.url(d.link).takeIf { !d.link.startsWith("http") } ?: d.link, style = MaterialTheme.typography.bodySmall)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            SmallButton("Copy link", iconRes = R.drawable.ic_content_copy) {
                                copyToClipboard(context, if (d.link.startsWith("http")) d.link else nav.graph.api.url(d.link))
                            }
                            SmallButton("Turn off link", enabled = !busy) { run({ nav.features.dropInviteLink(id) }, "The invite link stopped working") }
                        }
                    } else {
                        SmallButton("Create invite link", iconRes = R.drawable.ic_link, enabled = !busy) { run({ nav.features.makeInviteLink(id) }) }
                        Text(PlaylistCollabRules.LINK_HINT, style = MaterialTheme.typography.bodySmall, color = p.faint)
                    }
                } else {
                    Text("${PlaylistCollabRules.MEMBER_HINT}. Only ${d.owner} can rename, share or delete it.", style = MaterialTheme.typography.bodyMedium)
                }
                Text(PlaylistCollabRules.MEMBERS_TITLE, fontWeight = FontWeight.SemiBold, color = p.text)
                Text("Owner: ${d.owner}", style = MaterialTheme.typography.bodySmall)
                if (d.members.isEmpty()) Text("Nobody yet.", style = MaterialTheme.typography.bodySmall, color = p.faint)
                d.members.forEach { m ->
                    Row(Modifier.fillMaxWidth().padding(vertical = 2.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(m.username, color = p.text, modifier = Modifier.weight(1f))
                        if (m.invited) Badge("invited")
                        if (d.isOwner) SmallButton("Remove", enabled = !busy) { run({ nav.features.removeCollaborator(id, m.userId) }) }
                    }
                }
                if (!d.isOwner) {
                    SmallButton(PlaylistCollabRules.LEAVE, danger = true, iconRes = R.drawable.ic_close, enabled = !busy) {
                        busy = true
                        scope.launch {
                            try {
                                // a member's Delete is Leave: the playlist stays the owner's
                                withContext(Dispatchers.IO) { nav.features.deletePlaylist(id) }
                                Toasts.show(PlaylistCollabRules.LEFT, "ok")
                                onLeft()
                            } catch (e: CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                error = e.message ?: FAILED
                                busy = false
                            }
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Done", color = p.text, fontWeight = FontWeight.SemiBold) } },
    )
}

/** The invites waiting for the user, over the Playlists grid: Join opens the playlist, Decline drops the invite. */
@Composable
fun CollabInvites(nav: Nav, invites: List<PlaylistCollabRules.Invite>, onChanged: () -> Unit) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        invites.forEach { i ->
            Column(Modifier.fillMaxWidth()) {
                Text(PlaylistCollabRules.inviteText(i), style = MaterialTheme.typography.bodyMedium, color = p.text)
                Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    SmallButton("Join", primary = true) {
                        scope.launch {
                            try {
                                withContext(Dispatchers.IO) { nav.features.acceptCollabInvite(i.playlistId) }
                                Toasts.show(PlaylistCollabRules.JOINED, "ok")
                                nav.open(Screen.Playlist(i.playlistId, i.name))
                            } catch (e: CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                Toasts.show(e.message ?: FAILED, "err")
                            }
                            onChanged()
                        }
                    }
                    SmallButton("Decline") {
                        scope.launch {
                            try {
                                withContext(Dispatchers.IO) { nav.features.declineCollabInvite(i.playlistId) }
                            } catch (e: CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                Toasts.show(e.message ?: FAILED, "err")
                            }
                            onChanged()
                        }
                    }
                }
            }
        }
    }
}
