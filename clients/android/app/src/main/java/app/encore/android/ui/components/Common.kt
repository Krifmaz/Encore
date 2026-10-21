package app.encore.android.ui.components

import android.provider.Settings
import androidx.compose.foundation.background
import androidx.compose.foundation.basicMarquee
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.material3.TopAppBarScrollBehavior
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import app.encore.android.R
import app.encore.android.data.api.Album
import app.encore.android.data.api.CoverVersions
import app.encore.android.data.api.Track
import app.encore.android.features.library.SurroundRules
import app.encore.android.ui.theme.EncoreColors
import app.encore.android.ui.theme.palette
import coil3.ImageLoader
import coil3.compose.AsyncImage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.text.DateFormat
import java.util.Date
import java.util.Locale

/** The web's phone gutter (`.main { padding: 16px }`). */
val Gutter: Dp = 16.dp

// ---- scrolling titles ------------------------------------------------------------------------------------------

/** False when the phone's animations are off (Accessibility → Remove animations); titles then end in "…". */
@Composable
fun rememberMotionOn(): Boolean {
    val ctx = LocalContext.current
    return remember(ctx) { Settings.Global.getFloat(ctx.contentResolver, Settings.Global.ANIMATOR_DURATION_SCALE, 1f) != 0f }
}

/** A title too long for its line scrolls sideways, pausing at the start each time round; one that fits stays still. */
fun Modifier.scrollingTitle(on: Boolean): Modifier =
    if (on) basicMarquee(iterations = Int.MAX_VALUE, initialDelayMillis = 1500, repeatDelayMillis = 2500, velocity = 32.dp) else this

// ---- loading ---------------------------------------------------------------------------------------------------

/** What a screen loaded from the server. */
sealed interface Loadable<out T> {
    data object Loading : Loadable<Nothing>
    data class Ready<T>(val value: T) : Loadable<T>
    data class Failed(val message: String) : Loadable<Nothing>
}

/** A blocking API call run on the IO dispatcher when the screen opens (and again on retry). */
class Loader<T>(val state: MutableState<Loadable<T>>, private val retryKey: MutableState<Int>, private val quiet: MutableState<Boolean>) {
    fun retry() {
        quiet.value = false
        retryKey.value = retryKey.value + 1
    }

    /**
     * Loads again without the spinner (a song was corrected or deleted from its ⋯ menu): what is shown stays, and the
     * list keeps its scroll position, until the new answer replaces it.
     */
    fun refresh() {
        quiet.value = true
        retryKey.value = retryKey.value + 1
    }
}

@Composable
fun <T> rememberLoad(vararg keys: Any?, block: () -> T): Loader<T> {
    val state = remember(*keys) { mutableStateOf<Loadable<T>>(Loadable.Loading) }
    val retry = remember(*keys) { mutableStateOf(0) }
    val quiet = remember(*keys) { mutableStateOf(false) }
    LaunchedEffect(retry.value, *keys) {
        if (!quiet.value || state.value !is Loadable.Ready) state.value = Loadable.Loading
        quiet.value = false
        state.value = try {
            Loadable.Ready(withContext(Dispatchers.IO) { block() })
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            Loadable.Failed(e.message ?: "Something went wrong")
        }
    }
    return Loader(state, retry, quiet)
}

@Composable
fun <T> LoadableContent(loader: Loader<T>, content: @Composable (T) -> Unit) {
    when (val s = loader.state.value) {
        is Loadable.Loading -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            CircularProgressIndicator(color = MaterialTheme.palette.accent, trackColor = MaterialTheme.palette.border)
        }
        is Loadable.Failed -> ErrorBox(s.message) { loader.retry() }
        is Loadable.Ready -> content(s.value)
    }
}

/** The web's `.error-box` with a retry, centred in the empty page. */
@Composable
fun ErrorBox(message: String, onRetry: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(24.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box(
            Modifier
                .clip(RoundedCornerShape(10.dp))
                .background(EncoreColors.Err.copy(alpha = 0.12f))
                .border(1.dp, EncoreColors.Err.copy(alpha = 0.35f), RoundedCornerShape(10.dp))
                .padding(horizontal = 12.dp, vertical = 10.dp),
        ) {
            Text(message, style = MaterialTheme.typography.bodySmall, color = if (MaterialTheme.palette.dark) Color(0xFFFCA5A5) else Color(0xFFB91C1C))
        }
        TextButton(onClick = onRetry) { Text("Try again", color = MaterialTheme.palette.text) }
    }
}

