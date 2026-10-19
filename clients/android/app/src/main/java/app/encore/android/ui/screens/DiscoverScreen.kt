package app.encore.android.ui.screens

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateMap
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import app.encore.android.features.library.OnSongFixed
import app.encore.android.R
import app.encore.android.data.api.ApiException
import app.encore.android.data.api.LibrarySearch
import app.encore.android.data.repo.CatalogAlbum
import app.encore.android.data.repo.CatalogArtist
import app.encore.android.data.repo.CatalogTop
import app.encore.android.data.repo.CatalogTrack
import app.encore.android.data.repo.FeaturesApi
import app.encore.android.data.repo.SearchResults
import app.encore.android.data.repo.features
import app.encore.android.data.repo.statusKind
import app.encore.android.data.repo.statusLabel
import app.encore.android.ui.Nav
import app.encore.android.ui.Screen
import app.encore.android.ui.components.AlbumCard
import app.encore.android.ui.components.ArtPlaceholder
import app.encore.android.ui.components.Badge
import app.encore.android.ui.components.ChoiceRow
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.EmptyState
import app.encore.android.ui.components.EncoreTopBar
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.components.Gutter
import app.encore.android.ui.components.PageTitle
import app.encore.android.ui.components.SectionHead
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.Tile
import app.encore.android.ui.components.Toasts
import app.encore.android.ui.components.TrackRow
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import coil3.compose.AsyncImage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// ---- widgets shared by the feature screens (Discover, For you, Requests, picker, playlists, Account) ------------

/** The [FeaturesApi] behind the feature screens. */
val Nav.features: FeaturesApi get() = graph.features

/** The web's `.input`: an outlined field in the panel colours. */
@Composable
fun EncoreField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    placeholder: String = "",
    label: String? = null,
    leadingIcon: Int? = null,
    enabled: Boolean = true,
    singleLine: Boolean = true,
    minLines: Int = 1,
    password: Boolean = false,
    keyboardType: KeyboardType = KeyboardType.Text,
    imeAction: ImeAction = ImeAction.Done,
    onAction: (() -> Unit)? = null,
) {
    val p = MaterialTheme.palette
    val focus = LocalFocusManager.current
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        placeholder = if (placeholder.isNotEmpty()) ({ Text(placeholder, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis) }) else null,
        label = if (label != null) ({ Text(label) }) else null,
        leadingIcon = if (leadingIcon != null) ({ Icon(painterResource(leadingIcon), contentDescription = null, tint = p.faint) }) else null,
        singleLine = singleLine,
        minLines = minLines,
        enabled = enabled,
        shape = RoundedCornerShape(10.dp),
        visualTransformation = if (password) PasswordVisualTransformation() else VisualTransformation.None,
        keyboardOptions = KeyboardOptions(keyboardType = keyboardType, imeAction = imeAction),
        keyboardActions = KeyboardActions(onAny = { focus.clearFocus(); onAction?.invoke() }),
        colors = OutlinedTextFieldDefaults.colors(
            focusedBorderColor = p.accent,
            unfocusedBorderColor = p.border,
            focusedContainerColor = p.bg2,
            unfocusedContainerColor = p.bg2,
            cursorColor = p.accent,
        ),
        modifier = modifier,
    )
}

/** The web's `.card`: a panel with a border and 14dp corners. */
@Composable
fun CardBox(modifier: Modifier = Modifier, flat: Boolean = false, content: @Composable () -> Unit) {
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(14.dp)
    Box(
        modifier
            .fillMaxWidth()
            .clip(shape)
            .background(if (flat) p.bg2 else p.panel)
            .border(1.dp, p.border, shape)
            .padding(if (flat) 0.dp else 16.dp),
    ) { content() }
}

/** The web's `statusBadge(status)`: "Awaiting approval", "Queued", "Available"… in the status colour. */
@Composable
fun StatusBadge(status: String, modifier: Modifier = Modifier) {
    if (status.isEmpty()) return
    Badge(statusLabel(status), kind = statusKind(status), modifier = modifier)
}

/** The web's `availability(item)`: "In library" when owned, else the request status, else nothing. */
@Composable
fun Availability(inLibrary: Boolean, requestStatus: String, modifier: Modifier = Modifier) {
    when {
        inLibrary -> Badge("In library", kind = "ok", modifier = modifier)
        requestStatus.isNotEmpty() -> StatusBadge(requestStatus, modifier)
    }
}

