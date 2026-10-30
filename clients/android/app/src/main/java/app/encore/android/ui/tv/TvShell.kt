/**
 * The TV layout's frame around the pages: the whole app zoomed out ([TvScaled]) and, in place of the phone's stack of
 * bars at the bottom (pick-up bar, mini player, tabs), one slim menu down the left edge ([TvRail]) that a remote opens
 * by pressing Left at the page's left edge (or Menu).
 */
package app.encore.android.ui.tv

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusRestorer
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.nativeKeyCode
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Track
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Cover
import app.encore.android.ui.theme.palette
import kotlinx.coroutines.delay

/**
 * Draws [content] zoomed out by Settings → TV → Size ([TvZoom.scale]) while the TV layout is on, and as it is
 * otherwise: the screen's density is scaled, so every page, sheet and menu inside fits more on the TV at once without
 * any page knowing about it.
 *
 * @param tv whether the TV layout is on ([rememberTvLayout]).
 */
@Composable
fun TvScaled(tv: Boolean, content: @Composable () -> Unit) {
    if (!tv) {
        content()
        return
    }
    val zoom by TvPrefs.zoom.collectAsState()
    val d = LocalDensity.current
    // sheets, menus and dialogs made inside take the same density, so they zoom out with the page
    CompositionLocalProvider(LocalDensity provides Density(d.density * zoom.scale, d.fontScale)) { content() }
}

/**
 * The TV frame: the page fills the screen right of the closed menu, the bars that matter on a TV (a group listen,
 * "Controlling <device>", "Continue … from another device") sit above it, and [TvRail] lies over its left edge.
 *
 * How the remote moves around it:
 * - the menu is closed, and its entries can't take the focus, until Left is pressed with nothing further left on the
 *   page (or the remote's Menu button): then it opens with the focus on the marked entry. A press of OK on the page
 *   never opens it, and the focus can't fall into it by itself when a page changes.
 * - Back or Right closes the open menu and puts the focus back where it was on the page; choosing an entry closes it
 *   too and puts the focus on the new page.
 * - Up / Down always move the focus to the next control above / below ([TvRules.verticalMove]), so a text field (the
 *   device name, a search box) no longer keeps them for its cursor and traps the focus above what is under it.
 *
 * @param nav moves around (the menu's Now playing entry shows the song's cover).
 * @param selected the menu entry for what is on screen (null: a page the menu has no entry for, e.g. an album).
 * @param pageKey changes whenever another page is shown, so the focus is put on the new page.
 * @param onSelect goes to an entry's place.
 * @param topBar the bars above the page (nothing when none of them has something to say).
 * @param page the page on screen.
 */
