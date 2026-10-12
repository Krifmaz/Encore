package app.encore.client.media

import android.content.Context
import android.webkit.CookieManager
import org.json.JSONArray
import org.json.JSONObject
import org.json.JSONTokener
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL
import java.net.URLEncoder

/**
 * The Encore server as the native (car) player sees it: the saved server address and the web view's session
 * cookie, plus small JSON helpers. The Rust side stores the server address here whenever the app connects
 * (MediaPlugin.setServer), so the service can work with no web view open – Android Auto starts it on its own.
 */
class EncoreApi(ctx: Context) {
    private val prefs = ctx.getSharedPreferences("encore", Context.MODE_PRIVATE)

    val base: String get() = prefs.getString("server", "") ?: ""
    val configured: Boolean get() = base.startsWith("http")

    fun cookie(): String = try { CookieManager.getInstance().getCookie(base) ?: "" } catch (_: Exception) { "" }

    /** Headers every request carries: the session cookie and the header the server wants on writes. */
    fun headers(): Map<String, String> = mapOf("Cookie" to cookie(), "X-Requested-With" to "encore")

    fun url(path: String): String = base + path
    fun enc(s: String): String = URLEncoder.encode(s, "UTF-8").replace("+", "%20")

    fun get(path: String): Any? = call("GET", path, null)
    fun post(path: String, body: JSONObject): Any? = call("POST", path, body)
    fun getArray(path: String): JSONArray = when (val v = get(path)) { is JSONArray -> v; is JSONObject -> v.optJSONArray("items") ?: JSONArray(); else -> JSONArray() }
    fun getObject(path: String): JSONObject = (get(path) as? JSONObject) ?: JSONObject()

    private fun call(method: String, path: String, body: JSONObject?): Any? {
        if (!configured) return null
        val c = URL(url(path)).openConnection() as HttpURLConnection
        try {
            c.requestMethod = method
            c.connectTimeout = 8000
            c.readTimeout = 20000
            for ((k, v) in headers()) if (v.isNotEmpty()) c.setRequestProperty(k, v)
            c.setRequestProperty("Accept", "application/json")
            if (body != null) {
                c.setRequestProperty("Content-Type", "application/json")
                c.doOutput = true
                c.outputStream.use { it.write(body.toString().toByteArray()) }
            }
            val code = c.responseCode
            val stream = if (code >= 400) c.errorStream else c.inputStream
            val text = stream?.let { BufferedReader(InputStreamReader(it)).use { r -> r.readText() } } ?: ""
            if (code >= 400) throw ApiException(code, (JSONTokener(text).nextValue() as? JSONObject)?.optString("detail") ?: "HTTP $code")
            return if (text.isBlank()) null else JSONTokener(text).nextValue()
        } finally {
            c.disconnect()
        }
    }

    class ApiException(val code: Int, message: String) : Exception(message)

    companion object {
        /** The Rust side saves the server the app connected to; the car service reads it. */
        fun setServer(ctx: Context, url: String) {
            ctx.getSharedPreferences("encore", Context.MODE_PRIVATE).edit().putString("server", url.trimEnd('/')).apply()
        }
    }
}
