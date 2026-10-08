package app.encore.client;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.os.Build;
import android.os.IBinder;
import android.support.v4.media.MediaMetadataCompat;
import android.support.v4.media.session.MediaSessionCompat;
import android.support.v4.media.session.PlaybackStateCompat;

import androidx.core.app.NotificationCompat;
import androidx.media.app.NotificationCompat.MediaStyle;

import java.net.HttpURLConnection;
import java.net.URL;

/**
 * Keeps Encore alive while music plays and shows the media notification / lock-screen controls.
 * The buttons call back into the web player (window.encorePlayer).
 */
public class PlaybackService extends Service {
    private static final String CHANNEL = "playback";
    private MediaSessionCompat session;
    private String lastArt = "";
    private Bitmap artBitmap;

    @Override
    public void onCreate() {
        super.onCreate();
        session = new MediaSessionCompat(this, "Encore");
        session.setCallback(new MediaSessionCompat.Callback() {
            @Override public void onPlay() { send("toggle"); }
            @Override public void onPause() { send("toggle"); }
            @Override public void onSkipToNext() { send("next"); }
            @Override public void onSkipToPrevious() { send("prev"); }
        });
        session.setActive(true);
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationChannel ch = new NotificationChannel(CHANNEL, "Playback", NotificationManager.IMPORTANCE_LOW);
            ch.setShowBadge(false);
            getSystemService(NotificationManager.class).createNotificationChannel(ch);
        }
    }

    private void send(String action) {
        MainActivity a = MainActivity.current;
        if (a != null) a.runOnUiThread(() -> a.control(action));
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && intent.getAction() != null) {
            send(intent.getAction());
            return START_NOT_STICKY;
        }
        if (intent == null) return START_NOT_STICKY;
        String title = intent.getStringExtra("title");
        String artist = intent.getStringExtra("artist");
        boolean playing = intent.getBooleanExtra("playing", false);
        String art = intent.getStringExtra("art");
        session.setMetadata(new MediaMetadataCompat.Builder()
                .putString(MediaMetadataCompat.METADATA_KEY_TITLE, title)
                .putString(MediaMetadataCompat.METADATA_KEY_ARTIST, artist)
                .putString(MediaMetadataCompat.METADATA_KEY_ALBUM, intent.getStringExtra("album"))
                .putBitmap(MediaMetadataCompat.METADATA_KEY_ALBUM_ART, artBitmap)
                .build());
        session.setPlaybackState(new PlaybackStateCompat.Builder()
                .setActions(PlaybackStateCompat.ACTION_PLAY_PAUSE | PlaybackStateCompat.ACTION_PLAY | PlaybackStateCompat.ACTION_PAUSE
                        | PlaybackStateCompat.ACTION_SKIP_TO_NEXT | PlaybackStateCompat.ACTION_SKIP_TO_PREVIOUS)
                .setState(playing ? PlaybackStateCompat.STATE_PLAYING : PlaybackStateCompat.STATE_PAUSED, PlaybackStateCompat.PLAYBACK_POSITION_UNKNOWN, 1f)
                .build());
        Notification n = build(title, artist, playing);
        if (Build.VERSION.SDK_INT >= 29) startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
        else startForeground(1, n);
        if (art != null && !art.isEmpty() && !art.equals(lastArt)) {
            lastArt = art;
            final String cookie = intent.getStringExtra("cookie");
            new Thread(() -> {
                try {
                    HttpURLConnection c = (HttpURLConnection) new URL(art).openConnection();
                    if (cookie != null) c.setRequestProperty("Cookie", cookie);
                    c.setConnectTimeout(5000);
                    artBitmap = BitmapFactory.decodeStream(c.getInputStream());
                    c.disconnect();
                    getSystemService(NotificationManager.class).notify(1, build(title, artist, playing));
                } catch (Exception ignored) { }
            }).start();
        }
        return START_NOT_STICKY;
    }

    private PendingIntent action(String a, int code) {
        Intent i = new Intent(this, PlaybackService.class).setAction(a);
        return PendingIntent.getService(this, code, i, PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
    }

    private Notification build(String title, String artist, boolean playing) {
        PendingIntent open = PendingIntent.getActivity(this, 0, new Intent(this, MainActivity.class).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        return new NotificationCompat.Builder(this, CHANNEL)
                .setSmallIcon(R.drawable.ic_note)
                .setContentTitle(title)
                .setContentText(artist)
                .setLargeIcon(artBitmap)
                .setContentIntent(open)
                .setOngoing(playing)
                .setSilent(true)
                .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
                .addAction(R.drawable.ic_prev, "Previous", action("prev", 1))
                .addAction(playing ? R.drawable.ic_pause : R.drawable.ic_play, playing ? "Pause" : "Play", action("toggle", 2))
                .addAction(R.drawable.ic_next, "Next", action("next", 3))
                .setStyle(new MediaStyle().setMediaSession(session.getSessionToken()).setShowActionsInCompactView(0, 1, 2))
                .build();
    }

    @Override
    public void onDestroy() {
        session.release();
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