/** The web's `confirmDialog(title, text, okLabel, danger)`. */
@Composable
fun ConfirmDialog(title: String, text: String, okLabel: String, danger: Boolean = false, onDismiss: () -> Unit, onConfirm: () -> Unit) {
    val p = MaterialTheme.palette
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = p.panel,
        titleContentColor = p.text,
        textContentColor = p.muted,
        title = { Text(title) },
        text = { if (text.isNotEmpty()) Text(text) },
        confirmButton = {
            TextButton(onClick = { onDismiss(); onConfirm() }) { Text(okLabel, color = if (danger) EncoreColors.Err else p.text, fontWeight = FontWeight.SemiBold) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = p.muted) } },
    )
}

/** A small spinner for inline "working" states. */
@Composable
fun Spinner(size: Dp = 16.dp) {
    CircularProgressIndicator(color = MaterialTheme.palette.accent, trackColor = MaterialTheme.palette.border, strokeWidth = 2.dp, modifier = Modifier.size(size))
}

/** Opens a link in the browser (the web's `target="_blank"` links). */
fun openUrl(context: android.content.Context, url: String) {
    if (url.isEmpty()) return
    try {
        context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    } catch (_: Exception) {
        Toasts.show("No app can open that link", "err")
    }
}

/** Request statuses the page changed since it loaded (keyed by the item's id), so badges update in place. */
typealias RequestStatuses = SnapshotStateMap<String, String>

@Composable
fun rememberRequestStatuses(): RequestStatuses = remember { mutableStateMapOf() }

// ---- live request labels: requested → searching → downloading → importing → in library -----------------------
// Each request event (GET /api/requests/events) names the keys a page knows the release or song by (server:
// req_events.rs): `rg:<release group>`, `rec:<recording>`, `ar:<artist>`, `ext:deezer:<id>`…, `a:` / `t:` + artist|title.
// They go into the page's [RequestStatuses] under those keys (and the bare MusicBrainz id the pages used before),
// the label under "label:<key>".

/** Lower case, letters and digits only (the server's `norm`). */
fun reqNorm(s: String): String = s.lowercase().filter { it.isLetterOrDigit() }

/** The key of a Deezer / Apple Music / Bandcamp / Discogs release in request events. */
fun extKeyOf(ext: JsonElement?): String {
    val o = ext as? JsonObject ?: return ""
    for (k in listOf("deezer", "apple", "bandcamp", "discogs")) {
        val v = (o[k] as? JsonPrimitive)?.content ?: continue
        if (v.isNotEmpty() && v != "null") return "ext:$k:$v"
    }
    return ""
}

fun albumNameKey(artist: String, title: String): String = "a:${reqNorm(artist)}|${reqNorm(title)}"
fun songNameKey(artist: String, title: String): String = "t:${reqNorm(artist)}|${reqNorm(title)}"

/** Every key a catalogue album is known by, the live ones first. */
fun albumKeys(a: CatalogAlbum): List<String> = listOf(extKeyOf(a.ext), a.id, albumNameKey(a.artist, a.title)).filter { it.isNotEmpty() }

/** Every key a catalogue song is known by. */
fun trackKeys(t: CatalogTrack): List<String> = listOf(t.id, songNameKey(t.artist, t.title), t.key).filter { it.isNotEmpty() }.distinct()

/** The status of an item known by [keys] (the first key set wins), else what the server said when the page loaded. */
fun RequestStatuses.statusOf(keys: List<String>, fallback: String): String = keys.firstNotNullOfOrNull { this[it] } ?: fallback

/** The label that came with the status [statusOf] found ("" for the default words). */
fun RequestStatuses.labelOf(keys: List<String>, fallback: String): String {
    val k = keys.firstOrNull { this[it] != null } ?: return fallback
    return this["label:$k"] ?: ""
}

/** Sets the status (and label) of an item under all its keys. */
fun RequestStatuses.mark(keys: List<String>, status: String, label: String = "") {
    for (k in keys) {
        if (k.isEmpty()) continue
        this[k] = status
        if (label.isEmpty()) remove("label:$k") else this["label:$k"] = label
    }
}

/** The words of a request status on catalogue pages (the web's `liveBadge`). */
private val LIVE_STATUS: Map<String, Pair<String, String>> = mapOf(
    "pending" to ("warn" to "Awaiting approval"),
    "queued" to ("info" to "Requested"),
    "searching" to ("info" to "Searching"),
    "monitoring" to ("accent" to "Downloading"),
    "downloading" to ("accent" to "Downloading"),
    "importing" to ("accent" to "Importing"),
    "waiting" to ("warn" to "Waiting"),
    "completed" to ("ok" to "In library"),
    "failed" to ("err" to "Failed"),
    "denied" to ("err" to "Denied"),
    "cancelled" to ("" to "Cancelled"),
)