@Composable
fun TvShell(
    nav: Nav,
    selected: RailItem?,
    pageKey: Any?,
    onSelect: (RailItem) -> Unit,
    topBar: @Composable () -> Unit,
    page: @Composable () -> Unit,
) {
    val p = MaterialTheme.palette
    val focusManager = LocalFocusManager.current
    var railOpen by remember { mutableStateOf(false) }
    val pageFocus = remember { FocusRequester() }
    val railFocus = remember { FocusRequester() }
    // puts the focus back on the page (where it was, through the page's focusRestorer), then closes the menu: in this
    // order, so the focus never sits on an entry that just stopped taking it
    val closeRail: () -> Unit = {
        tryFocus(pageFocus)
        railOpen = false
    }
    // the menu's entries take the focus once they are focusable again, i.e. after the opening is composed
    LaunchedEffect(railOpen) { if (railOpen) tryFocus(railFocus) }
    // a new page: the focus goes onto it (after a moment, so its first controls are laid out)
    LaunchedEffect(pageKey) {
        delay(PAGE_FOCUS_DELAY_MS)
        if (!railOpen) tryFocus(pageFocus)
    }
    // Back closes the open menu first; composed after the app's own Back handling, so it is asked first
    BackHandler(enabled = railOpen) { closeRail() }

    Box(
        Modifier
            .fillMaxSize()
            .background(p.bg)
            .onPreviewKeyEvent { e ->
                if (e.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
                val dir = TvRules.verticalMove(e.key.nativeKeyCode) ?: return@onPreviewKeyEvent false
                // moved: done; nothing there (the top or the bottom): the page may still scroll on its own
                focusManager.moveFocus(if (dir) FocusDirection.Down else FocusDirection.Up)
            },
    ) {
        Column(Modifier.fillMaxSize().padding(start = TvRules.RAIL_CLOSED.dp)) {
            topBar()
            Box(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .onKeyEvent { e ->
                        // reached only when nothing on the page used the key (a text field keeps Left for its cursor)
                        if (e.type != KeyEventType.KeyDown) return@onKeyEvent false
                        when (TvRules.railKey(e.key.nativeKeyCode)) {
                            RailKey.MENU -> { railOpen = true; true }
                            RailKey.LEFT -> {
                                // Left moves on the page while there is something to the left; at its left edge it opens the menu
                                if (!focusManager.moveFocus(FocusDirection.Left)) railOpen = true
                                true
                            }
                            else -> false
                        }
                    }
                    .focusRequester(pageFocus)
                    .focusRestorer(),
            ) { page() }
        }
        TvRail(
            nav = nav,
            selected = selected,
            open = railOpen,
            focus = railFocus,
            onClose = closeRail,
            onSelect = { item ->
                railOpen = false
                onSelect(item)
            },
        )
    }
}

/** How long a new page has to lay out its first controls before the focus is put on it, in ms. */
private const val PAGE_FOCUS_DELAY_MS = 150L

/** Asks [r] for the focus; a requester not attached yet (nothing laid out) is left alone. */
private fun tryFocus(r: FocusRequester) {
    try { r.requestFocus() } catch (_: IllegalStateException) { }
}

/**
 * The TV side menu: a slim column of icons down the left edge (Search, Home, Library, Playlists, For you, Now playing
 * while a song is loaded, and Settings apart at the bottom, see [TvRules.railItems]). Closed, its entries can't take
 * the focus; [open], it widens over the page with each entry's word beside its icon, dims the page, and the focus
 * starts on the marked entry. Right closes it.
 *
 * @param selected the entry for what is on screen, marked even while the menu is closed.
 * @param open whether the menu is open.
 * @param focus put on the entry the focus starts on when the menu opens.
 * @param onClose closes the menu, the focus going back to the page.
 * @param onSelect goes to an entry's place (and closes the menu).
 */
@Composable
private fun TvRail(nav: Nav, selected: RailItem?, open: Boolean, focus: FocusRequester, onClose: () -> Unit, onSelect: (RailItem) -> Unit) {
    val p = MaterialTheme.palette
    val track by nav.player.nowPlaying.collectAsState()
    val width by animateDpAsState(if (open) TvRules.RAIL_OPEN.dp else TvRules.RAIL_CLOSED.dp, tween(180), label = "rail")
    val items = TvRules.railItems(playing = track != null)
    // the entry the focus starts on: the marked one, else the first
    val start = selected?.takeIf { it in items || it == RailItem.SETTINGS } ?: items.first()
    Box(Modifier.fillMaxSize()) {
        // the page dims while the menu is open, so the eye goes to the menu
        AnimatedVisibility(open, enter = fadeIn(tween(180)), exit = fadeOut(tween(180))) {
            Box(Modifier.fillMaxSize().background(p.bg.copy(alpha = 0.6f)))
        }
        Column(
            Modifier
                .width(width)
                .fillMaxHeight()
                .background(Brush.horizontalGradient(listOf(p.bg2, p.bg2.copy(alpha = if (open) 0.98f else 0.92f))))
                .onPreviewKeyEvent { e ->
                    // Right leaves the menu for the page
                    if (open && e.type == KeyEventType.KeyDown && TvRules.railKey(e.key.nativeKeyCode) == RailKey.RIGHT) {
                        onClose()
                        true
                    } else false
                }
                .padding(horizontal = 10.dp, vertical = 20.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            for (item in items) {
                RailEntry(item, item == selected, open, track, nav, if (item == start) focus else null) { onSelect(item) }
            }
            Spacer(Modifier.weight(1f))
            RailEntry(RailItem.SETTINGS, selected == RailItem.SETTINGS, open, track, nav, if (start == RailItem.SETTINGS) focus else null) { onSelect(RailItem.SETTINGS) }
        }
    }
}

/**
 * One entry of the TV side menu: its icon (the song's cover for Now playing) and, while the menu is open, its word.
 * The marked entry (the place on screen) has a filled background and an accent icon; the focused one gets the app's
 * focus ring from its clickable.
 *
 * @param item the entry.
 * @param on whether it is the place on screen.
 * @param open whether the menu is open (the word shows).
 * @param track the song loaded, for Now playing's cover.
 * @param focus attached when this is the entry the focus starts on as the menu opens.
 */
@Composable
private fun RailEntry(item: RailItem, on: Boolean, open: Boolean, track: Track?, nav: Nav, focus: FocusRequester?, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Row(
        Modifier
            .fillMaxWidth()
            .height(52.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(if (on) p.panel2 else p.bg2.copy(alpha = 0f))
            // closed, the menu can't take the focus: OK on the page or a page change never lands here
            .focusProperties { canFocus = open }
            .then(if (focus != null) Modifier.focusRequester(focus) else Modifier)
            .clickable(onClick = onClick)
            .padding(horizontal = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val t = track
        if (item == RailItem.NOW_PLAYING && t != null) {
            Cover(
                if (t.albumId.isNotEmpty()) nav.graph.api.coverUrl(t.albumId, 96) else null,
                nav.graph.imageLoader,
                size = 28.dp,
                corner = 6.dp,
                label = t.album.ifEmpty { t.title },
            )
        } else {
            Icon(painterResource(iconOf(item)), contentDescription = if (open) null else item.label, tint = if (on) p.accent else p.muted, modifier = Modifier.size(28.dp))
        }
        if (open) {
            Spacer(Modifier.width(16.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    item.label,
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal,
                    color = if (on) p.text else p.muted,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                // Now playing names the song under its word
                if (item == RailItem.NOW_PLAYING && t != null) {
                    Text(t.title, style = MaterialTheme.typography.bodySmall, color = p.faint, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

/** The icon of a side menu entry (the phone's tab icons for the tabs). */
private fun iconOf(item: RailItem): Int = when (item) {
    RailItem.SEARCH -> R.drawable.ic_search
    RailItem.HOME -> R.drawable.ic_home
    RailItem.LIBRARY -> R.drawable.ic_library
    RailItem.PLAYLISTS -> R.drawable.ic_queue_music
    RailItem.DOWNLOADS -> R.drawable.ic_download
    RailItem.FOR_YOU -> R.drawable.ic_spark
    RailItem.NOW_PLAYING -> R.drawable.ic_play
    RailItem.SETTINGS -> R.drawable.ic_settings
}
