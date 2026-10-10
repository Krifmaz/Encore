package app.encore.client;

import android.Manifest;
import android.app.Activity;
import android.app.DownloadManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.graphics.Bitmap;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.view.ViewGroup;
import android.webkit.CookieManager;
import android.webkit.JavascriptInterface;
import android.webkit.URLUtil;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.FrameLayout;
import android.widget.ProgressBar;
import android.view.Gravity;
import android.view.View;
import android.webkit.WebResourceResponse;
import android.widget.Toast;

import org.json.JSONObject;

import java.net.HttpURLConnection;
import java.net.URL;

/**
 * Encore for Android: your Encore server in a full-screen app, with background playback,
 * lock-screen / notification controls, downloads, uploads and "Share → Encore" for links.
 */
public class MainActivity extends Activity {
    private static final String PREFS = "encore";
    private static final int FILE_CHOOSER = 41;
    static MainActivity current;

    private WebView web;
    private ValueCallback<Uri[]> fileCallback;
    private String pendingShare;
    private ProgressBar loading;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        current = this;
        web = new WebView(this);
        web.setBackgroundColor(0xFF0B0B12);
        FrameLayout root = new FrameLayout(this);
        root.setBackgroundColor(0xFF0B0B12);
        root.addView(web, new ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        loading = new ProgressBar(this);
        root.addView(loading, new FrameLayout.LayoutParams(ViewGroup.LayoutParams.WRAP_CONTENT, ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.CENTER));
        // Android 15 draws apps edge-to-edge: keep the page clear of the status bar and gesture bar
        root.setOnApplyWindowInsetsListener((v, insets) -> {
            v.setPadding(insets.getSystemWindowInsetLeft(), insets.getSystemWindowInsetTop(),
                    insets.getSystemWindowInsetRight(), insets.getSystemWindowInsetBottom());
            return insets.consumeSystemWindowInsets();
        });
        setContentView(root);

        WebSettings s = web.getSettings();
        s.setJavaScriptEnabled(true);
        s.setDomStorageEnabled(true);
        s.setMediaPlaybackRequiresUserGesture(false);
        s.setAllowFileAccess(true);   // the built-in offline page plays files saved in app storage
        s.setMixedContentMode(WebSettings.MIXED_CONTENT_COMPATIBILITY_MODE);
        s.setUserAgentString(s.getUserAgentString() + " EncoreAndroid/" + BuildConfig.VERSION_NAME);
        CookieManager.getInstance().setAcceptCookie(true);
        CookieManager.getInstance().setAcceptThirdPartyCookies(web, true);
        web.addJavascriptInterface(new Bridge(), "EncoreNative");

        web.setWebViewClient(new WebViewClient() {
            @Override
            public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest req) {
                Uri u = req.getUrl();
                if ("file".equals(u.getScheme())) return false;
                Uri home = Uri.parse(server());
                if (u.getHost() != null && u.getHost().equals(home.getHost()) && u.getPort() == home.getPort()) return false;
                startActivity(new Intent(Intent.ACTION_VIEW, u));   // external links open in the browser
                return true;
            }

            @Override
            public void onPageStarted(WebView view, String url, Bitmap favicon) { }

            @Override
            public void onReceivedHttpError(WebView view, WebResourceRequest req, WebResourceResponse res) {
                // the saved address now answers with an error (moved port, other service, proxy down): ask again
                if (req.isForMainFrame() && !req.getUrl().toString().startsWith("file:") && res.getStatusCode() >= 400) unreachable(server() + " answered " + res.getStatusCode());
            }

            @Override
            public void onPageFinished(WebView view, String url) {
                loading.setVisibility(View.GONE);
                if (pendingShare != null && !url.startsWith("file:")) { shareLink(pendingShare); pendingShare = null; }
            }

            @Override
            public void onReceivedError(WebView view, WebResourceRequest req, WebResourceError err) {
                if (req.isForMainFrame()) loading.setVisibility(View.GONE);
                if (req.isForMainFrame() && !req.getUrl().toString().startsWith("file:")) unreachable("Can't reach " + server());
            }
        });
        web.setWebChromeClient(new WebChromeClient() {
            @Override
            public boolean onShowFileChooser(WebView v, ValueCallback<Uri[]> cb, FileChooserParams params) {
                fileCallback = cb;
                try {
                    startActivityForResult(params.createIntent(), FILE_CHOOSER);
                } catch (Exception e) {
                    fileCallback = null;
                    return false;
                }
                return true;
            }
        });
        web.setDownloadListener((url, ua, disposition, mime, length) -> {
            DownloadManager.Request r = new DownloadManager.Request(Uri.parse(url));
            r.addRequestHeader("Cookie", CookieManager.getInstance().getCookie(url));
            r.addRequestHeader("User-Agent", ua);
            String name = URLUtil.guessFileName(url, disposition, mime);
            r.setTitle(name);
            r.setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED);
            r.setDestinationInExternalPublicDir(Environment.DIRECTORY_MUSIC, "Encore/" + name);
            ((DownloadManager) getSystemService(Context.DOWNLOAD_SERVICE)).enqueue(r);
            Toast.makeText(this, "Downloading " + name, Toast.LENGTH_SHORT).show();
        });

        if (Build.VERSION.SDK_INT >= 33 && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            requestPermissions(new String[]{Manifest.permission.POST_NOTIFICATIONS}, 1);
        }
        handleShare(getIntent());
        if (server().isEmpty()) askServer(null);
        else web.loadUrl(server());
    }

    String server() {
        return getSharedPreferences(PREFS, MODE_PRIVATE).getString("server", "");
    }

    static final String PICKER = "file:///android_asset/servers.html";
    static final String OFFLINE = "file:///android_asset/offline.html";

    // ---- offline music: files the user saved, kept in app storage, playable with no server ----
    java.io.File offlineDir() { java.io.File d = new java.io.File(getFilesDir(), "offline"); d.mkdirs(); return d; }

    org.json.JSONArray offlineIndex() {
        try (java.io.FileInputStream in = new java.io.FileInputStream(new java.io.File(offlineDir(), "index.json"))) {
            java.io.ByteArrayOutputStream b = new java.io.ByteArrayOutputStream();
            byte[] buf = new byte[8192];
            int n;
            while ((n = in.read(buf)) > 0) b.write(buf, 0, n);
            return new org.json.JSONArray(b.toString("UTF-8"));
        } catch (Exception e) { return new org.json.JSONArray(); }
    }

    synchronized void writeOfflineIndex(org.json.JSONArray a) {
        try (java.io.FileOutputStream o = new java.io.FileOutputStream(new java.io.File(offlineDir(), "index.json"))) {
            o.write(a.toString().getBytes("UTF-8"));
        } catch (Exception ignored) { }
    }

    boolean download(String url, java.io.File out) {
        try {
            HttpURLConnection c = (HttpURLConnection) new URL(url).openConnection();
            String cookie = CookieManager.getInstance().getCookie(url);
            if (cookie != null) c.setRequestProperty("Cookie", cookie);
            c.setConnectTimeout(10000);
            c.setReadTimeout(60000);
            if (c.getResponseCode() != 200) { c.disconnect(); return false; }
            java.io.File tmp = new java.io.File(out.getPath() + ".part");
            try (java.io.InputStream in = c.getInputStream(); java.io.OutputStream o = new java.io.FileOutputStream(tmp)) {
                byte[] buf = new byte[1 << 16];
                int n;
                while ((n = in.read(buf)) > 0) o.write(buf, 0, n);
            }
            c.disconnect();
            return tmp.renameTo(out);
        } catch (Exception e) { return false; }
    }

    /** No server: offline music if there is any, otherwise the server screen. */
    void unreachable(String problem) {
        loading.setVisibility(View.GONE);
        if (offlineIndex().length() > 0) web.loadUrl(OFFLINE + "?err=" + Uri.encode(problem));
        else askServer(problem);
    }

    /** First start, "Change server", or the saved server can't be reached: the built-in server screen. */
    void askServer(String problem) {
        loading.setVisibility(View.GONE);
        web.loadUrl(PICKER + (problem == null ? "" : "?err=" + Uri.encode(problem)));
    }

    org.json.JSONArray saved() {
        try { return new org.json.JSONArray(getSharedPreferences(PREFS, MODE_PRIVATE).getString("servers", "[]")); }
        catch (Exception e) { return new org.json.JSONArray(); }
    }

    void remember(String base) {
        org.json.JSONArray list = saved(), out = new org.json.JSONArray();
        out.put(base);
        for (int i = 0; i < list.length(); i++) if (!base.equals(list.optString(i))) out.put(list.optString(i));
        getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString("servers", out.toString()).putString("server", base).apply();
    }

    void check(String raw) {
        String url = raw.matches("^https?://.*") ? raw : "http://" + raw;
        final String base = url.replaceAll("/+$", "");
        new Thread(() -> {
            String err = null;
            try {
                HttpURLConnection c = (HttpURLConnection) new URL(base + "/api/health").openConnection();
                c.setConnectTimeout(6000);
                c.setReadTimeout(6000);
                int code = c.getResponseCode();
                if (code != 200) err = base + " answered " + code + " – is that your Encore address?";
                c.disconnect();
            } catch (Exception e) {
                err = "No Encore server answered at " + base;
            }
            final String problem = err;
            runOnUiThread(() -> {
                if (problem != null) { web.evaluateJavascript("window.onConnectError && onConnectError(" + JSONObject.quote(problem) + ")", null); return; }
                remember(base);
                loading.setVisibility(View.VISIBLE);
                web.clearCache(false);
                web.clearHistory();
                web.loadUrl(base);
            });
        }).start();
    }

    // "Share → Encore" from Spotify, YouTube Music, a browser…: download that link
    void handleShare(Intent i) {
        if (i == null || !Intent.ACTION_SEND.equals(i.getAction())) return;
        String text = i.getStringExtra(Intent.EXTRA_TEXT);
        if (text == null) return;
        java.util.regex.Matcher m = java.util.regex.Pattern.compile("https?://\\S+").matcher(text);
        if (m.find()) pendingShare = m.group();
    }

    void shareLink(String link) {
        String js = "fetch('/api/requests/link',{method:'POST',headers:{'Content-Type':'application/json','X-Requested-With':'encore'},"
                + "body:JSON.stringify({url:" + JSONObject.quote(link) + "})}).then(r=>r.json().then(d=>alert(r.ok?(d.mode==='direct'?'Downloading':'Requested '+d.requested+' song(s)'):(d.detail||'Failed'))))";
        web.evaluateJavascript(js, null);
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        handleShare(intent);
        if (pendingShare != null && !server().isEmpty()) { shareLink(pendingShare); pendingShare = null; }
    }

    void control(String action) {
        web.evaluateJavascript("window.encorePlayer && window.encorePlayer." + action + "()", null);
    }

    @Override
    public void onBackPressed() {
        if (web.canGoBack()) web.goBack();
        else moveTaskToBack(true);      // keep playing in the background instead of closing
    }

    @Override
    protected void onActivityResult(int req, int res, Intent data) {
        if (req == FILE_CHOOSER && fileCallback != null) {
            fileCallback.onReceiveValue(WebChromeClient.FileChooserParams.parseResult(res, data));
            fileCallback = null;
        }
        super.onActivityResult(req, res, data);
    }

    // No onPause()/pauseTimers(): audio keeps playing with the screen off (PlaybackService keeps us alive).

    @Override
    protected void onDestroy() {
        stopService(new Intent(this, PlaybackService.class));
        if (current == this) current = null;
        web.destroy();
        super.onDestroy();
    }

    /** window.EncoreNative – called by the web app. */
    class Bridge {
        @JavascriptInterface
        public void nowPlaying(String json) {
            try {
                JSONObject o = new JSONObject(json);
                Intent i = new Intent(MainActivity.this, PlaybackService.class);
                if (o.optBoolean("stopped")) { stopService(i); return; }
                i.putExtra("title", o.optString("title"));
                i.putExtra("artist", o.optString("artist"));
                i.putExtra("album", o.optString("album"));
                i.putExtra("art", o.optString("art"));
                i.putExtra("playing", o.optBoolean("playing"));
                i.putExtra("cookie", CookieManager.getInstance().getCookie(server()));
                if (Build.VERSION.SDK_INT >= 26) startForegroundService(i); else startService(i);
            } catch (Exception ignored) { }
        }

        @JavascriptInterface
        public String servers() {
            try {
                org.json.JSONArray list = saved();
                if (list.length() == 0 && !server().isEmpty()) list.put(server());
                return new JSONObject().put("list", list).put("current", server()).toString();
            } catch (Exception e) { return "{\"list\":[],\"current\":\"\"}"; }
        }

        /** Save a song for offline listening: {id, title, artist, album, url, art} (url/art absolute). */
        @JavascriptInterface
        public void saveOffline(String json) {
            new Thread(() -> {
                boolean ok = false;
                String title = "";
                try {
                    JSONObject o = new JSONObject(json);
                    String id = o.getString("id").replaceAll("[^A-Za-z0-9_-]", "");
                    title = o.optString("title");
                    String ext = o.optString("ext", "audio").replaceAll("[^a-z0-9]", "");
                    java.io.File f = new java.io.File(offlineDir(), id + "." + ext);
                    ok = f.exists() || download(o.getString("url"), f);
                    java.io.File art = new java.io.File(offlineDir(), id + ".jpg");
                    if (ok && !o.optString("art").isEmpty() && !art.exists()) download(o.getString("art"), art);
                    if (ok) {
                        synchronized (MainActivity.this) {
                            org.json.JSONArray idx = offlineIndex(), out = new org.json.JSONArray();
                            for (int i = 0; i < idx.length(); i++) if (!id.equals(idx.getJSONObject(i).optString("id"))) out.put(idx.getJSONObject(i));
                            out.put(new JSONObject().put("id", id).put("title", title).put("artist", o.optString("artist")).put("album", o.optString("album"))
                                    .put("file", Uri.fromFile(f).toString()).put("art", art.exists() ? Uri.fromFile(art).toString() : ""));
                            writeOfflineIndex(out);
                        }
                    }
                } catch (Exception ignored) { }
                final boolean done = ok;
                final String t = title;
                runOnUiThread(() -> Toast.makeText(MainActivity.this, done ? "Saved for offline: " + t : "Couldn't save " + t, Toast.LENGTH_SHORT).show());
            }).start();
        }

        @JavascriptInterface
        public String offlineList() { return offlineIndex().toString(); }

        @JavascriptInterface
        public String offlineIds() {
            org.json.JSONArray idx = offlineIndex(), ids = new org.json.JSONArray();
            for (int i = 0; i < idx.length(); i++) ids.put(idx.optJSONObject(i).optString("id"));
            return ids.toString();
        }

        @JavascriptInterface
        public void removeOffline(String id) {
            synchronized (MainActivity.this) {
                org.json.JSONArray idx = offlineIndex(), out = new org.json.JSONArray();
                for (int i = 0; i < idx.length(); i++) {
                    JSONObject o = idx.optJSONObject(i);
                    if (id.equals(o.optString("id"))) {
                        new java.io.File(Uri.parse(o.optString("file")).getPath()).delete();
                        new java.io.File(offlineDir(), id + ".jpg").delete();
                    } else out.put(o);
                }
                writeOfflineIndex(out);
            }
        }

        @JavascriptInterface
        public void openOffline() { runOnUiThread(() -> web.loadUrl(OFFLINE)); }

        @JavascriptInterface
        public void retryServer() { runOnUiThread(() -> { loading.setVisibility(View.VISIBLE); web.loadUrl(server()); }); }

        @JavascriptInterface
        public void connect(String url) { runOnUiThread(() -> check(url.trim())); }

        @JavascriptInterface
        public void removeServer(String url) {
            org.json.JSONArray list = saved(), out = new org.json.JSONArray();
            for (int i = 0; i < list.length(); i++) if (!url.equals(list.optString(i))) out.put(list.optString(i));
            getSharedPreferences(PREFS, MODE_PRIVATE).edit().putString("servers", out.toString()).apply();
        }

        @JavascriptInterface
        public void changeServer() {
            runOnUiThread(() -> askServer(null));
        }
    }
}
