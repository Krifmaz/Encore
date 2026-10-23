package app.encore.android.features.library

import app.encore.android.core.bool
import app.encore.android.core.int
import app.encore.android.core.long
import app.encore.android.core.obj
import app.encore.android.core.objects
import app.encore.android.core.str
import kotlinx.serialization.json.JsonObject

/**
 * Collaborative playlists (web-rs/src/pages/collab.rs, docs/contracts/collaborative-playlists.md): the owner turns
 * Collaborative on (PATCH /api/playlists/{id} {collaborative}) and invites accounts on this server by user name or with
 * an invite link (`…/#/collab/<token>`). Whoever joins sees it among their playlists and can add, remove and reorder its
 * songs; only the owner renames, shares or deletes it, and a member's Delete is Leave. Pure, so the unit tests check it.
 */
object PlaylistCollabRules {
    const val TOGGLE = "Collaborative"
    const val INVITE_TITLE = "Invite collaborators"
    const val MEMBERS_TITLE = "Members"
    const val MEMBER_HINT = "You can add, remove and reorder songs"
    const val OWNER_HINT = "People you invite can add, remove and reorder songs. Only you can rename, share or delete the playlist."
    const val USER_PLACEHOLDER = "User name on this server"
    const val LINK_HINT = "Anyone on this server with the link can join."
    const val ON = "Collaborative: people you invite can change the songs"
    const val OFF = "Collaborative off: members can't see it until it's on again"
    const val LEAVE = "Leave playlist"
    const val LEFT = "You left the playlist"
    const val JOINED = "Joined: you can change its songs now"
    const val LEAVE_TEXT = "You stop seeing it and can't change it any more. The owner can invite you again."

    /** "COLLABORATIVE · BY ANNA" over a member's playlist (the phone shows it in capitals). */
    fun kicker(owner: String) = "Collaborative playlist · by $owner"

    /** "" / "1 member" / "3 members". */
    fun membersText(n: Int) = when {
        n <= 0 -> ""
        n == 1 -> "1 member"
        else -> "$n members"
    }

    /** The tile's line: " · collaborative" for the owner's, " · by anna" for a member's. */
    fun tileSuffix(collaborative: Boolean, member: Boolean, owner: String) = when {
        member -> " · by $owner"
        collaborative -> " · collaborative"
        else -> ""
    }

    fun leaveText(name: String) = "Leave “$name”? $LEAVE_TEXT"

    fun inviteText(i: Invite) = "${i.owner} invited you to collaborate on ${i.name} · ${i.count} tracks"

    /** A pasted text that is an invite link rather than a share link. */
    fun isInviteLink(text: String) = text.contains("collab/")

    /** `collaborative` and `collab.{role, owner, members}` of a playlist object. */
    data class Collab(val collaborative: Boolean, val role: String, val owner: String, val members: Int) {
        val isMember: Boolean get() = role == "collaborator"
    }

    fun collabOf(o: JsonObject): Collab {
        val c = o.obj("collab")
        return Collab(o.bool("collaborative"), c?.str("role") ?: "", c?.str("owner") ?: "", c?.int("members") ?: 0)
    }

    data class Member(val userId: Long, val username: String, val invited: Boolean)

    /** GET /api/playlists/{id}/collaborators. [link] is the invite link's url (or path) for the owner, else "". */
    data class Members(val owner: String, val isOwner: Boolean, val collaborative: Boolean, val members: List<Member>, val link: String) {
        companion object {
            fun from(o: JsonObject): Members {
                val l = o.obj("invite_link")
                val link = l?.str("url")?.takeIf { it.startsWith("http") } ?: l?.str("path") ?: ""
                return Members(
                    o.str("owner"), o.str("role") == "owner", o.bool("collaborative"),
                    o["members"].objects().map { Member(it.long("user_id"), it.str("username"), it.str("status") == "invited") },
                    link,
                )
            }
        }
    }

    /** GET /api/collab/invites: one waiting invite. */
    data class Invite(val playlistId: Long, val name: String, val owner: String, val count: Int) {
        companion object {
            fun from(o: JsonObject) = Invite(o.long("playlist_id"), o.str("name"), o.str("owner"), o.int("count"))
        }
    }

    /** An entry of a playlist's ⋯ that depends on collaboration. */
    enum class Entry(val label: String) { TURN_ON("Make collaborative"), TURN_OFF("Turn off collaborative"), INVITE(INVITE_TITLE), MEMBERS(MEMBERS_TITLE), LEAVE(PlaylistCollabRules.LEAVE) }

    /** Those entries in order: none for Liked Songs; a member's Members and Leave; the owner's switch and Invite. */
    fun menuEntries(liked: Boolean, member: Boolean, collaborative: Boolean): List<Entry> = when {
        member -> listOf(Entry.MEMBERS, Entry.LEAVE)
        liked -> emptyList()
        collaborative -> listOf(Entry.INVITE, Entry.TURN_OFF)
        else -> listOf(Entry.TURN_ON)
    }
}
