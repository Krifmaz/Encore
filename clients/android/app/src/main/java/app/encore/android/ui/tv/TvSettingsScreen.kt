/**
 * Settings → TV: how Encore looks on this TV. Listed in Settings only on a TV (Android TV / Google TV), and kept on
 * this device ([TvPrefs]), never in the account.
 */
package app.encore.android.ui.tv

import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import app.encore.android.features.settings.Muted
import app.encore.android.features.settings.SegRow
import app.encore.android.features.settings.SetCard
import app.encore.android.features.settings.SettingsPage

/**
 * The TV page: Layout (the TV layout or the phone one), Player side (which side the TV player's controls take; the
 * lyrics / Up next panel takes the other), Size (how far the screen is zoomed out) and When idle (how long the player
 * waits untouched before turning the screen off or dimming it by itself; never by default). Each choice applies at once.
 *
 * @param onBack back to the Settings list.
 */
@Composable
fun TvSettingsScreen(onBack: () -> Unit) {
    val mode by TvPrefs.mode.collectAsState()
    val side by TvPrefs.side.collectAsState()
    val zoom by TvPrefs.zoom.collectAsState()
    val idleOff by TvPrefs.idleOff.collectAsState()
    val idleStyle by TvPrefs.idleStyle.collectAsState()
    SettingsPage("TV", onBack, subtitle = "How Encore looks on this TV. Kept on this TV only.") {
        SetCard {
            Label("Layout")
            SegRow(TvLayoutMode.entries.map { it.key to it.label }, mode.key) { TvPrefs.setMode(TvLayoutMode.of(it)) }
            Muted("Automatic: the side menu and the TV player, made for the remote. Phone layout: the same screens as on a phone.")
        }
        Spacer(Modifier.height(12.dp))
        // the other two only mean something in the TV layout
        val tvOn = mode == TvLayoutMode.AUTO
        SetCard {
            Label("Player side")
            SegRow(PlayerSide.entries.map { it.key to it.label }, side.key, enabled = tvOn) { TvPrefs.setSide(PlayerSide.of(it)) }
            Muted("Where Now playing puts the cover and the controls. The other side switches between the lyrics and Up next.")
        }
        Spacer(Modifier.height(12.dp))
        SetCard {
            Label("Size")
            SegRow(TvZoom.entries.map { it.key to it.label }, zoom.key, enabled = tvOn) { TvPrefs.setZoom(TvZoom.of(it)) }
            Muted("Smaller fits more on the screen at once; Larger is easier to read from far away.")
        }
        Spacer(Modifier.height(12.dp))
        SetCard {
            Label("When idle")
            SegRow(TvIdleStyle.entries.map { it.key to it.label }, idleStyle.key, enabled = tvOn && idleOff != TvIdleOff.NEVER) {
                TvPrefs.setIdleStyle(TvIdleStyle.of(it))
            }
            Spacer(Modifier.height(8.dp))
            SegRow(TvIdleOff.entries.map { it.key to it.label }, idleOff.key, enabled = tvOn) { TvPrefs.setIdleOff(TvIdleOff.of(it)) }
            Muted("While the music plays and the remote goes untouched this long, the screen turns off (black) or dims; the music keeps playing. Any button brings it back.")
        }
    }
}

/** A setting's name over its choices. */
@Composable
private fun Label(text: String) {
    Text(text, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
}