/** The web's `.empty`: an icon, a title and an optional line below it. */
@Composable
fun EmptyState(iconRes: Int, title: String, text: String = "", modifier: Modifier = Modifier) {
    Column(
        modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 50.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(painterResource(iconRes), contentDescription = null, tint = MaterialTheme.palette.faint, modifier = Modifier.size(46.dp))
        Spacer(Modifier.height(10.dp))
        Text(title, style = MaterialTheme.typography.titleMedium, color = MaterialTheme.palette.muted, textAlign = TextAlign.Center)
        if (text.isNotEmpty()) {
            Text(
                text,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.palette.muted,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(top = 6.dp),
            )
        }
    }
}

// ---- artwork ---------------------------------------------------------------------------------------------------

/** The web's `initials()`: the first letter, skipping a leading "The". */
fun initialOf(label: String?): String {
    val s = (label ?: "").replace(Regex("^(?i)the\\s+"), "").trim()
    return if (s.isEmpty()) "?" else s.substring(0, 1).uppercase()
}

/** The web's `.ph` placeholder: a soft gradient with the first letter of the name. */
@Composable
fun ArtPlaceholder(label: String?, modifier: Modifier = Modifier, letterSize: Dp = 34.dp) {
    val p = MaterialTheme.palette
    Box(modifier.background(Brush.linearGradient(listOf(p.panel2, p.border))), contentAlignment = Alignment.Center) {
        if (label == null) {
            Icon(painterResource(R.drawable.ic_note), contentDescription = null, tint = p.faint, modifier = Modifier.size(letterSize))
        } else {
            Text(
                initialOf(label),
                color = p.faint,
                fontWeight = FontWeight.ExtraBold,
                fontSize = (letterSize.value * 0.9f).sp,
            )
        }
    }
}

/**
 * A cover (the web's `art()`): the image over a lettered placeholder, so something shows while it loads or when
 * there is none. `label` gives the placeholder its letter (an album or artist name); null shows a note.
 */
@Composable
fun Cover(
    url: String?,
    imageLoader: ImageLoader,
    modifier: Modifier = Modifier,
    size: Dp? = null,
    corner: Dp = 8.dp,
    label: String? = null,
) {
    val base = if (size != null) modifier.size(size) else modifier
    // the cover's main colour (the server's cover_color) while it loads; the image crossfades in over it
    val tint = remember(url) { CoverVersions.colorOfUrl(url) }
    Box(base.clip(RoundedCornerShape(corner)).background(if (tint != null) Color(tint) else MaterialTheme.palette.panel2), contentAlignment = Alignment.Center) {
        if (tint == null) ArtPlaceholder(label, Modifier.fillMaxSize(), letterSize = if (size != null && size < 56.dp) 18.dp else 34.dp)
        if (!url.isNullOrEmpty()) {
            AsyncImage(
                model = url,
                contentDescription = null,
                imageLoader = imageLoader,
                modifier = Modifier.fillMaxSize(),
                contentScale = ContentScale.Crop,
            )
        }
    }
}

