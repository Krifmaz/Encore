package app.encore.android.car

import android.graphics.Bitmap
import androidx.car.app.CarContext
import androidx.car.app.Screen
import androidx.car.app.constraints.ConstraintManager
import androidx.car.app.model.Action
import androidx.car.app.model.CarIcon
import androidx.car.app.model.Header
import androidx.car.app.model.ItemList
import androidx.car.app.model.ListTemplate
import androidx.car.app.model.Row
import androidx.car.app.model.Template
import androidx.core.content.ContextCompat
import androidx.core.graphics.drawable.IconCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.media3.common.MediaItem
import app.encore.android.R
import java.util.concurrent.Executors

/**
 * A page of the media app's browse tree in the Encore Lyrics car app (0.31): the artist's page from the artist row
 * (`artist:<mbid or name>`: Shuffle, Popular songs, Liked from this artist, the albums), and the lists it opens,
 * read through the session's browse API ([LyricsCarSession.children]). A song or Shuffle plays and goes back to the
 * lyrics; a folder or album opens its own list.
 */
class BrowseScreen(
    carContext: CarContext,
    private val session: LyricsCarSession,
    private val nodeId: String,
    private val title: String,
) : Screen(carContext) {
    private var items: List<MediaItem>? = null
    private var failed = false
    /** covers of the rows, loaded once after the list arrived (one template update for all of them) */
    private val art = HashMap<String, Bitmap>()
    private val covers = Executors.newSingleThreadExecutor { r -> Thread(r, "car-browse-covers").apply { isDaemon = true } }
    private var gone = false

    init {
        lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onDestroy(owner: LifecycleOwner) {
                gone = true
                covers.shutdownNow()
            }
        })
        session.children(nodeId) { list ->
            if (gone) return@children
            items = list
            failed = list == null
            invalidate()
            if (list != null) loadCovers(list.take(limit()))
        }
    }

    /** How many rows the host shows in a list (the rest would be cut anyway). */
    private fun limit(): Int = try {
        carContext.getCarService(ConstraintManager::class.java).getContentLimit(ConstraintManager.CONTENT_LIMIT_TYPE_LIST)
    } catch (_: Exception) { 50 }.coerceIn(6, LyricsCarSession.BROWSE_PAGE)

    private fun loadCovers(list: List<MediaItem>) {
        val want = list.mapNotNull { item -> item.mediaMetadata.artworkUri?.let { item.mediaId to it } }
        if (want.isEmpty()) return
        val main = ContextCompat.getMainExecutor(carContext)
        try {
            covers.execute {
                val got = HashMap<String, Bitmap>()
                for ((id, uri) in want) {
                    if (gone) return@execute
                    CoverArt.load(carContext, uri, 128)?.let { got[id] = it }
                }
                main.execute {
                    if (gone || got.isEmpty()) return@execute
                    art.putAll(got)
                    invalidate()
                }
            }
        } catch (_: java.util.concurrent.RejectedExecutionException) {}
    }

    override fun onGetTemplate(): Template {
        val b = ListTemplate.Builder()
            .setHeader(Header.Builder().setTitle(title).setStartHeaderAction(Action.BACK).build())
        val list = items
        if (list == null && !failed) return b.setLoading(true).build()
        val rows = ItemList.Builder()
        if (list.isNullOrEmpty()) {
            rows.setNoItemsMessage(if (failed) "Encore on your phone didn't answer. Try again in a moment." else "Nothing here yet")
        } else {
            for (item in list.take(limit())) rows.addItem(row(item))
        }
        return b.setSingleList(rows.build()).build()
    }

    private fun row(item: MediaItem): Row {
        val md = item.mediaMetadata
        val name = md.title?.toString()?.ifEmpty { null } ?: "Untitled"
        val r = Row.Builder().setTitle(name)
        val sub = md.subtitle?.toString()?.ifEmpty { null } ?: md.artist?.toString()?.ifEmpty { null }
        if (sub != null) r.addText(sub)
        val pic = art[item.mediaId]
        val icon = when {
            pic != null -> IconCompat.createWithBitmap(pic)
            md.isBrowsable == true -> IconCompat.createWithResource(carContext, R.drawable.ic_queue_music)
            item.mediaId.startsWith("track:") -> IconCompat.createWithResource(carContext, R.drawable.ic_note)
            else -> IconCompat.createWithResource(carContext, R.drawable.ic_shuffle)
        }
        r.setImage(CarIcon.Builder(icon).build())
        if (md.isBrowsable == true) {
            r.setBrowsable(true)
            r.setOnClickListener { screenManager.push(BrowseScreen(carContext, session, item.mediaId, name)) }
        } else {
            r.setOnClickListener {
                session.play(item.mediaId)
                // back to the lyrics of what plays now
                screenManager.popToRoot()
            }
        }
        return r.build()
    }
}