/** A request's badge: the server's label when it sent one (Not found, Discography requested…), else the status in words. */
@Composable
fun LiveBadge(status: String, label: String = "", modifier: Modifier = Modifier) {
    if (status.isEmpty()) return
    val (kind, words) = LIVE_STATUS[status] ?: ("info" to "Requested")
    Badge(label.ifEmpty { words }, kind = kind, modifier = modifier)
}

/** "In library" when owned, else the live request badge, else nothing. */
@Composable
fun LiveAvailability(inLibrary: Boolean, status: String, label: String, modifier: Modifier = Modifier) {
    when {
        inLibrary -> Badge("In library", kind = "ok", modifier = modifier)
        status.isNotEmpty() -> LiveBadge(status, label, modifier)
    }
}

/** Follows the request events while the page is open, so its badges change as requests move along. */
@Composable
fun LiveRequestStatuses(nav: Nav, statuses: RequestStatuses) {
    LaunchedEffect(Unit) {
        withContext(Dispatchers.IO) {
            nav.features.requestEvents { ev ->
                val keys = ((ev["keys"] as? JsonArray) ?: emptyList<JsonElement>()).mapNotNull { (it as? JsonPrimitive)?.content }.flatMap { k ->
                    // the pages key MusicBrainz items by their bare id too
                    val bare = when { k.startsWith("rg:") || k.startsWith("rec:") -> k.substringAfter(':'); else -> "" }
                    listOf(k, bare)
                }
                val status = (ev["status"] as? JsonPrimitive)?.content ?: ""
                val label = (ev["label"] as? JsonPrimitive)?.content ?: ""
                if (status.isNotEmpty()) withContext(Dispatchers.Main) { statuses.mark(keys, status, label) }
            }
        }
    }
}

/**
 * The web's `submitRequest(body)`: POST /api/requests, a toast either way; returns the new status or null when
 * it failed. Runs the call on the IO dispatcher.
 */
suspend fun submitRequest(features: FeaturesApi, body: JsonObject): String? {
    return try {
        val status = withContext(Dispatchers.IO) { features.createRequest(body) }
        Toasts.show(if (status == "pending") "Request sent – waiting for approval" else "Request queued", "ok")
        status.ifEmpty { "queued" }
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        Toasts.show(e.message ?: "Request failed", "err")
        null
    }
}

/** True when the error is the weekly request limit (the loops stop then, like the web's `/limit/i`). */
fun isLimitError(e: Exception): Boolean = (e.message ?: "").contains("limit", ignoreCase = true) || (e is ApiException && e.code == 429)

/** POST /api/requests for a catalogue album (discover.js `albumBody`): by MusicBrainz id, or Deezer / Apple ids. */
fun albumRequestBody(a: CatalogAlbum): JsonObject = buildJsonObject {
    put("kind", "album")
    if (a.ext != null) put("ext", a.ext) else put("mbid", a.id)
    put("artist", a.artist)
    put("title", a.title)
    put("album", a.title)
    if (a.year.isNotEmpty()) put("year", a.year)
    if (a.cover.isNotEmpty()) put("cover_url", a.cover)
}

/** POST /api/requests for a MusicBrainz recording; `albumMode` is "song" (just it) or "album" (the whole album). */
fun trackRequestBody(t: CatalogTrack, albumMode: String): JsonObject = buildJsonObject {
    put("kind", "track")
    if (t.id.isNotEmpty()) put("mbid", t.id)
    if (t.isrc.isNotEmpty()) put("isrc", t.isrc)
    put("artist", t.artist)
    put("title", t.title)
    if (t.album.isNotEmpty()) put("album", t.album)
    if (t.cover.isNotEmpty()) put("cover_url", t.cover)
    if (t.year.isNotEmpty()) put("year", t.year)
    if (t.lengthSec > 0) put("length", t.lengthSec)
    put("album_mode", albumMode)
}

