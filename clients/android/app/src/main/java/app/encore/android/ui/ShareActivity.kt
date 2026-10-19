package app.encore.android.ui

import android.content.Intent
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import app.encore.android.graph
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * "Share → Request in Encore": takes the first link from shared text (Spotify, Deezer, YouTube, Bandcamp …) and
 * asks the server to get it (POST /api/requests/link, the same box as the web's Requests page). No UI of its
 * own: a toast says what happened.
 */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val shared: Intent? = intent
        val text = if (shared != null && shared.action == Intent.ACTION_SEND) shared.getStringExtra(Intent.EXTRA_TEXT) ?: "" else ""
        val link = LINK.find(text)?.value ?: text.trim().takeIf { it.startsWith("spotify:") }
        val app = applicationContext
        val graph = app.graph
        // a group listen invite (https://<server>/#/group/<code>): the app opens and asks to join
        if (text.contains("#/group/")) {
            val open = Intent(this, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .putExtra(MainActivity.EXTRA_GROUP_LINK, text.trim())
            try { startActivity(open) } catch (_: Exception) {}
            finish()
            return
        }
        finish()
        if (link == null) {
            Toast.makeText(app, "No link to request", Toast.LENGTH_SHORT).show()
            return
        }
        graph.scope.launch {
            try { graph.store.awaitLoaded() } catch (_: Exception) {}
            if (graph.store.current == null) {
                Toast.makeText(app, "Sign in to Encore first", Toast.LENGTH_LONG).show()
                return@launch
            }
            val message = try {
                withContext(Dispatchers.IO) { graph.api.requestLink(link) }
            } catch (e: Exception) {
                "Encore: " + (e.message ?: "request failed")
            }
            Toast.makeText(app, message, Toast.LENGTH_LONG).show()
        }
    }

    private companion object {
        val LINK = Regex("""https?://\S+""")
    }
}
