/**
 * The TV player: Now playing laid out for a TV across the room and a remote. The controls (cover, song, seek bar,
 * buttons) take one side, chosen in Settings → TV; the other side is a panel that switches between the lyrics and the
 * Up next queue. Built from the same pieces as the phone's full player (PlayerScreens.kt) so both stay alike.
 */
package app.encore.android.ui.tv

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.blur
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalLayoutDirection
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import app.encore.android.R
import app.encore.android.data.api.Track
import app.encore.android.features.group.GroupListen
import app.encore.android.features.group.GroupPill
import app.encore.android.features.library.AlbumTypesRules
import app.encore.android.features.offline.OfflineMode
import app.encore.android.features.offline.rememberOfflineMenu
import app.encore.android.remote.DevicesButton
import app.encore.android.remote.LinkedVolume
import app.encore.android.ui.Nav
import app.encore.android.ui.components.Cover
import app.encore.android.ui.components.PlayerMenuActions
import app.encore.android.ui.components.SmallButton
import app.encore.android.ui.components.formatDay
import app.encore.android.ui.components.formatDuration
import app.encore.android.ui.screens.LyricsLoad
import app.encore.android.ui.screens.LyricsPane
import app.encore.android.ui.screens.PlayerActions
import app.encore.android.ui.screens.PlayerButtons
import app.encore.android.ui.screens.PlayerMeta
import app.encore.android.ui.screens.UpNextList
import app.encore.android.ui.screens.rememberLyrics
import app.encore.android.ui.theme.focusRing
import app.encore.android.ui.theme.palette
import coil3.compose.AsyncImage
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext

/** The most the TV player's cover grows to: past this the song's lines and buttons would be pushed off the screen. */
private val MAX_ART = 360.dp

/** The least the TV player's cover shrinks to, so it still reads as the album's art. */
private val MIN_ART = 96.dp

/** What the controls half needs under the cover: the song's lines, the seek bar, the buttons and the actions. */
private val UNDER_ART = 300.dp

/**
 * The full-screen TV player (the phone's [app.encore.android.ui.screens.FullPlayer] on a TV): the cover blurred as a
 * backdrop, a top bar (close, "Now playing", group, devices), then the controls half and the switchable panel side by
 * side, in the order Settings → TV → Player side gives ([TvPrefs.side]).
 *
 * The remote's focus starts on play / pause. Left / Right on the seek bar seek 10 s ([TvSeek]); Back closes the player
 * (the root's Back handling). Screen off (top right) blacks the screen out while the music plays on
 * ([ScreenOffLayer]), and turns on by itself after the remote sits untouched for Settings → TV → When idle
 * ([TvIdleOff], never by default), fully black or dimmed ([TvIdleStyle]).
 *
 * @param nav moves around and plays.
 * @param actions the player's ⋯ menu actions (Go to artist, View album, Share, Signal path…).
 * @param onClose closes the player (also before a link opens a page).
 */