/** A round cover (artists: the web's `.tile.round`). */
@Composable
fun RoundCover(url: String?, imageLoader: ImageLoader, size: Dp, label: String?, modifier: Modifier = Modifier) {
    val tint = remember(url) { CoverVersions.colorOfUrl(url) }
    Box(modifier.size(size).clip(CircleShape).background(if (tint != null) Color(tint) else MaterialTheme.palette.panel2), contentAlignment = Alignment.Center) {
        if (tint == null) ArtPlaceholder(label, Modifier.fillMaxSize(), letterSize = if (size < 56.dp) 18.dp else 34.dp)
        if (!url.isNullOrEmpty()) {
            AsyncImage(model = url, contentDescription = null, imageLoader = imageLoader, modifier = Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        }
    }
}

/** The web's `.collage`: four covers in a 2×2 grid inside one square (playlists, Made for you). */
@Composable
fun Collage(urls: List<String>, imageLoader: ImageLoader, modifier: Modifier = Modifier, corner: Dp = 12.dp, label: String? = null) {
    val cells = urls.take(4)
    Box(modifier.clip(RoundedCornerShape(corner)).background(MaterialTheme.palette.panel2)) {
        if (cells.size < 4) {
            Cover(cells.firstOrNull(), imageLoader, Modifier.fillMaxSize(), corner = 0.dp, label = label)
        } else {
            Column(Modifier.fillMaxSize()) {
                for (row in 0 until 2) {
                    Row(Modifier.weight(1f).fillMaxWidth()) {
                        for (col in 0 until 2) {
                            AsyncImage(
                                model = cells[row * 2 + col],
                                contentDescription = null,
                                imageLoader = imageLoader,
                                modifier = Modifier.weight(1f).fillMaxSize(),
                                contentScale = ContentScale.Crop,
                            )
                        }
                    }
                }
            }
        }
    }
}

/** The web's `.avatar`: the picture when there is one, else the first letter on the accent gradient. */
@Composable
fun Avatar(name: String, url: String?, imageLoader: ImageLoader, size: Dp = 32.dp, modifier: Modifier = Modifier) {
    val p = MaterialTheme.palette
    Box(modifier.size(size).clip(CircleShape).background(p.gradient), contentAlignment = Alignment.Center) {
        Text(
            (name.ifEmpty { "?" }).substring(0, 1).uppercase(),
            color = p.onAccent,
            fontWeight = FontWeight.Bold,
            fontSize = (size.value * 0.45f).sp,
        )
        if (!url.isNullOrEmpty()) {
            AsyncImage(model = url, contentDescription = null, imageLoader = imageLoader, modifier = Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        }
    }
}

// ---- headings, badges, tabs ------------------------------------------------------------------------------------

/** A plain section heading (h2) with the page gutter. Kept for callers that only need the title. */
@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(
        text,
        style = MaterialTheme.typography.titleLarge,
        modifier = modifier.padding(start = Gutter, end = Gutter, top = 24.dp, bottom = 12.dp),
    )
}

/** The web's `.section-head`: an h2 (with an optional line under it) and an optional small button on the right. */
@Composable
fun SectionHead(title: String, subtitle: String? = null, modifier: Modifier = Modifier, action: String? = null, onAction: (() -> Unit)? = null) {
    Row(
        modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 24.dp, bottom = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleLarge)
            if (!subtitle.isNullOrEmpty()) {
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted)
            }
        }
        if (action != null && onAction != null) SmallButton(action, onClick = onAction)
    }
}

