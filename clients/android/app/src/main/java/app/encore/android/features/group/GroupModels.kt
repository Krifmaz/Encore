package app.encore.android.features.group

import app.encore.android.core.arr
import app.encore.android.core.bool
import app.encore.android.core.dbl
import app.encore.android.core.int
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.objectsOf
import app.encore.android.core.str
import app.encore.android.data.api.Track
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject

// Group listen (0.31): several people on this server listen to the same queue at the same time, each on their own
// device. The shapes below are the ones written down in docs/contracts/client-shapes-0.31-0.33.md (§2.2, §2.3);
// every field is read leniently, so an older or newer server never costs the whole answer.

/** One person (one device) in a group listen. */
data class GroupMember(
    /** the server's id of this membership (`member`): what "Remove from the group" names */
    val key: String,
    val userId: Long,
    val name: String,
    /** a server path of the avatar ("/api/users/3/avatar"), or "" for the first letter */
    val avatar: String,
    val deviceName: String,
    val joinedAt: Double,
    val host: Boolean,
    val guest: Boolean,
) {
    companion object {
        fun from(o: JsonObject): GroupMember? {
            val name = o.str("name")
            val key = o.str("member").ifEmpty { o.str("id") }.ifEmpty { (if (o.bool("guest")) "g:" else "u:") + o.str("user") + ":" + o.str("device_name") }
            if (name.isEmpty() && key.isEmpty()) return null
            return GroupMember(
                key = key,
                userId = o.long("user"),
                name = name.ifEmpty { "Someone" },
                avatar = o.str("avatar"),
                deviceName = o.str("device_name"),
                joinedAt = o.dbl("joined_at"),
                host = o.bool("host"),
                guest = o.bool("guest"),
            )
        }
    }
}

/** One song of the group's queue, with who added it ("added by Sam"). */
data class GroupEntry(
    /** the queue entry's own id (the same song can be queued twice): what remove / move / jump name */
    val qid: String,
    val track: Track,
    val addedBy: String,
    val addedByUser: Long,
) {
    companion object {
        fun from(o: JsonObject, at: Int): GroupEntry? {
            val t = Track.from(o) ?: return null
            val by = o.obj("added_by")
            return GroupEntry(
                qid = o.str("qid").ifEmpty { "i$at-${t.id}" },
                track = t,
                addedBy = by?.str("name") ?: o.str("added_by"),
                addedByUser = by?.long("user") ?: 0L,
            )
        }
    }
}

/** The group listen this phone is in, as the server last told it (REST answers and the event stream). */
data class GroupState(
    val id: String,
    val code: String,
    val inviteUrl: String,
    val rev: Long,
    val hostName: String,
    val hostUser: Long,
    val hostAvatar: String,
    val everyoneControls: Boolean,
    /** "anyone" (anyone on this server can see and join) | "link" (only people with the link) */
    val visibility: String,
    /** the host's switch "Guests can join with the link" */
    val guestsAllowed: Boolean,
    /** the server allows guests at all (`group_listen_guests`): the host's switch shows only then */
    val guestsPossible: Boolean,
    /** this group's cap ("Limit to N people") and the server's (`group_listen_max`) */
    val maxMembers: Int,
    val serverMax: Int,
    val queue: List<GroupEntry>,
    val index: Int,
    val playing: Boolean,
    /** seconds into the song at [anchorMs] (the server's clock, ms) */
    val positionSec: Double,
    val anchorMs: Long,
    val members: List<GroupMember>,
    val startedAt: Double,
    /** the server process's id: a different one after a restart */
    val boot: String,
    /** this device's membership, and what it may do */
    val youMember: String,
    val youHost: Boolean,
    val youCanControl: Boolean,
) {
    val track: Track? get() = queue.getOrNull(index)?.track
    val count: Int get() = members.size
    val linkOnly: Boolean get() = visibility == "link"

    companion object {
        fun from(e: JsonElement?): GroupState? {
            val o = e as? JsonObject ?: return null
            // the event stream may wrap it: {state: {...}}
            val s = o.obj("state") ?: o
            val id = s.str("id")
            if (id.isEmpty()) return null
            val host = s.obj("host")
            val queue = s.arr("queue").objectsOf().mapIndexedNotNull { i, q -> GroupEntry.from(q, i) }
            val members = s.arr("members").objectsOf().mapNotNull { GroupMember.from(it) }
            val you = s.obj("you")
            val hostUser = host?.long("user") ?: 0L
            val youHost = you?.bool("host") ?: false
            val everyone = s.bool("everyone_controls")
            val serverMax = s.int("server_max_members").takeIf { it > 0 } ?: 20  // group_listen_max's default (p6)
            return GroupState(
                id = id,
                code = s.str("code").uppercase(),
                inviteUrl = s.str("invite_url"),
                rev = s.long("rev"),
                hostName = host?.str("name")?.ifEmpty { null } ?: members.firstOrNull { it.host }?.name ?: "the host",
                hostUser = hostUser,
                hostAvatar = host?.str("avatar") ?: "",
                everyoneControls = everyone,
                visibility = if (s.str("visibility") == "link") "link" else "anyone",
                guestsAllowed = s.bool("guests_allowed"),
                guestsPossible = s.bool("guests_possible"),
                maxMembers = s.int("max_members").takeIf { it > 0 } ?: serverMax,
                serverMax = serverMax,
                queue = queue,
                index = if (queue.isEmpty()) -1 else s.int("index").coerceIn(0, queue.size - 1),
                playing = s.bool("playing"),
                positionSec = s.dbl("position").coerceAtLeast(0.0),
                anchorMs = s.long("anchor_ms"),
                members = members,
                startedAt = s.dbl("started_at"),
                boot = s.str("boot"),
                youMember = you?.str("member") ?: "",
                youHost = youHost,
                // an older answer without `you.can_control`: the host and "Everyone can control"
                youCanControl = if (you != null && you.containsKey("can_control")) you.bool("can_control") else youHost || everyone,
            )
        }
    }
}

