package app.encore.android.data.session

import android.content.Context
import androidx.datastore.core.DataStore
import androidx.datastore.preferences.core.Preferences
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.core.stringSetPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import java.util.UUID

/** How requests are authenticated: a bearer app token (server feature `app_tokens`) or the web session cookie. */
enum class AuthKind { BEARER, COOKIE }

/** A signed-in server. `server` is the base URL without a trailing slash. */
data class Session(val server: String, val token: String, val kind: AuthKind, val username: String)

enum class ThemeChoice { ENCORE, OLED, LIGHT }

data class Settings(val theme: ThemeChoice = ThemeChoice.ENCORE, val autoplay: Boolean = true)

/** Where the app is: still reading the store, signed out, or signed in. */
sealed interface AuthState {
    data object Loading : AuthState
    data object SignedOut : AuthState
    data class SignedIn(val session: Session) : AuthState
}

private val Context.encoreStore: DataStore<Preferences> by preferencesDataStore(name = "encore")

/**
 * The session and the app's own settings, in DataStore.
 *
 * M1 keeps the token in plain DataStore (app-private storage, excluded from backups via allowBackup=false).
 * PLAN §4.2 moves it to a Tink AEAD key in the Android Keystore; that swaps only this class.
 *
 * [current] is a synchronous snapshot for OkHttp interceptors and the playback service.
 */
class SessionStore(context: Context, private val scope: CoroutineScope) {
    private val app = context.applicationContext
    private val store: DataStore<Preferences> = app.encoreStore
    private val loaded = CompletableDeferred<Unit>()

    private val _auth = MutableStateFlow<AuthState>(AuthState.Loading)
    val auth: StateFlow<AuthState> = _auth.asStateFlow()

    private val _settings = MutableStateFlow(Settings())
    val settings: StateFlow<Settings> = _settings.asStateFlow()

    private val _servers = MutableStateFlow<List<String>>(emptyList())
    val servers: StateFlow<List<String>> = _servers.asStateFlow()

    @Volatile var current: Session? = null
        private set

    @Volatile var deviceId: String = ""
        private set

    init {
        scope.launch {
            try {
                store.data.collect { p -> apply(p) }
            } catch (e: Exception) {
                _auth.value = AuthState.SignedOut
                if (!loaded.isCompleted) loaded.complete(Unit)
            }
        }
    }

    private fun apply(p: Preferences) {
        val server = p[SERVER] ?: ""
        val token = p[TOKEN] ?: ""
        val kind = if (p[KIND] == "cookie") AuthKind.COOKIE else AuthKind.BEARER
        val s = if (server.isNotEmpty() && token.isNotEmpty()) Session(server, token, kind, p[USERNAME] ?: "") else null
        current = s
        _auth.value = if (s != null) AuthState.SignedIn(s) else AuthState.SignedOut
        val theme = when (p[THEME]) {
            "oled" -> ThemeChoice.OLED
            "light" -> ThemeChoice.LIGHT
            else -> ThemeChoice.ENCORE
        }
        _settings.value = Settings(theme = theme, autoplay = p[AUTOPLAY] ?: true)
        _servers.value = (p[SERVERS] ?: emptySet()).sorted()
        val id = p[DEVICE_ID]
        if (id.isNullOrEmpty()) {
            val fresh = UUID.randomUUID().toString()
            deviceId = fresh
            // written outside the collector that is reading the store
            scope.launch { store.edit { it[DEVICE_ID] = fresh } }
        } else {
            deviceId = id
        }
        if (!loaded.isCompleted) loaded.complete(Unit)
    }

    /** Suspends until the first read from disk is done (the playback service can start before any UI). */
    suspend fun awaitLoaded() {
        loaded.await()
    }

    suspend fun signIn(session: Session) {
        store.edit {
            it[SERVER] = session.server
            it[TOKEN] = session.token
            it[KIND] = if (session.kind == AuthKind.COOKIE) "cookie" else "bearer"
            it[USERNAME] = session.username
            it[SERVERS] = (it[SERVERS] ?: emptySet()) + session.server
        }
    }

    /** Forgets the token (the server address stays as a suggestion). */
    suspend fun signOut() {
        current = null
        store.edit {
            it.remove(TOKEN)
            it.remove(USERNAME)
        }
    }

    /** A 401 on an authenticated call: the session expired or was revoked. Ignored if the token already changed. */
    fun onUnauthorized(token: String) {
        val s = current ?: return
        if (s.token != token) return
        scope.launch { signOut() }
    }

    suspend fun lastServer(): String = store.data.first()[SERVER] ?: ""

    suspend fun forgetServer(url: String) {
        store.edit { it[SERVERS] = (it[SERVERS] ?: emptySet()) - url }
    }

    fun setTheme(theme: ThemeChoice) {
        scope.launch { store.edit { it[THEME] = theme.name.lowercase() } }
    }

    fun setAutoplay(on: Boolean) {
        scope.launch { store.edit { it[AUTOPLAY] = on } }
    }

    private companion object {
        val SERVER = stringPreferencesKey("server")
        val TOKEN = stringPreferencesKey("token")
        val KIND = stringPreferencesKey("kind")
        val USERNAME = stringPreferencesKey("username")
        val SERVERS = stringSetPreferencesKey("servers")
        val DEVICE_ID = stringPreferencesKey("device_id")
        val THEME = stringPreferencesKey("theme")
        val AUTOPLAY = booleanPreferencesKey("autoplay")
    }
}