/** Plays a library album by id (the tiles' play button; the web's `playAlbum`). */
fun playLibraryAlbum(nav: Nav, albumId: String, scope: kotlinx.coroutines.CoroutineScope) {
    scope.launch {
        try {
            val songs = withContext(Dispatchers.IO) { nav.graph.api.album(albumId).tracks }
            if (songs.isEmpty()) Toasts.show("Nothing to play", "err") else nav.player.play(songs)
        } catch (e: Exception) {
            Toasts.show(e.message ?: "Couldn't load the album", "err")
        }
    }
}

/** Rows of `columns` tiles inside a LazyColumn (the web's `.grid` on a phone). */
fun <T> LazyListScope.tileRows(items: List<T>, columns: Int = 2, key: String = "", content: @Composable (T) -> Unit) {
    val rows = items.chunked(columns)
    items(rows.size, key = { i -> "$key-row-$i" }) { i ->
        val row = rows[i]
        Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 7.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            for (item in row) Box(Modifier.weight(1f)) { content(item) }
            for (j in row.size until columns) Spacer(Modifier.weight(1f))
        }
    }
}

/** A round "+" / play button on a tile's art (the web's `.fab`). */
@Composable
fun ArtFab(iconRes: Int, description: String, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Box(
        modifier.size(38.dp).clip(CircleShape).background(p.gradient).clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(painterResource(iconRes), contentDescription = description, tint = p.onAccent, modifier = Modifier.size(20.dp))
    }
}

/** The web's `.tile.round` for a MusicBrainz / Last.fm artist (discover.js `artistTile`, foryou.js `artistCard`). */
@Composable
fun CatalogArtistTile(nav: Nav, a: CatalogArtist, modifier: Modifier = Modifier, subtitle: String? = null) {
    val scope = rememberCoroutineScope()
    val sub = subtitle ?: listOf(a.type, a.country, a.disambiguation).filter { it.isNotEmpty() }.joinToString(" · ")
    Tile(
        title = a.name,
        subtitle = sub.ifEmpty { " " },
        modifier = modifier,
        centered = true,
        onClick = { openCatalogArtist(nav, a.id, a.name, scope) },
    ) { m ->
        Box(m.clip(CircleShape), contentAlignment = Alignment.BottomCenter) {
            ArtPlaceholder(a.name, Modifier.fillMaxSize())
            if (a.image.isNotEmpty()) {
                AsyncImage(model = a.image, contentDescription = null, imageLoader = nav.graph.imageLoader, modifier = Modifier.fillMaxSize(), contentScale = androidx.compose.ui.layout.ContentScale.Crop)
            }
            if (a.inLibrary) Badge("In library", kind = "ok", modifier = Modifier.padding(bottom = 8.dp))
        }
    }
}

// `openCatalogArtist` (the catalogue page of an artist, looked up by name when there is no id) lives in
// ArtistLinks.kt with the other artist navigation.

/**
 * The web's `albumTile` for a release group: cover, the "In library" / status corner badge, the play or request
 * button, title and "artist · year · type" (or [subtitle]).
 */
@Composable
fun CatalogAlbumTile(
    nav: Nav,
    a: CatalogAlbum,
    statuses: RequestStatuses,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    onClick: (() -> Unit)? = null,
) {
    val scope = rememberCoroutineScope()
    val keys = albumKeys(a)
    val status = statuses.statusOf(keys, a.requestStatus)
    val label = statuses.labelOf(keys, a.requestLabel)
    Tile(
        title = a.title,
        subtitle = subtitle ?: listOf(a.artist, a.year, a.type).filter { it.isNotEmpty() }.joinToString(" · "),
        modifier = modifier,
        onClick = onClick ?: { openCatalogAlbum(nav, a, scope) },
    ) { m ->
        Box(m) {
            Cover(a.cover.ifEmpty { null }, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 12.dp, label = a.title)
            LiveAvailability(a.inLibrary, status, label, Modifier.padding(8.dp))
            when {
                a.libraryAlbumId.isNotEmpty() -> ArtFab(R.drawable.ic_play, "Play", Modifier.align(Alignment.BottomEnd).padding(8.dp)) {
                    playLibraryAlbum(nav, a.libraryAlbumId, scope)
                }
                !a.inLibrary && status.isEmpty() -> ArtFab(R.drawable.ic_add, "Request album", Modifier.align(Alignment.BottomEnd).padding(8.dp)) {
                    scope.launch { submitRequest(nav.features, albumRequestBody(a))?.let { statuses.mark(keys, it) } }
                }
            }
        }
    }
}

