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
// task list above Scheduled tasks' card, the cards' extras (apps.rs card_body), Download tools at the top and How
// requests are fulfilled at the end of the page with the Download pipeline card (Downloads; on a server before R2,
// Apps & sources, where Download tools sits under its "Download sources" heading), and the API key and About cards at
// the end of General.

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
    onBack: () -> Unit,
    onOpen: (String) -> Unit,
) {
    val state = schema.state
    // a page this server calls otherwise (apps on a server since R2, downloads on an older one) opens its page here
    val section = state?.schema?.let { it.section(it.pageOf(sectionId)) } ?: state?.schema?.sections?.firstOrNull()
    SettingsPage(section?.title ?: SECTION_TITLES[sectionId] ?: "Settings", onBack) {
        if (state == null || section == null) {
            if (schema.error != null) ErrorBox(schema.error) { schema.retry() } else Spinner(24.dp)
            return@SettingsPage
        }
        if (section.id == "tasks") TaskList(api)
        // one engine-settings holder per page, so the Cadenza card's Save carries what was changed in its block
        val engine = remember(section.id) { EngineState() }
        val slots = remember(section.id, state) { cardSlots(api, state, engine) }
        // Download tools starts the first part of the page with the Download pipeline card
        val downloads = section.groups.any { it.id == "pipeline" }
        val firstPart = SchemaRules.visibleCards(section, state.values).firstOrNull()?.first?.part
        SchemaPage(
            state = state,
            api = schemaApi,
            section = section,
            focus = focus,
            partTop = { part -> if (downloads && part == firstPart) DownloadToolsCard(api, state, onOpen) },
            slots = { g -> slots[g.id] ?: CardSlots.None },
        )
        if (downloads) PipelineHelp()
        if (section.id == "general") {
            ApiKeyCard(api)
            AboutCard(info)
        }
    }
}

/** The extras of the cards that have some, by card id (apps.rs card_body and bind_card). */
private fun cardSlots(api: SettingsApi, state: SchemaFormState, engine: EngineState): Map<String, CardSlots> = mapOf(
    "youtube" to CardSlots(above = { YouTubeCard(api) }),
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
    "integrations" to "Integrations",
    "processing" to "Server processing",
    "accounts" to "Email & notifications",
    "autodownload" to "Auto-download",
    "contribute" to "Contribute",
    "blocked" to "Blocked",
    "tasks" to "Scheduled tasks",
    "diagnostics" to "Diagnostics",
)
