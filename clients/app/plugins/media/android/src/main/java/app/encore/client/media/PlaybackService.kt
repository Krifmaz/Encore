package app.encore.client.media

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.os.IBinder
import android.support.v4.media.MediaMetadataCompat
import android.support.v4.media.session.MediaSessionCompat
import android.support.v4.media.session.PlaybackStateCompat
import androidx.core.app.NotificationCompat
import androidx.media.app.NotificationCompat.MediaStyle
import java.net.HttpURLConnection
import java.net.URL

/**
 * Keeps Encore alive while music plays and shows the media notification / lock-screen controls.
 * The buttons call back into the web player through MediaPlugin.control().
 */
class PlaybackService : Service() {
    private lateinit var session: MediaSessionCompat
    private var lastArt = ""
    private var artBitmap: Bitmap? = null

    override fun onCreate() {
        super.onCreate()
        session = MediaSessionCompat(this, "Encore")
        session.setCallback(object : MediaSessionCompat.Callback() {
            override fun onPlay() = send("toggle")
            override fun onPause() = send("toggle")
            override fun onSkipToNext() = send("next")
            override fun onSkipToPrevious() = send("prev")
        })
        session.isActive = true
        if (Build.VERSION.SDK_INT >= 26) {
            val ch = NotificationChannel(CHANNEL, "Playback", NotificationManager.IMPORTANCE_LOW)
            ch.setShowBadge(false)
            getSystemService(NotificationManager::class.java).createNotificationChannel(ch)
        }
    }

    private fun send(action: String) {
        MediaPlugin.current?.control(action)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent == null) return START_NOT_STICKY
        intent.action?.let { send(it); return START_NOT_STICKY }
        val title = intent.getStringExtra("title") ?: ""
        val artist = intent.getStringExtra("artist") ?: ""
        val playing = intent.getBooleanExtra("playing", false)
        val art = intent.getStringExtra("art") ?: ""
        session.setMetadata(MediaMetadataCompat.Builder()
            .putString(MediaMetadataCompat.METADATA_KEY_TITLE, title)
            .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, artist)
            .putString(MediaMetadataCompat.METADATA_KEY_ALBUM, intent.getStringExtra("album") ?: "")
            .putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, artBitmap)
            .build())
        session.setPlaybackState(PlaybackStateCompat.Builder()
            .setActions(PlaybackStateCompat.ACTION_PLAY_PAUSE or PlaybackStateCompat.ACTION_PLAY or PlaybackStateCompat.ACTION_PAUSE
                or PlaybackStateCompat.ACTION_SKIP_TO_NEXT or PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS)
            .setState(if (playing) PlaybackStateCompat.STATE_PLAYING else PlaybackStateCompat.STATE_PAUSED,
                PlaybackStateCompat.PLAYBACK_POSITION_UNKNOWN, 1f)
            .build())
        val n = build(title, artist, playing)
        if (Build.VERSION.SDK_INT >= 29) startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK) else startForeground(1, n)
        if (art.isNotEmpty() && art != lastArt) {
            lastArt = art
            val cookie = intent.getStringExtra("cookie") ?: ""
            Thread {
                try {
                    val c = URL(art).openConnection() as HttpURLConnection
                    if (cookie.isNotEmpty()) c.setRequestProperty("Cookie", cookie)
                    c.connectTimeout = 5000
                    artBitmap = BitmapFactory.decodeStream(c.inputStream)
                    c.disconnect()
                    getSystemService(NotificationManager::class.java).notify(1, build(title, artist, playing))
                } catch (_: Exception) { }
            }.start()
        }
        return START_NOT_STICKY
    }

    private fun action(a: String, code: Int): PendingIntent {
        val i = Intent(this, PlaybackService::class.java).setAction(a)
        return PendingIntent.getService(this, code, i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    private fun build(title: String, artist: String, playing: Boolean): Notification {
        val launch = packageManager.getLaunchIntentForPackage(packageName)?.addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val open = PendingIntent.getActivity(this, 0, launch ?: Intent(), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.ic_note)
            .setContentTitle(title)
            .setContentText(artist)
            .setLargeIcon(artBitmap)
            .setContentIntent(open)
            .setOngoing(playing)
            .setSilent(true)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .addAction(R.drawable.ic_prev, "Previous", action("prev", 1))
            .addAction(if (playing) R.drawable.ic_pause else R.drawable.ic_play, if (playing) "Pause" else "Play", action("toggle", 2))
            .addAction(R.drawable.ic_next, "Next", action("next", 3))
            .setStyle(MediaStyle().setMediaSession(session.sessionToken).setShowActionsInCompactView(0, 1, 2))
            .build()
    }

    override fun onDestroy() {
        session.release()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    companion object {
        private const val CHANNEL = "playback"
    }
}