/** One live group listen in the server's list (GET /api/group): Home's "Listening together now" and the devices sheet. */
data class GroupSummary(
    val id: String,
    val hostName: String,
    val hostAvatar: String,
    val title: String,
    val artist: String,
    val albumId: String,
    val playing: Boolean,
    val membersCount: Int,
    val startedAt: Double,
    val visibility: String,
    /** this account is in it (on this or another device) */
    val mine: Boolean,
) {
    companion object {
        fun from(o: JsonObject): GroupSummary? {
            val id = o.str("id")
            if (id.isEmpty()) return null
            val host = o.obj("host")
            val t = o.obj("track")
            return GroupSummary(
                id = id,
                hostName = host?.str("name")?.ifEmpty { null } ?: "Someone",
                hostAvatar = host?.str("avatar") ?: "",
                title = t?.str("title") ?: "",
                artist = t?.str("artist") ?: "",
                albumId = t?.str("album_id") ?: "",
                playing = o.bool("playing"),
                membersCount = o.int("members_count").coerceAtLeast(1),
                startedAt = o.dbl("started_at"),
                visibility = if (o.str("visibility") == "link") "link" else "anyone",
                mine = o.bool("mine"),
            )
        }
    }
}

/** A link or code to join, waiting for "Join <host>'s group listen?". */
data class GroupInvite(val code: String, val summary: GroupSummary?)

/**
 * The rules of a group listen that need no Android: reading codes and links, where the song should be now and the
 * words. Pure Kotlin (tested in GroupRulesTest); what to do about drift, the clock and the rung are in SyncRules.kt.
 */
object GroupRules {
    /** Codes are 6 characters from an unambiguous alphabet (typed: spaces and dashes left out); links may carry 4–12. */
    private val CODE = Regex("^[A-Za-z0-9]{6}$")
    private val LINK = Regex("""#/group/([A-Za-z0-9]{4,12})""")

    /** The code in what was typed or shared: a bare code ("k7qm 4x" too), or a link `https://<server>/#/group/<code>`. */
    fun codeFrom(text: String): String? {
        val t = text.trim()
        LINK.find(t)?.let { return it.groupValues[1].uppercase() }
        val bare = t.replace(" ", "").replace("-", "")
        return if (CODE.matches(bare)) bare.uppercase() else null
    }

    /** The server part of a group link ("https://music.example.com"), or null when the text is no link. */
    fun serverOf(text: String): String? {
        val m = Regex("""(https?://[^/\s#?]+)(/[^#\s]*)?#/group/""").find(text) ?: return null
        val path = (m.groupValues.getOrNull(2) ?: "").trimEnd('/')
        return m.groupValues[1] + path
    }

    /** "K7QM4X" → "K7Q M4X" (easier to read out). */
    fun spaced(code: String): String = if (code.length == 6) code.substring(0, 3) + " " + code.substring(3) else code

    /** The invite link: the server's own when it sent one, else `<server>/#/group/<code>` as the web builds it. */
    fun inviteLink(state: GroupState, base: String): String =
        state.inviteUrl.ifEmpty { base.trimEnd('/') + "/#/group/" + state.code }

    /**
     * Where the group is now: the song's index and the position in it (seconds), given the server's clock. While
     * playing, the position runs on from the anchor (expected = position + (server_now − anchor_ms) / 1000), and a
     * song that would have ended rolls over into the next one (the server moves on by itself at the end of a song).
     */
    fun expected(state: GroupState, serverNowMs: Long): Pair<Int, Double> {
        if (state.queue.isEmpty()) return -1 to 0.0
        var index = state.index.coerceIn(0, state.queue.size - 1)
        var pos = state.positionSec
        if (state.playing && state.anchorMs > 0) pos += (serverNowMs - state.anchorMs).coerceAtLeast(0L) / 1000.0
        while (true) {
            val dur = state.queue[index].track.durationSec
            if (dur <= 0.0 || pos < dur) break
            if (index >= state.queue.size - 1) { pos = dur; break }
            pos -= dur
            index++
        }
        return index to pos.coerceAtLeast(0.0)
    }