/** Opens a catalogue album: its page, or (an album only Deezer / Apple know) its artist. */
fun openCatalogAlbum(nav: Nav, a: CatalogAlbum, scope: kotlinx.coroutines.CoroutineScope) {
    if (a.ext == null) nav.open(Screen.CatalogAlbum(a.id)) else openCatalogArtist(nav, a.artistId, a.artist, scope)
}

/** The web's `trackRow` of a MusicBrainz recording: play when owned, else "Song" / "+ Album" request buttons. */
@Composable
fun CatalogTrackRow(nav: Nav, t: CatalogTrack, statuses: RequestStatuses) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    val keys = trackKeys(t)
    val status = statuses.statusOf(keys, t.requestStatus)
    val label = statuses.labelOf(keys, t.requestLabel)
    val lib = t.library
    var busy by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(48.dp).clip(RoundedCornerShape(8.dp)).clickable(enabled = lib != null) { if (lib != null) nav.player.play(listOf(lib)) }) {
            Cover(t.cover.ifEmpty { null }, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 8.dp, label = t.album.ifEmpty { t.title })
            if (lib != null) {
                Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.35f)), contentAlignment = Alignment.Center) {
                    Icon(painterResource(R.drawable.ic_play), contentDescription = "Play", tint = Color.White, modifier = Modifier.size(22.dp))
                }
            }
        }
        Spacer(Modifier.width(14.dp))
        Column(Modifier.weight(1f)) {
            Text(t.title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    t.artist,
                    style = MaterialTheme.typography.bodySmall,
                    color = p.text,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.clickable { openCatalogArtist(nav, t.artistId, t.artist, scope) },
                )
                val rest = listOf(t.album, t.year).filter { it.isNotEmpty() }.joinToString(" · ")
                if (rest.isNotEmpty()) {
                    Text(" · $rest", style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            if (!t.inLibrary && status.isEmpty()) {
                Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    SmallButton("Song", iconRes = R.drawable.ic_add, primary = true, enabled = !busy) {
                        busy = true
                        scope.launch { submitRequest(nav.features, trackRequestBody(t, "song"))?.let { statuses.mark(keys, it) }; busy = false }
                    }
                    SmallButton("+ Album", iconRes = R.drawable.ic_disc, enabled = !busy) {
                        busy = true
                        scope.launch { submitRequest(nav.features, trackRequestBody(t, "album"))?.let { statuses.mark(keys, it) }; busy = false }
                    }
                }
            }
        }
        if (t.lengthSec > 0) {
            Text(formatDuration((t.lengthSec * 1000).toLong()), style = MaterialTheme.typography.bodySmall, color = p.faint, modifier = Modifier.padding(start = 8.dp))
        }
        if (t.inLibrary || status.isNotEmpty()) {
            Spacer(Modifier.width(8.dp))
            LiveAvailability(t.inLibrary, status, label)
        }
    }
}

// ---- Discover (web/js/pages/discover.js renderSearch) -----------------------------------------------------------

private val SEARCH_TYPES = listOf("all" to "Top results", "artist" to "Artists", "album" to "Albums", "track" to "Songs")

