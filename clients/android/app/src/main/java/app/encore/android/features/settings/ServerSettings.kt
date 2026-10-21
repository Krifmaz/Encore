package app.encore.android.features.settings

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.unit.dp
import app.encore.android.data.api.SettingsApi
import app.encore.android.ui.components.ErrorBox
import app.encore.android.ui.screens.Spinner
import kotlinx.serialization.json.JsonObject

// Settings → Server pages drawn from the server's settings schema (General, Library, Downloads, Integrations, Server
// processing, Email & notifications, and the Playlists & sync card of Scheduled tasks): SchemaPage (SchemaForm.kt)
// draws the cards; this file adds what each page has of its own, as web-rs/src/settings/groups.rs show_page does: the
// task list above Scheduled tasks' card, the cards' extras (apps.rs card_body), How requests are fulfilled at the end
// of the page with the Download pipeline card (Downloads; on a server before R2, Apps & sources), and the API key and
// About cards at the end of General. Since 0.32 the source cards of Downloads (streamrip, Torrents & Usenet,
// Soulseek, yt-dlp and YouTube sign-in) are plugin cards on Settings → Plugins (PluginsScreen.kt); a server without
// plugins (before 0.32) still has them here.

/** The schema and values the Server pages share while Settings is open (null while loading). */
class ServerSchema(val state: SchemaFormState?, val error: String?, val retry: () -> Unit)

@Composable
fun ServerSectionScreen(
    sectionId: String,
    schema: ServerSchema,
    schemaApi: SchemaApi,
    api: SettingsApi,
    info: JsonObject?,
    focus: String?,
    /** whether this server has plugins (null: not known, taken as yes): its source cards are on Settings → Plugins */
    plugins: Boolean?,
    onBack: () -> Unit,
    onOpen: (String) -> Unit,
) {
    val state = schema.state
    // a page this server calls otherwise (apps on a server since R2, downloads on an older one) opens its page here
    val page = state?.schema?.let { it.section(it.pageOf(sectionId)) } ?: state?.schema?.sections?.firstOrNull()
    // the cards that are plugin cards since 0.32 are drawn on the Plugins page (groups.rs `page_cards`)
    val section = if (plugins != false) page?.let { p -> p.copy(groups = p.groups.filterNot { PluginRules.moved(it.id) }) } else page
    SettingsPage(section?.title ?: SECTION_TITLES[sectionId] ?: "Settings", onBack) {
        if (state == null || section == null) {
            if (schema.error != null) ErrorBox(schema.error) { schema.retry() } else Spinner(24.dp)
            return@SettingsPage
        }
        if (section.id == "tasks") TaskList(api)
        // one engine-settings holder per page, so the Cadenza card's Save carries what was changed in its block
        val engine = remember(section.id) { EngineState() }
        val slots = remember(section.id, state) { cardSlots(api, state, engine) }
        // How requests are fulfilled ends the page with the Download pipeline card
        val downloads = section.groups.any { it.id == "pipeline" }
        SchemaPage(
            state = state,
            api = schemaApi,
            section = section,
            focus = focus,
            slots = { g -> slots[g.id] ?: CardSlots.None },
        )
        if (downloads) PipelineHelp(plugins != false, onOpen)
        if (section.id == "general") {
            ApiKeyCard(api)
            AboutCard(info)
        }
    }
}

/**
 * The extras of the source cards, by card id, which are also the plugin cards' (PluginsScreen.kt): Tidal sign-in and
 * the streamrip config under streamrip's fields; the Soulseek status, the VPN's state with Test VPN and the listen
 * port's rule on Soulseek's (apps.rs card_body, plugins.rs extras_html).
 */
fun sourceSlots(api: SettingsApi): Map<String, CardSlots> = mapOf(
    "streamrip" to CardSlots(below = {
        TidalSignIn(api)
        StreamripConfigBlock(api)
    }),
    "slskd" to CardSlots(
        below = { SoulseekStatusBlock(api) },
        vpnFooter = { card -> VpnFooter(api, card) },
        // the listen port is not used while the VPN is on (the VPN's forwarded port is)
        hide = { f, current -> f.key == "soulseek_port" && current("vpn_enabled") == "true" },
    ),
)

/** The extras of the cards that have some, by card id (apps.rs card_body and bind_card). */
private fun cardSlots(api: SettingsApi, state: SchemaFormState, engine: EngineState): Map<String, CardSlots> = sourceSlots(api) + mapOf(
    "youtube" to CardSlots(above = { YouTubeCard(api) }),
    "cadenza" to CardSlots(
        below = { CadenzaBlocks(api, state, engine) },
        extraBody = { engine.extra(state.values) },
        afterSave = { engine.committed() },
    ),
)

/** The pages' titles before the schema has loaded (the web's words). */
val SECTION_TITLES: Map<String, String> = mapOf(
    "account" to "Account",
    "appearance" to "Appearance",
    "language" to "Language",
    "playback" to "Playback",
    "connections" to "Connections",
    "device" to "This device",
    "users" to "Users & permissions",
    "invites" to "Invites",
    "general" to "General",
    "library" to "Library",
    "downloads" to "Downloads",
    "plugins" to "Plugins",
    "integrations" to "Integrations",
    "processing" to "Server processing",
    "accounts" to "Email & notifications",
    "autodownload" to "Auto-download",
    "contribute" to "Contribute",
    "blocked" to "Blocked",
    "tasks" to "Scheduled tasks",
    "diagnostics" to "Diagnostics",
)
