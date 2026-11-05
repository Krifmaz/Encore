package app.encore.android

import dev.krifmaz.kit.compose.bugs.BugReportsApi
import dev.krifmaz.kit.compose.bugs.CrashLogHost
import dev.krifmaz.kit.compose.core.KitApi
import dev.krifmaz.kit.compose.settings.SchemaApi
import kotlinx.coroutines.CoroutineScope

// What the kit's parts (kit-compose) need from Encore: its server API with the session, and its scope and store.

/** The settings schema calls (Settings → Server), over the app's client. */
val AppGraph.schemaApi: SchemaApi get() = SchemaApi(api, http)

/** The bug report calls, over the app's client. */
val AppGraph.bugReports: BugReportsApi get() = BugReportsApi(api, http)

/** What the crash log needs to send reports from earlier runs: the session and the server. */
val AppGraph.crashHost: CrashLogHost
    get() {
        val graph = this
        return object : CrashLogHost {
            override val scope: CoroutineScope get() = graph.scope

            override suspend fun signedIn(): Boolean {
                graph.store.awaitLoaded()
                return graph.store.current != null
            }

            override val deviceId: String get() = graph.store.deviceId
            override val api: KitApi get() = graph.api
            override val bugReports: BugReportsApi get() = graph.bugReports
        }
    }