/** The web's `.badge` pill. `kind`: "" (muted), "ok", "warn", "err", "info", "accent". */
@Composable
fun Badge(text: String, kind: String = "", modifier: Modifier = Modifier, onClick: (() -> Unit)? = null) {
    val p = MaterialTheme.palette
    val tone: Color? = when (kind) {
        "ok" -> EncoreColors.Ok
        "warn" -> EncoreColors.Warn
        "err" -> EncoreColors.Err
        "info" -> EncoreColors.Info
        else -> null
    }
    val fg: Color = if (kind == "accent") p.text else tone ?: p.muted
    val edge: Color = if (kind == "accent") p.accent.copy(alpha = 0.4f) else tone?.copy(alpha = 0.35f) ?: p.border
    val bg: Color = if (kind == "accent") p.accent.copy(alpha = 0.14f) else tone?.copy(alpha = 0.10f) ?: p.panel2
    val shape = RoundedCornerShape(99.dp)
    var m = modifier.clip(shape).background(bg).border(1.dp, edge, shape)
    if (onClick != null) m = m.clickable(onClick = onClick)
    Box(m.padding(horizontal = 9.dp, vertical = 2.dp)) {
        Text(text, style = MaterialTheme.typography.labelSmall, color = fg, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** The web's `.btn.sm`: a small bordered button on the panel colour (`danger`: the web's `.btn.sm.danger`, in red). */
@Composable
fun SmallButton(label: String, modifier: Modifier = Modifier, iconRes: Int? = null, primary: Boolean = false, enabled: Boolean = true, danger: Boolean = false, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(8.dp)
    val bg = when {
        primary -> Modifier.background(p.gradient)
        danger -> Modifier.background(EncoreColors.Err.copy(alpha = 0.10f)).border(1.dp, EncoreColors.Err.copy(alpha = 0.45f), shape)
        else -> Modifier.background(p.panel2).border(1.dp, p.border, shape)
    }
    Row(
        modifier
            .clip(shape)
            .then(bg)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 11.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        val fg = if (primary) p.onAccent else if (danger) EncoreColors.Err else p.text
        if (iconRes != null) Icon(painterResource(iconRes), contentDescription = null, tint = fg, modifier = Modifier.size(16.dp))
        Text(label, style = MaterialTheme.typography.labelMedium, color = fg)
    }
}

/** The web's `.btn` / `.btn.primary`: the page's main actions (Play, Shuffle, Import…). */
@Composable
fun EncoreButton(label: String, modifier: Modifier = Modifier, iconRes: Int? = null, primary: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    val shape = RoundedCornerShape(10.dp)
    val bg = if (primary) Modifier.background(p.gradient) else Modifier.background(p.panel2).border(1.dp, p.border, shape)
    val alpha = if (enabled) 1f else 0.5f
    Row(
        modifier
            .clip(shape)
            .then(bg)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val fg = (if (primary) p.onAccent else p.text).copy(alpha = alpha)
        if (iconRes != null) Icon(painterResource(iconRes), contentDescription = null, tint = fg, modifier = Modifier.size(16.dp))
        Text(label, style = MaterialTheme.typography.labelLarge, color = fg)
    }
}

/**
 * A row of choices that look like the web's `.tabs` (Albums / Artists / Songs); scrolls when they don't fit.
 * [end]: the space after it (less when something sits beside it, like the library's Sort chip). [busy]: the choices
 * that show a small spinner after their label (the picker's tabs while their source searches).
 */
@Composable
fun ChoiceRow(options: List<String>, selected: Int, onSelect: (Int) -> Unit, modifier: Modifier = Modifier, end: Dp = Gutter, busy: Set<Int> = emptySet()) {
    val p = MaterialTheme.palette
    val outer = RoundedCornerShape(12.dp)
    Row(modifier.padding(start = Gutter, end = end, top = 8.dp, bottom = 8.dp).horizontalScroll(rememberScrollState())) {
        Row(
            Modifier.clip(outer).background(p.panel).border(1.dp, p.border, outer).padding(4.dp),
            horizontalArrangement = Arrangement.spacedBy(4.dp),
        ) {
            options.forEachIndexed { i, label ->
                val on = i == selected
                Box(
                    Modifier
                        .clip(RoundedCornerShape(8.dp))
                        .background(if (on) p.panel2 else Color.Transparent)
                        .clickable { onSelect(i) }
                        .padding(horizontal = 14.dp, vertical = 7.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(label, style = MaterialTheme.typography.labelLarge, color = if (on) p.text else p.muted, maxLines = 1)
                        if (i in busy) CircularProgressIndicator(color = p.accent, trackColor = p.border, strokeWidth = 1.5.dp, modifier = Modifier.size(12.dp))
                    }
                }
            }
        }
    }
}

// ---- tiles and rows --------------------------------------------------------------------------------------------

/** The web's `.tile`: square art with 12px corners, a 650-weight title and a muted subtitle; [onLongClick] (0.36) picks it. */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
fun Tile(
    title: String,
    subtitle: String,
    modifier: Modifier = Modifier,
    subtitle2: String? = null,
    centered: Boolean = false,
    onLongClick: (() -> Unit)? = null,
    onClick: () -> Unit,
    art: @Composable (Modifier) -> Unit,
) {
    Column(
        modifier.clip(RoundedCornerShape(12.dp)).then(
            if (onLongClick != null) Modifier.combinedClickable(onLongClick = onLongClick, onClick = onClick) else Modifier.clickable(onClick = onClick),
        ),
        horizontalAlignment = if (centered) Alignment.CenterHorizontally else Alignment.Start,
    ) {
        art(Modifier.fillMaxWidth().aspectRatio(1f))
        Spacer(Modifier.height(9.dp))
        Text(
            title,
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            textAlign = if (centered) TextAlign.Center else null,
            modifier = Modifier.fillMaxWidth(),
        )
        Text(
            subtitle,
            style = MaterialTheme.typography.bodySmall.copy(fontSize = 13.sp),
            color = MaterialTheme.palette.muted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            textAlign = if (centered) TextAlign.Center else null,
            modifier = Modifier.fillMaxWidth(),
        )
        if (!subtitle2.isNullOrEmpty()) {
            Text(
                subtitle2,
                style = MaterialTheme.typography.bodySmall.copy(fontSize = 13.sp),
                color = MaterialTheme.palette.faint,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                textAlign = if (centered) TextAlign.Center else null,
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

/**
 * An album tile (the web's `libraryTile`): cover, title, "artist · year". A Surround album (0.35) names its format in
 * the corner: Dolby Atmos, E-AC-3 5.1 …
 */
@Composable
fun AlbumCard(album: Album, coverUrl: String, imageLoader: ImageLoader, modifier: Modifier = Modifier, onLongClick: (() -> Unit)? = null, onClick: () -> Unit) {
    val corner = if (album.isSurround) SurroundRules.badge(album.atmos, album.quality) else ""
    Tile(
        title = album.shownTitle(),
        subtitle = listOf(album.shownArtist(), album.year).filter { it.isNotEmpty() }.joinToString(" · "),
        subtitle2 = album.titleUnder(),
        modifier = modifier,
        onLongClick = onLongClick,
        onClick = onClick,
    ) { m ->
        if (corner.isEmpty()) {
            Cover(coverUrl, imageLoader, m, corner = 12.dp, label = album.title)
        } else {
            Box(m) {
                Cover(coverUrl, imageLoader, Modifier.fillMaxSize(), corner = 12.dp, label = album.title)
                Badge(corner, kind = "accent", modifier = Modifier.align(Alignment.TopEnd).padding(8.dp))
            }
        }
    }
}

/**
 * One song in a list (the web's `.list-item`): 48dp art (or the track number), a 600-weight title, a muted
 * subtitle, the duration, the heart that fills when liked, and an optional "⋯".
 */
@Composable
fun TrackRow(
    track: Track,
    imageLoader: ImageLoader?,
    coverUrl: String?,
    liked: Boolean,
    playing: Boolean,
    onClick: () -> Unit,
    /** null: no heart (Surround songs have no likes) */
    onLike: (() -> Unit)?,
    number: Int? = null,
    subtitle: String? = null,
    onMore: (() -> Unit)? = null,
    showDuration: Boolean = true,
    note: String? = null,
    /** the subtitle with tappable artist / album names (ui/screens/ArtistLinks.kt); wins over [subtitle] */
    subtitleLinks: AnnotatedString? = null,
    /** a small badge on the line under the subtitle, before [note] (the album page's audio check verdict) */
    badge: (@Composable () -> Unit)? = null,
) {
    val p = MaterialTheme.palette
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(start = Gutter, end = 4.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (imageLoader != null) {
            Cover(coverUrl, imageLoader, size = 48.dp, corner = 8.dp, label = track.album.ifEmpty { track.title })
            Spacer(Modifier.width(14.dp))
        } else if (number != null) {
            Text(
                if (number > 0) number.toString() else "–",
                style = MaterialTheme.typography.bodyMedium,
                color = if (playing) p.accent else p.faint,
                textAlign = TextAlign.End,
                modifier = Modifier.width(28.dp),
            )
            Spacer(Modifier.width(14.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(
                track.shownTitle(),
                style = MaterialTheme.typography.bodyMedium,
                fontWeight = FontWeight.SemiBold,
                color = if (playing) p.accent else p.text,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            // Settings → Language: the translation under the original
            track.titleUnder()?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            if (subtitleLinks != null) {
                Text(
                    subtitleLinks,
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            } else {
                Text(
                    subtitle ?: track.shownArtist(),
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            if (badge != null) {
                Row(Modifier.padding(top = 3.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    badge()
                    if (!note.isNullOrEmpty()) {
                        Text(note, style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                    }
                }
            } else if (!note.isNullOrEmpty()) {
                Text(note, style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (showDuration) {
            Text(
                formatDuration((track.durationSec * 1000).toLong()),
                style = MaterialTheme.typography.bodySmall,
                color = p.muted,
                modifier = Modifier.padding(start = 8.dp),
            )
        }
        if (onLike != null) LikeButton(liked, onLike)
        if (onMore != null) {
            IconButton(onClick = onMore) {
                Icon(painterResource(R.drawable.ic_more_vert), contentDescription = "More", tint = p.muted)
            }
        }
    }
}

/** The heart (the web's `.btn.like`): outlined, and filled in the accent colour when liked. Buzzes on tap. */
@Composable
fun LikeButton(liked: Boolean, onClick: () -> Unit, tint: Color = Color.Unspecified) {
    val haptics = LocalHapticFeedback.current
    val p = MaterialTheme.palette
    IconButton(onClick = {
        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
        onClick()
    }) {
        Icon(
            painterResource(if (liked) R.drawable.ic_favorite else R.drawable.ic_favorite_border),
            contentDescription = if (liked) "Liked – tap to remove" else "Like",
            tint = if (tint != Color.Unspecified) tint else if (liked) p.accent2 else p.muted,
        )
    }
}

/** A thin line between list rows (the web's `.list-item` border). */
@Composable
fun RowDivider(modifier: Modifier = Modifier, startIndent: Dp = Gutter) {
    Box(modifier.fillMaxWidth().padding(start = startIndent).height(1.dp).background(MaterialTheme.palette.border))
}

// ---- menus -----------------------------------------------------------------------------------------------------

/**
 * One line of a "⋯" menu (the web's `popMenu`). A null `onClick` draws a separator; `badge` is a count after the label.
 * [info] (0.36): a muted line of text, not an action (the song ⋯ menu's "Requested by alex on Oct 13, 2026").
 */
class MenuEntry(val label: String, val iconRes: Int? = null, val danger: Boolean = false, val badge: String? = null, val info: Boolean = false, val onClick: (() -> Unit)?) {
    /** A line that runs something when tapped (not a separator, not an info line). */
    val isAction: Boolean get() = !info && onClick != null
}

/** A separator for [PopMenu]. */
val MenuSeparator: MenuEntry = MenuEntry("-", onClick = null)

/** The web's `.menu-pop`: a panel of actions anchored to its "⋯" button. */
@Composable
fun PopMenu(expanded: Boolean, onDismiss: () -> Unit, entries: List<MenuEntry?>) {
    val p = MaterialTheme.palette
    DropdownMenu(
        expanded = expanded,
        onDismissRequest = onDismiss,
        shape = RoundedCornerShape(12.dp),
        containerColor = p.panel,
    ) {
        for (e in entries) {
            if (e == null) continue
            if (e.info) {
                // text, not an action: nothing to tap
                Text(
                    e.label,
                    style = MaterialTheme.typography.bodySmall,
                    color = p.muted,
                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                )
                continue
            }
            if (e.onClick == null) {
                Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).height(1.dp).background(p.border))
                continue
            }
            val fg = if (e.danger) EncoreColors.Err else p.text
            DropdownMenuItem(
                text = {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(e.label, color = fg, style = MaterialTheme.typography.bodyLarge)
                        if (!e.badge.isNullOrEmpty()) Badge(e.badge, kind = "warn")
                    }
                },
                onClick = { onDismiss(); e.onClick.invoke() },
                leadingIcon = if (e.iconRes != null) {
                    { Icon(painterResource(e.iconRes), contentDescription = null, tint = fg.copy(alpha = 0.85f), modifier = Modifier.size(18.dp)) }
                } else null,
            )
        }
    }
}

/** A "⋯" button with its menu. */
@Composable
fun MoreButton(entries: List<MenuEntry?>, modifier: Modifier = Modifier, tint: Color = Color.Unspecified, iconSize: Dp = 24.dp, onOpen: () -> Unit = {}) {
    var open by remember { mutableStateOf(false) }
    Box(modifier) {
        IconButton(onClick = { open = true; onOpen() }) {
            Icon(
                painterResource(R.drawable.ic_more_vert),
                contentDescription = "More",
                tint = if (tint != Color.Unspecified) tint else MaterialTheme.palette.muted,
                modifier = Modifier.size(iconSize),
            )
        }
        PopMenu(open, { open = false }, entries)
    }
}

// ---- top bars --------------------------------------------------------------------------------------------------

/**
 * A top app bar that scrolls away with the content. Pass the [scrollBehavior] to the bar and hang
 * `Modifier.nestedScroll(scrollBehavior.nestedScrollConnection)` on the scrolling container (or the Scaffold).
 * Status-bar insets are not added here: the root Scaffold already keeps everything below the status bar.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun EncoreTopBar(
    title: String,
    scrollBehavior: TopAppBarScrollBehavior? = null,
    onBack: (() -> Unit)? = null,
    actions: @Composable () -> Unit = {},
) {
    val p = MaterialTheme.palette
    TopAppBar(
        title = { Text(title, style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        navigationIcon = {
            if (onBack != null) {
                IconButton(onClick = onBack) { Icon(painterResource(R.drawable.ic_arrow_back), contentDescription = "Back") }
            }
        },
        actions = { actions() },
        windowInsets = WindowInsets(0, 0, 0, 0),
        colors = TopAppBarDefaults.topAppBarColors(
            containerColor = p.bg,
            scrolledContainerColor = p.bg,
            titleContentColor = p.text,
            navigationIconContentColor = p.text,
            actionIconContentColor = p.text,
        ),
        scrollBehavior = scrollBehavior,
    )
}

/** The web's `h1` page title row with an optional line under it, as the first item of a scrolling list. */
@Composable
fun PageTitle(title: String, subtitle: String? = null, modifier: Modifier = Modifier, trailing: @Composable () -> Unit = {}) {
    Row(modifier.fillMaxWidth().padding(start = Gutter, end = Gutter, top = 12.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.headlineMedium)
            if (!subtitle.isNullOrEmpty()) {
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.palette.muted, modifier = Modifier.padding(top = 2.dp))
            }
        }
        trailing()
    }
}

// ---- formatting (web/js/ui.js `fmt`) ----------------------------------------------------------------------------

/** `fmt.dur`: m:ss or h:mm:ss; "" for nothing. */
fun formatDuration(ms: Long): String {
    if (ms <= 0L) return ""
    val total = ms / 1000L
    val h = total / 3600L
    val m = (total % 3600L) / 60L
    val s = total % 60L
    return if (h > 0) "%d:%02d:%02d".format(h, m, s) else "%d:%02d".format(m, s)
}

/** `fmt.longDur`: "4 mo 22 days 8 hr 24 min" – years, months (30 days), days, hours, minutes; zero parts left out. */
fun formatLongDuration(seconds: Double): String {
    var m = Math.round(maxOf(0.0, seconds) / 60.0)
    val parts = ArrayList<String>()
    for ((n, one, many) in listOf(Triple(525600L, "yr", "yr"), Triple(43200L, "mo", "mo"), Triple(1440L, "day", "days"), Triple(60L, "hr", "hr"))) {
        val v = m / n
        if (v > 0) { parts.add("$v ${if (v == 1L) one else many}"); m -= v * n }
    }
    if (m > 0 || parts.isEmpty()) parts.add("$m min")
    return parts.joinToString(" ")
}

/** `fmt.size`: "0 B", "12 KB", "1.5 GB". */
fun formatSize(bytes: Long): String {
    if (bytes <= 0L) return "0 B"
    val units = arrayOf("B", "KB", "MB", "GB", "TB")
    val i = minOf((Math.log(bytes.toDouble()) / Math.log(1024.0)).toInt(), 4)
    val v = bytes / Math.pow(1024.0, i.toDouble())
    return if (i > 1) "%.1f %s".format(Locale.US, v, units[i]) else "%.0f %s".format(Locale.US, v, units[i])
}

/** `fmt.day`: "Oct 13, 2026" in the phone's language; "—" for nothing. */
fun formatDay(epochSeconds: Double): String {
    if (epochSeconds <= 0.0) return "—"
    return DateFormat.getDateInstance(DateFormat.MEDIUM).format(Date((epochSeconds * 1000).toLong()))
}

/** `fmt.num`: 12,345. */
fun formatNumber(n: Long): String = java.text.NumberFormat.getIntegerInstance().format(n)
