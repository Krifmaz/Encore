package app.encore.android.ui.components

/**
 * What the player's "⋯" menu can do (web/js/player.js `moreMenu`), in the order the web lists them:
 * Go to artist, View album, Add to playlist, Lyrics, Share link, Signal path – and the "Up next" button.
 *
 * Shared seam (docs: android-m2m3 contract): the DESIGN part defines this interface and draws the menu; the
 * PLAYER part implements it in ui/PlayerConnection.kt. Every call is about the song that is playing now.
 */
interface PlayerMenuActions {
    /** "Go to artist": open the artist of the current song. */
    fun goToArtist()

    /** "View album": open the album page of the current song. */
    fun viewAlbum()

    /** "Add to playlist": the add-to-playlist picker for the current song. */
    fun addToPlaylist()

    /** "Lyrics": show the lyrics screen (synced .lrc when available). */
    fun lyrics()

    /** "Share link": POST /api/library/tracks/{id}/share, copy the link and open the system share sheet. */
    fun shareLink()

    /** "Signal path": file / what Encore sent / what this device does. */
    fun signalPath()

    /** "Up next": the queue sheet. */
    fun upNext()
}

/** Does nothing; a placeholder until the real actions are wired. */
object NoPlayerMenuActions : PlayerMenuActions {
    override fun goToArtist() {}
    override fun viewAlbum() {}
    override fun addToPlaylist() {}
    override fun lyrics() {}
    override fun shareLink() {}
    override fun signalPath() {}
    override fun upNext() {}
}
