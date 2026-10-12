package app.encore.client.media

import android.Manifest
import android.app.Activity
import android.app.DownloadManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.webkit.CookieManager
import android.webkit.URLUtil
import android.webkit.WebView
import android.widget.Toast
import androidx.core.app.ActivityCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import org.json.JSONObject

@InvokeArg
class NowPlayingArgs {
    var title: String = ""
    var artist: String = ""
    var album: String = ""
    var art: String = ""
    var playing: Boolean = false
    var stopped: Boolean = false
}

@InvokeArg
class UrlArgs {
    lateinit var url: String
}

/**
 * The Android half of the Encore app (the Rust side is clients/app/src/mobile.rs): the media notification with
 * lock-screen controls, "Share → Encore" links, downloads from the web app, the webview's cookies, and the
 * server address the car service (CarService, Android Auto) plays from.
 */
@TauriPlugin
class MediaPlugin(private val activity: Activity) : Plugin(activity) {
    companion object {
        @Volatile var current: MediaPlugin? = null
    }

    private var webView: WebView? = null
    @Volatile private var pendingShare: String? = null

    override fun load(webView: WebView) {
        this.webView = webView
        current = this
        // downloads from the web app (album zips, playlist exports) → Music/Encore with a notification
        webView.setDownloadListener { url, ua, disposition, mime, _ ->
            try {
                val r = DownloadManager.Request(Uri.parse(url))
                CookieManager.getInstance().getCookie(url)?.let { r.addRequestHeader("Cookie", it) }
                r.addRequestHeader("User-Agent", ua)
                val name = URLUtil.guessFileName(url, disposition, mime)
                r.setTitle(name)
                r.setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
                r.setDestinationInExternalPublicDir(Environment.DIRECTORY_MUSIC, "Encore/$name")
                (activity.getSystemService(Context.DOWNLOAD_SERVICE) as DownloadManager).enqueue(r)
                Toast.makeText(activity, "Downloading $name", Toast.LENGTH_SHORT).show()
            } catch (e: Exception) {
                Toast.makeText(activity, "Download failed: ${e.message}", Toast.LENGTH_SHORT).show()
            }
        }
        if (Build.VERSION.SDK_INT >= 33 &&
            activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            ActivityCompat.requestPermissions(activity, arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        takeShareFrom(activity.intent)
    }

    override fun onNewIntent(intent: Intent) {
        takeShareFrom(intent)
        deliverShare()
    }

    // "Share → Encore" from Spotify, YouTube Music, a browser…: the first link in the shared text
    private fun takeShareFrom(intent: Intent?) {
        if (intent == null || intent.action != Intent.ACTION_SEND) return
        val text = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
        Regex("https?://\\S+").find(text)?.let { pendingShare = it.value }
    }

    /** The Encore page is open: hand it the shared link right away (otherwise Rust asks once the page is up). */
    private fun deliverShare() {
        val link = pendingShare ?: return
        val w = webView ?: return
        val u = w.url ?: return
        if (!u.startsWith("http")) return
        pendingShare = null
        activity.runOnUiThread { w.evaluateJavascript(shareJs(link), null) }
    }

    /** The server the app connected to – the car service plays from it with no web view open. */
    @Command
    fun setServer(invoke: Invoke) {
        val a = invoke.parseArgs(UrlArgs::class.java)
        EncoreApi.setServer(activity, a.url)
        invoke.resolve()
    }

    /** Show, update or (stopped) remove the media notification. */
    @Command
    fun nowPlaying(invoke: Invoke) {
        val a = invoke.parseArgs(NowPlayingArgs::class.java)
        if (a.playing) CarService.instance?.pauseFromWebView()   // one player at a time
        val i = Intent(activity, PlaybackService::class.java)
        if (a.stopped) {
            activity.stopService(i)
        } else {
            i.putExtra("title", a.title).putExtra("artist", a.artist).putExtra("album", a.album)
                .putExtra("art", a.art).putExtra("playing", a.playing)
                .putExtra("cookie", if (a.art.isEmpty()) "" else CookieManager.getInstance().getCookie(a.art) ?: "")
            if (Build.VERSION.SDK_INT >= 26) activity.startForegroundService(i) else activity.startService(i)
        }
        invoke.resolve()
    }

    /** The link shared into the app, once. */
    @Command
    fun takeShare(invoke: Invoke) {
        val r = JSObject()
        r.put("url", pendingShare ?: "")
        pendingShare = null
        invoke.resolve(r)
    }

    /** The webview's cookies for a URL (the Encore session), as a Cookie header value. */
    @Command
    fun cookie(invoke: Invoke) {
        val a = invoke.parseArgs(UrlArgs::class.java)
        val r = JSObject()
        r.put("cookie", CookieManager.getInstance().getCookie(a.url) ?: "")
        invoke.resolve(r)
    }

    /** A notification / lock-screen button: toggle, next or prev on the web player. */
    fun control(action: String) {
        val w = webView ?: return
        activity.runOnUiThread { w.evaluateJavascript("window.encorePlayer && window.encorePlayer.$action()", null) }
    }
}

/** Asks the Encore server to download the shared link (same call the web app's Downloader makes). */
fun shareJs(link: String): String =
    "fetch('/api/requests/link',{method:'POST',headers:{'Content-Type':'application/json','X-Requested-With':'encore'}," +
    "body:JSON.stringify({url:" + JSONObject.quote(link) + "})}).then(r=>r.json().then(d=>alert(r.ok?(d.mode==='direct'?'Downloading':'Requested '+d.requested+' song(s)'):(d.detail||'Failed'))))"