/**
 * The Discover tab: search MusicBrainz for artists, albums and songs; everything can be requested. Stacked with
 * [initialQuery] it opens as a page with a back arrow (the For you album tiles and library links use that).
 *
 * Every search also asks the library (GET /api/library/search, in parallel; the newest query wins, 0.31): Top
 * results start with "In your library" (artists, albums, songs), the Artists, Albums and Songs tabs with a short
 * strip of their kind, and when the catalogue has nothing the library's matches show instead of "No results".
 * Library items play or open the library's pages; catalogue items keep their Request buttons.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DiscoverScreen(nav: Nav, initialQuery: String = "", initialType: String = "all", stacked: Boolean = false) {
    val p = MaterialTheme.palette
    val features = nav.features
    var input by remember { mutableStateOf(initialQuery) }
    var q by remember { mutableStateOf(initialQuery) }
    var type by remember { mutableStateOf(SEARCH_TYPES.indexOfFirst { it.first == initialType }.coerceAtLeast(0)) }
    var results by remember { mutableStateOf<SearchResults?>(null) }
    var library by remember { mutableStateOf<LibrarySearch?>(null) }
    /** the query [library] answers (a newer one may be typed already) */
    var libraryFor by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    var generation by remember { mutableStateOf(0) }
    val statuses = rememberRequestStatuses()
    LiveRequestStatuses(nav, statuses)
    val liked by nav.graph.likes.liked.collectAsState()
    val nowPlaying by nav.player.nowPlaying.collectAsState()

    // typing: after 650 ms, three or more characters (the web's debounce); Search on the keyboard runs at once
    LaunchedEffect(input) {
        val v = input.trim()
        if (v.length > 2 && v != q) {
            delay(650)
            q = v
        }
    }
    // a song corrected or deleted from its ⋯: the library part asks again (the catalogue part stays)
    var libraryGen by remember { mutableStateOf(0) }
    OnSongFixed { libraryGen += 1 }
    // the library answers at once and shows first; the catalogue (MusicBrainz, Deezer, Apple) takes a moment
    LaunchedEffect(q, generation, libraryGen) {
        if (q.isEmpty()) {
            library = null
            libraryFor = ""
            return@LaunchedEffect
        }
        val lib = try {
            withContext(Dispatchers.IO) { nav.graph.api.search(q, 12) }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            null
        }
        library = lib
        libraryFor = q
    }
    LaunchedEffect(q, type, generation) {
        if (q.isEmpty()) {
            results = null
            error = null
            return@LaunchedEffect
        }
        busy = true
        try {
            results = withContext(Dispatchers.IO) { features.search(q, SEARCH_TYPES[type].first) }
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // (not the results of the query before)
            results = null
            error = e.message ?: "Search failed"
        } finally {
            busy = false
        }
    }

    Column(Modifier.fillMaxSize()) {
        if (stacked) {
            EncoreTopBar("Discover", onBack = nav.back)
        } else {
            PageTitle("Discover") { SmallButton("For you", iconRes = R.drawable.ic_spark) { nav.open(Screen.ForYou) } }
        }
        EncoreField(
            value = input,
            onValueChange = { input = it },
            placeholder = "Search MusicBrainz for artists, albums, songs…",
            leadingIcon = R.drawable.ic_search,
            imeAction = ImeAction.Search,
            onAction = { q = input.trim(); generation += 1 },
            modifier = Modifier.fillMaxWidth().padding(horizontal = Gutter),
        )
        ChoiceRow(SEARCH_TYPES.map { it.second }, type, onSelect = { type = it; q = input.trim() })
        val r = results
        val err = error
        // the library's matches of this tab's kind (all three on Top results)
        val lib = library?.takeIf { libraryFor == q }
        val libArtists = if (type == 0 || type == 1) lib?.artists.orEmpty().take(6) else emptyList()
        val libAlbums = if (type == 0 || type == 2) lib?.albums.orEmpty().take(6) else emptyList()
        val libSongs = when (type) { 0 -> lib?.songs.orEmpty().take(5); 3 -> lib?.songs.orEmpty().take(3); else -> emptyList() }
        val libHas = libArtists.isNotEmpty() || libAlbums.isNotEmpty() || libSongs.isNotEmpty()
        when {
            q.isEmpty() -> EmptyState(R.drawable.ic_search, "Search the entire MusicBrainz catalogue", "Everything you find can be requested with one click.")
            err != null && !libHas -> ErrorBox(err) { generation += 1 }
            busy && r == null && !libHas -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator(color = p.accent, trackColor = p.border) }
            r == null && !libHas -> Spacer(Modifier.height(1.dp))
            r != null && r.isEmpty && !libHas -> EmptyState(R.drawable.ic_search, "No results for “$q”")
            else -> LazyColumn(Modifier.fillMaxSize()) {
                if (busy) item { Row(Modifier.padding(horizontal = Gutter, vertical = 4.dp)) { Spinner() } }
                // what the library already has, first
                if (libHas) {
                    item { SectionHead("In your library") }
                    if (libArtists.isNotEmpty()) {
                        item {
                            LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                                items(libArtists, key = { "lib-artist-" + it.name }) { a ->
                                    Tile(
                                        title = a.shownName(),
                                        subtitle = "${a.albums} album${if (a.albums == 1) "" else "s"}",
                                        centered = true,
                                        modifier = Modifier.width(120.dp),
                                        onClick = { nav.open(Screen.Artist(a.name)) },
                                    ) { m ->
                                        Cover(if (a.coverAlbum.isNotEmpty()) nav.graph.api.coverUrl(a.coverAlbum, 400) else null, nav.graph.imageLoader, m, corner = 999.dp, label = a.name)
                                    }
                                }
                            }
                        }
                    }
                    if (libAlbums.isNotEmpty()) {
                        item {
                            LazyRow(contentPadding = PaddingValues(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                                items(libAlbums, key = { "lib-album-" + it.id }) { a ->
                                    AlbumCard(a, nav.graph.api.coverUrl(a.id, 400), nav.graph.imageLoader, Modifier.width(140.dp)) { nav.open(Screen.Album(a.id)) }
                                }
                            }
                        }
                    }
                    val songs = libSongs
                    items(songs.size, key = { i -> "lib-song-" + songs[i].id }) { i ->
                        val t = songs[i]
                        TrackRow(
                            track = t,
                            imageLoader = nav.graph.imageLoader,
                            coverUrl = if (t.albumId.isNotEmpty()) nav.graph.api.coverUrl(t.albumId, 96) else null,
                            liked = t.id in liked,
                            playing = nowPlaying?.id == t.id,
                            onClick = { nav.player.play(songs, i) },
                            onLike = { nav.graph.likes.toggle(t.id) },
                            subtitle = listOf(t.artist, t.album).filter { it.isNotEmpty() }.joinToString(" · "),
                            subtitleLinks = rememberSongLinks(nav, t),
                            onMore = { nav.trackMenu(t) },
                        )
                    }
                }
                if (r == null) {
                    // the catalogue is still searching, or failed
                    if (err != null) item { NoteText("Couldn't search the catalogue: $err", Modifier.padding(vertical = 8.dp)) }
                    item { Spacer(Modifier.height(24.dp)) }
                    return@LazyColumn
                }
                if (r.isEmpty && !busy) {
                    item { NoteText("Nothing in the catalogue matches “$q”", Modifier.padding(vertical = 12.dp)) }
                }
                val top = r.top
                if (top != null) {
                    item { SectionHead("Top result") }
                    item { TopResultCard(nav, top, statuses) }
                }
                if (r.tracks.isNotEmpty()) {
                    item { SectionHead("Songs") }
                    items(r.tracks.size, key = { i -> "track-" + r.tracks[i].key + "-" + i }) { i -> CatalogTrackRow(nav, r.tracks[i], statuses) }
                }
                if (r.albums.isNotEmpty()) {
                    item { SectionHead("Albums") }
                    tileRows(r.albums, 2, key = "albums") { a -> CatalogAlbumTile(nav, a, statuses) }
                }
                if (r.artists.isNotEmpty()) {
                    item { SectionHead("Artists") }
                    tileRows(r.artists.take(if (type == 0) 6 else 60), 3, key = "artists") { a -> CatalogArtistTile(nav, a) }
                }
                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}

/**
 * The Top result of a Discover search (discover.js `topResult`): a big cover, the kind, the name, "artist · on
 * album" for songs, and Play when it is in the library, else Request.
 */
@Composable
fun TopResultCard(nav: Nav, top: CatalogTop, statuses: RequestStatuses) {
    val p = MaterialTheme.palette
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    val t = top.track
    val a = top.album
    val ar = top.artist
    val kicker = when { t != null -> "Song"; a != null -> a.type.ifEmpty { "Album" }; else -> "Artist" }
    val title = t?.title ?: a?.title ?: ar?.name ?: ""
    val meta = when {
        t != null -> listOf(t.artist, if (t.album.isNotEmpty()) "on ${t.album}" else "", t.year).filter { it.isNotEmpty() }.joinToString(" · ")
        a != null -> listOf(a.artist, a.year).filter { it.isNotEmpty() }.joinToString(" · ")
        else -> listOfNotNull(ar?.type, ar?.country, ar?.disambiguation).filter { it.isNotEmpty() }.joinToString(" · ")
    }
    val open: () -> Unit = {
        when {
            t != null -> if (t.releaseGroupId.isNotEmpty()) nav.open(Screen.CatalogAlbum(t.releaseGroupId)) else openCatalogArtist(nav, t.artistId, t.artist, scope)
            a != null -> openCatalogAlbum(nav, a, scope)
            ar != null -> openCatalogArtist(nav, ar.id, ar.name, scope)
        }
    }
    CardBox(Modifier.padding(horizontal = Gutter).clickable(onClick = open)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            val art = t?.cover ?: a?.cover ?: ar?.image ?: ""
            if (ar != null) {
                Box(Modifier.size(96.dp).clip(CircleShape)) {
                    ArtPlaceholder(ar.name, Modifier.fillMaxSize())
                    if (art.isNotEmpty()) {
                        AsyncImage(model = art, contentDescription = null, imageLoader = nav.graph.imageLoader, modifier = Modifier.fillMaxSize(), contentScale = androidx.compose.ui.layout.ContentScale.Crop)
                    }
                }
            } else {
                Cover(art.ifEmpty { null }, nav.graph.imageLoader, size = 96.dp, corner = 12.dp, label = title)
            }
            Spacer(Modifier.width(14.dp))
            Column(Modifier.weight(1f)) {
                Text(kicker.uppercase(), style = MaterialTheme.typography.labelSmall, color = p.muted, fontWeight = FontWeight.SemiBold)
                Text(title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (meta.isNotEmpty()) {
                    Text(meta, style = MaterialTheme.typography.bodySmall, color = p.muted, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
                Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                    when {
                        t != null -> {
                            val keys = trackKeys(t)
                            val status = statuses.statusOf(keys, t.requestStatus)
                            val lib = t.library
                            when {
                                lib != null -> SmallButton("Play", iconRes = R.drawable.ic_play, primary = true) { nav.player.play(listOf(lib)) }
                                t.inLibrary || status.isNotEmpty() -> LiveAvailability(t.inLibrary, status, statuses.labelOf(keys, t.requestLabel))
                                else -> {
                                    SmallButton("Song", iconRes = R.drawable.ic_add, primary = true, enabled = !busy) {
                                        busy = true
                                        scope.launch { submitRequest(nav.features, trackRequestBody(t, "song"))?.let { statuses.mark(keys, it) }; busy = false }
                                    }
                                    SmallButton("+ Album", iconRes = R.drawable.ic_disc, enabled = !busy) {
                                        busy = true
                                        scope.launch { submitRequest(nav.features, trackRequestBody(t, "album"))?.let { statuses.mark(keys, it) }; busy = false }
                                    }
                                }
                            }
                        }
                        a != null -> {
                            val keys = albumKeys(a)
                            val status = statuses.statusOf(keys, a.requestStatus)
                            when {
                                a.libraryAlbumId.isNotEmpty() -> SmallButton("Play", iconRes = R.drawable.ic_play, primary = true) { playLibraryAlbum(nav, a.libraryAlbumId, scope) }
                                a.inLibrary || status.isNotEmpty() -> LiveAvailability(a.inLibrary, status, statuses.labelOf(keys, a.requestLabel))
                                else -> SmallButton("Request album", iconRes = R.drawable.ic_add, primary = true, enabled = !busy) {
                                    busy = true
                                    scope.launch { submitRequest(nav.features, albumRequestBody(a))?.let { statuses.mark(keys, it) }; busy = false }
                                }
                            }
                        }
                        ar != null -> {
                            SmallButton("Open artist", iconRes = R.drawable.ic_person, primary = true) { openCatalogArtist(nav, ar.id, ar.name, scope) }
                            if (ar.inLibrary) Badge("In library", kind = "ok")
                        }
                    }
                }
            }
        }
    }
}

/** A play icon over a cover for rows that can play (used by the catalogue pages). */
@Composable
fun PlayableCover(nav: Nav, url: String?, label: String, size: Dp, playable: Boolean, onPlay: () -> Unit) {
    Box(Modifier.size(size).clip(RoundedCornerShape(8.dp)).clickable(enabled = playable, onClick = onPlay)) {
        Cover(url, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 8.dp, label = label)
        if (playable) {
            Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.35f)), contentAlignment = Alignment.Center) {
                Icon(painterResource(R.drawable.ic_play), contentDescription = "Play", tint = Color.White, modifier = Modifier.size(20.dp))
            }
        }
    }
}

/** A centred muted line (the web's `.muted.small` notes under section heads). */
@Composable
fun NoteText(text: String, modifier: Modifier = Modifier, center: Boolean = false) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.palette.muted,
        textAlign = if (center) TextAlign.Center else null,
        modifier = modifier.fillMaxWidth().padding(horizontal = Gutter),
    )
}

/** An icon-only button with the web's ghost look (spark "More like this", search "Pick"…). */
@Composable
fun GhostIconButton(iconRes: Int, description: String, onClick: () -> Unit, tint: Color = Color.Unspecified) {
    IconButton(onClick = onClick, modifier = Modifier.size(36.dp)) {
        Icon(painterResource(iconRes), contentDescription = description, tint = if (tint != Color.Unspecified) tint else MaterialTheme.palette.muted, modifier = Modifier.size(18.dp))
    }
}

/** Where a loaded page draws a spinner until it is there. */
@Composable
fun LoadingBox() {
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        CircularProgressIndicator(color = MaterialTheme.palette.accent, trackColor = MaterialTheme.palette.border)
    }
}