@Composable
fun TvPlayer(nav: Nav, actions: PlayerMenuActions, onClose: () -> Unit) {
    val t by nav.player.nowPlaying.collectAsState()
    val track = t ?: return
    val p = MaterialTheme.palette
    val link by nav.player.remote.link.collectAsState()
    val side by TvPrefs.side.collectAsState()
    val panel by TvPrefs.panel.collectAsState()
    val rtl = LocalLayoutDirection.current == LayoutDirection.Rtl
    val coverUrl = if (track.albumId.isNotEmpty()) nav.graph.api.coverUrl(track.albumId, 1000) else null
    // the lyrics load while their half shows them; the player keeps them, so switching back and forth is instant
    val lyrics = rememberLyrics(nav, track.id, active = panel == SidePanel.LYRICS)
    val playFocus = remember { FocusRequester() }
    // Screen off: the screen black while the music plays on (TvScreenOff.kt); kept if the activity is made again
    var screenOff by rememberSaveable { mutableStateOf(false) }
    // the remote starts on play / pause, the one button most presses are for: when the player opens, and again when
    // the screen comes back on
    LaunchedEffect(screenOff) {
        if (screenOff) return@LaunchedEffect
        try { playFocus.requestFocus() } catch (_: IllegalStateException) { /* not laid out yet: the focus stays where it was */ }
    }
    // When idle: the time of the last remote press, and the wait before the screen goes black by itself
    val idleOff by TvPrefs.idleOff.collectAsState()
    val idleStyle by TvPrefs.idleStyle.collectAsState()
    val playing by nav.player.isPlaying.collectAsState()
    // whether the screen went off by itself (dims if so set) rather than from its button (always black)
    var byIdle by rememberSaveable { mutableStateOf(false) }
    var lastInput by remember { mutableLongStateOf(SystemClock.uptimeMillis()) }
    // restarted by every press, by the screen coming back on, by play / pause and by the setting changing
    LaunchedEffect(idleOff, screenOff, playing, lastInput) {
        val wait = ScreenOffRules.idleDelay(idleOff, screenOff, playing, SystemClock.uptimeMillis() - lastInput)
            ?: return@LaunchedEffect
        delay(wait)
        byIdle = true
        screenOff = true
    }

    Box(
        Modifier
            .fillMaxSize()
            .background(p.bg)
            // every remote press restarts the idle wait; the press itself goes on to wherever it was headed
            .onPreviewKeyEvent { lastInput = SystemClock.uptimeMillis(); false }
            // swallow presses so they don't reach the screen underneath
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = {}),
    ) {
        // the cover, blurred and washed out, under a wash that keeps the text readable (as the phone's player)
        if (coverUrl != null) {
            AsyncImage(
                model = coverUrl,
                contentDescription = null,
                imageLoader = nav.graph.imageLoader,
                contentScale = ContentScale.Crop,
                modifier = Modifier.fillMaxSize().blur(60.dp).alpha(if (p.dark) 0.45f else 0.25f),
            )
        }
        Box(Modifier.fillMaxSize().background(Brush.horizontalGradient(listOf(p.bg.copy(alpha = 0.70f), p.bg.copy(alpha = 0.90f)))))

        Column(Modifier.fillMaxSize().safeDrawingPadding().padding(horizontal = 24.dp, vertical = 12.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = onClose) {
                    Icon(painterResource(R.drawable.ic_expand_more), contentDescription = "Close", tint = p.text)
                }
                Text(
                    link?.let { "Controlling ${it.name}" + if (!it.online) " · offline" else "" } ?: "Now playing",
                    style = MaterialTheme.typography.titleMedium,
                    color = p.muted,
                    modifier = Modifier.weight(1f).padding(start = 8.dp),
                )
                GroupPill(Modifier.padding(end = 4.dp))
                // up here, away from the song's controls, so it isn't pressed by accident
                IconButton(onClick = { byIdle = false; screenOff = true }) {
                    Icon(painterResource(R.drawable.ic_screen_off), contentDescription = "Screen off", tint = p.text)
                }
                DevicesButton(nav, tint = p.text)
            }
            Row(Modifier.weight(1f).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(32.dp)) {
                // the halves in screen order; a right-to-left layout flips a Row, so the order is flipped back for it
                if (TvRules.controlsFirst(side, rtl)) {
                    ControlsHalf(nav, track, actions, onClose, coverUrl, playFocus)
                    PanelHalf(nav, panel, lyrics, onClose)
                } else {
                    PanelHalf(nav, panel, lyrics, onClose)
                    ControlsHalf(nav, track, actions, onClose, coverUrl, playFocus)
                }
            }
        }
        // over everything, the player included, while the screen is off
        ScreenOffLayer(
            screenOff,
            nav,
            onWake = { screenOff = false; lastInput = SystemClock.uptimeMillis() },
            alpha = if (byIdle) idleStyle.alpha else 1f,
        )
    }
}

/**
 * The controls half of the TV player: the cover (as big as the height left over allows), the song's lines and quality
 * badge, the seek bar, the buttons, the linked device's volume and like / share / ⋯, centred in a column.
 *
 * @param coverUrl the album's cover (null: the song has no album, [Cover] draws its placeholder).
 * @param playFocus put on play / pause, where the remote's focus starts.
 */