    // ---- Play and Pause in a group (the owner on Oct 21: "it should just add it to the que", "it may pause for me but
    // dont stop the other users playback"; the same on web, phone and desktop) --------------------------------------

    /**
     * Play (or Shuffle) of an album, a playlist, an artist or a song in a group starts the group – the songs become its
     * queue (`set_queue`) – only when it plays nothing (paused, or past its last song) and has nothing in Up next, or
     * its queue is empty. Otherwise the songs are added to its Up next (`enqueue`). The web's `rules::play_starts`.
     */
    fun playStarts(state: GroupState, serverNowMs: Long): Boolean {
        if (state.queue.isEmpty()) return true
        val (ei, epos) = expected(state, serverNowMs)
        val last = ei >= state.queue.size - 1
        val dur = state.queue[ei].track.durationSec
        val atEnd = last && dur > 0.0 && epos >= dur - 0.3
        return (!state.playing || atEnd) && last
    }

    /**
     * The songs Play adds to the group's Up next: those that would have played – the library songs (id > 0) from the one
     * asked for ([index] in [ids]) to the end of the list, at most [max].
     */
    fun toAdd(ids: List<Long>, index: Int, max: Int = 1000): List<Long> =
        ids.drop(index.coerceAtLeast(0)).filter { it > 0L }.take(max)

    /** The toast once the group took songs into its Up next: "Added 12 songs to the group's Up next", "…, shuffled". */
    fun addedText(n: Int, shuffled: Boolean): String {
        val songs = if (n == 1) "1 song" else "$n songs"
        return "Added $songs to the group's Up next" + if (shuffled) ", shuffled" else ""
    }

    /** Pause in a group pauses only this phone; the group plays on. */
    const val PAUSED_FOR_YOU = "Paused for you – the group keeps playing"

    /** The group's own pause (the player's ⋯ and the group panel, for those who control the group). */
    const val PAUSE_FOR_EVERYONE = "Pause for everyone"

    /** Whether "Pause for everyone" is offered: this device may control the group and the group plays. */
    fun offersPauseForEveryone(state: GroupState): Boolean = state.youCanControl && state.playing

    /** "Only Alex controls playback": why a member's control is off. */
    fun onlyHost(state: GroupState): String = "Only ${state.hostName} controls playback"

    /** The pill on the player: "Group · 3". */
    fun pill(state: GroupState): String = "Group · ${state.count}"

    /** "Alex · 3 listening" on a tile of the server's list. */
    fun tileLine(g: GroupSummary): String = "${g.hostName} · ${g.membersCount} listening"

    /**
     * The toasts for people who came or went between two states ("Sam joined", "Sam left"); this device left out. Its
     * membership before counts as this device too: after being dropped (2 minutes without the event stream) the phone
     * joins again with the code and gets a new membership id, which is not "<you> left".
     */
    fun comings(old: GroupState?, new: GroupState): List<String> {
        if (old == null || old.id != new.id) return emptyList()
        val me = setOf(old.youMember, new.youMember) - ""
        val before = old.members.associateBy { it.key }
        val after = new.members.associateBy { it.key }
        val out = ArrayList<String>()
        for ((k, m) in after) if (k !in before && k !in me) out.add("${m.name} joined")
        for ((k, m) in before) if (k !in after && k !in me) out.add("${m.name} left")
        if (old.hostUser != new.hostUser && new.hostUser != 0L && old.hostUser != 0L) {
            out.add(if (new.youHost) "You host the group listen now" else "${new.hostName} hosts the group listen now")
        }
        return out
    }

    /**
     * Why this phone is out of its group when joining again after being dropped was refused (client-shapes §3.9, §3.17):
     * the server's own sentence (it always sends one: the group is full, too many tries, turned off), else one by the
     * status.
     */
    fun rejoinRefused(code: Int, detail: String?): String {
        val d = detail?.trim().orEmpty()
        if (d.isNotEmpty() && !d.startsWith("HTTP ")) return d
        return when (code) {
            429 -> "Too many tries. Wait a moment and try again."
            403 -> "Group listens are turned off on this server"
            else -> "Couldn't join"
        }
    }

    /** What the end of a group listen is called, by the `ended` event's reason. */
    fun endedText(reason: String, hostName: String): String = when (reason) {
        "removed" -> "You were removed from the group listen"
        "off" -> "Group listens are turned off on this server"
        "restart" -> "The group listen ended (the server restarted)"
        "ended" -> if (hostName.isNotEmpty()) "$hostName ended the group listen" else "The group listen ended"
        else -> "The group listen ended"
    }
}
