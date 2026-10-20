package app.encore.android.ui.components

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * Share an album or artist link the way the player shares a song (PlayerConnection.shareLink): create the link
 * on the server, copy it, show the toast and open the system share sheet.
 *
 * [create] runs on Dispatchers.IO and returns the public url (EncoreApi.shareAlbumLink / shareArtistLink /
 * sharePlaylistLink). [after] runs once the link is made, before the share sheet (the playlist shows its Shared mark).
 */
fun shareLinkOf(context: Context, scope: CoroutineScope, subject: String, text: String, toast: String, after: () -> Unit = {}, create: () -> String) {
    scope.launch {
        try {
            val url = withContext(Dispatchers.IO) { create() }
            try {
                val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                cm?.setPrimaryClip(ClipData.newPlainText("Encore", url))
            } catch (_: Exception) {}
            Toasts.show(toast, "ok")
            after()
            val send = Intent(Intent.ACTION_SEND).apply {
                type = "text/plain"
                putExtra(Intent.EXTRA_SUBJECT, subject)
                putExtra(Intent.EXTRA_TEXT, if (text.isNotEmpty()) "$text\n$url" else url)
            }
            context.startActivity(Intent.createChooser(send, "Share link").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (e: Exception) {
            Toasts.show(e.message ?: "Couldn't create the link", "err")
        }
    }
}