@Composable
private fun RowScope.ControlsHalf(
    nav: Nav,
    track: Track,
    actions: PlayerMenuActions,
    onClose: () -> Unit,
    coverUrl: String?,
    playFocus: FocusRequester,
) {
    // "Download for offline" / "Remove download" for the song, and who added it (the ⋯ menu's first line, asked when
    // the menu opens), as in the phone's player
    val offlineMenu = rememberOfflineMenu(nav)
    var askAdded by remember(track.id) { mutableStateOf(false) }
    var added by remember(track.id) { mutableStateOf("") }
    LaunchedEffect(track.id, askAdded) {
        if (!askAdded || track.id <= 0 || OfflineMode.active.value) return@LaunchedEffect
        added = try {
            withContext(Dispatchers.IO) { AlbumTypesRules.addedOf(nav.graph.api.trackAdded(track.id), ::formatDay) }
        } catch (e: CancellationException) {
            throw e
        } catch (_: Exception) {
            ""
        }
    }
    BoxWithConstraints(Modifier.weight(TvRules.PLAYER_SHARE).fillMaxHeight(), contentAlignment = Alignment.Center) {
        val art = minOf(maxWidth * 0.8f, maxHeight - UNDER_ART, MAX_ART).coerceAtLeast(MIN_ART)
        Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
            Box(Modifier.size(art).shadow(24.dp, RoundedCornerShape(16.dp), clip = false)) {
                Cover(coverUrl, nav.graph.imageLoader, Modifier.fillMaxSize(), corner = 16.dp, label = track.album.ifEmpty { track.title })
            }
            Spacer(Modifier.height(16.dp))
            PlayerMeta(nav, track, actions, onClose, center = true, Modifier.fillMaxWidth())
            TvSeek(nav, Modifier.fillMaxWidth().padding(top = 12.dp))
            PlayerButtons(nav, Modifier.fillMaxWidth().padding(top = 4.dp), playFocus = playFocus)
            // linked: the other device's volume
            LinkedVolume(nav, Modifier.fillMaxWidth().padding(horizontal = 32.dp))
            PlayerActions(
                nav, track, actions,
                // the lyrics have their own half here
                onLyrics = null,
                added = added,
                onMenuOpen = { askAdded = true },
                offlineMenu = offlineMenu,
                center = true,
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}

/**
 * The switchable half of the TV player: a two-way switch (Lyrics | Up next, [SidePanel]) over what it picks. The
 * choice is remembered ([TvPrefs.panel]), so the player opens the way it was left.
 *
 * In a group listen the queue is the group's: its half offers the group's own Up next sheet (with who added each
 * song) instead of the plain list.
 *
 * @param panel what the half shows now.
 * @param lyrics the song's lyrics as they load.
 * @param onClose closes the player (before an artist link in the queue opens a page).
 */
@Composable
private fun RowScope.PanelHalf(nav: Nav, panel: SidePanel, lyrics: LyricsLoad, onClose: () -> Unit) {
    val p = MaterialTheme.palette
    val position by nav.player.positionMs.collectAsState()
    val group by GroupListen.state.collectAsState()
    Column(
        Modifier
            .weight(1f - TvRules.PLAYER_SHARE)
            .fillMaxHeight()
            .clip(RoundedCornerShape(20.dp))
            .background(p.panel.copy(alpha = 0.55f))
            .padding(top = 12.dp),
    ) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for (s in SidePanel.entries) PanelTab(s.label, on = s == panel) { TvPrefs.setPanel(s) }
        }
        Box(Modifier.weight(1f).fillMaxWidth().padding(top = 8.dp)) {
            when {
                panel == SidePanel.LYRICS -> LyricsPane(nav, lyrics, position, Modifier.fillMaxSize())
                group != null -> Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("You're in a group listen: its queue shows who added each song.", color = p.muted, textAlign = TextAlign.Center)
                    Spacer(Modifier.height(12.dp))
                    SmallButton("Open the group's queue", iconRes = R.drawable.ic_queue_music) { nav.player.upNext() }
                }
                else -> UpNextList(nav, onLeave = onClose, modifier = Modifier.fillMaxSize(), showTitle = false)
            }
        }
    }
}

/**
 * One choice of the panel's switch: a pill, filled with the accent while it is the one shown. A press (or the remote's
 * OK) switches the half.
 *
 * @param label the choice's word.
 * @param on whether it is the half shown now.
 */
@Composable
private fun PanelTab(label: String, on: Boolean, onClick: () -> Unit) {
    val p = MaterialTheme.palette
    Text(
        label,
        style = MaterialTheme.typography.titleSmall,
        fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal,
        color = if (on) p.onAccent else p.text,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(if (on) p.accent else p.panel2)
            .clickable(onClick = onClick)
            .padding(horizontal = 18.dp, vertical = 8.dp),
    )
}

/**
 * The TV seek bar: the played part in the accent colour with the time played and the song's length under it. It takes
 * the remote's focus like a button (a ring shows it); while focused Left / Right seek [TvRules.SEEK_STEP_MS] back or
 * forward, and Up / Down move on as usual. A phone's Slider would keep Left / Right for itself in small steps, which
 * a remote can't drag.
 */
@Composable
private fun TvSeek(nav: Nav, modifier: Modifier = Modifier) {
    val p = MaterialTheme.palette
    val position by nav.player.positionMs.collectAsState()
    val duration by nav.player.durationMs.collectAsState()
    var focused by remember { mutableStateOf(false) }
    val dur = duration.coerceAtLeast(0L)
    val frac = if (dur > 0) (position.toFloat() / dur).coerceIn(0f, 1f) else 0f
    Column(modifier) {
        Box(
            Modifier
                .fillMaxWidth()
                .focusRing(p.accent)
                .onFocusChanged { focused = it.isFocused }
                .onKeyEvent { e ->
                    if (e.type != KeyEventType.KeyDown) return@onKeyEvent false
                    val forward = when (e.key) {
                        Key.DirectionRight -> true
                        Key.DirectionLeft -> false
                        else -> return@onKeyEvent false
                    }
                    if (dur > 0) nav.player.seekTo(TvRules.seekTarget(position, dur, forward))
                    true
                }
                .focusable()
                .padding(horizontal = 8.dp, vertical = 10.dp),
            contentAlignment = Alignment.CenterStart,
        ) {
            // thicker while focused, so it's clear Left / Right will move it
            Box(Modifier.fillMaxWidth().height(if (focused) 8.dp else 4.dp).clip(RoundedCornerShape(4.dp)).background(p.border)) {
                Box(Modifier.fillMaxWidth(frac).fillMaxHeight().background(p.accent))
            }
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp)) {
            Text(formatDuration(position).ifEmpty { "0:00" }, style = MaterialTheme.typography.bodySmall, color = p.faint)
            Spacer(Modifier.weight(1f))
            if (focused) Text("◀ ▶ 10 s", style = MaterialTheme.typography.bodySmall, color = p.muted)
            Spacer(Modifier.weight(1f))
            Text(formatDuration(dur).ifEmpty { "0:00" }, style = MaterialTheme.typography.bodySmall, color = p.faint)
        }
    }
}

