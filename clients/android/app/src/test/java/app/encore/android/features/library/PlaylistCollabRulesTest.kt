package app.encore.android.features.library

import app.encore.android.core.EncoreJson
import app.encore.android.data.api.Playlist
import kotlinx.serialization.json.JsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class PlaylistCollabRulesTest {
    private fun obj(json: String): JsonObject = EncoreJson.parseToJsonElement(json) as JsonObject

    @Test
    fun aPlaylistSaysWhoItIsTo() {
        val own = Playlist.from(obj("""{"id":1,"name":"Road trip","kind":"playlist","collaborative":1,"collab":{"role":"owner","owner":"anna","members":2}}"""))!!
        assertTrue(own.collab.collaborative)
        assertFalse(own.isMember)
        assertEquals(2, own.collab.members)
        val theirs = Playlist.from(obj("""{"id":2,"name":"Road trip","kind":"playlist","collaborative":1,"collab":{"role":"collaborator","owner":"anna","members":1}}"""))!!
        assertTrue(theirs.isMember)
        assertEquals("anna", theirs.collab.owner)
        val plain = Playlist.from(obj("""{"id":3,"name":"Mix","kind":"playlist","collaborative":0,"collab":null}"""))!!
        assertFalse(plain.collab.collaborative)
        assertFalse(plain.isMember)
    }

    @Test
    fun linesAndMenus() {
        assertEquals("", PlaylistCollabRules.membersText(0))
        assertEquals("1 member", PlaylistCollabRules.membersText(1))
        assertEquals("3 members", PlaylistCollabRules.membersText(3))
        assertEquals(" · by anna", PlaylistCollabRules.tileSuffix(true, true, "anna"))
        assertEquals(" · collaborative", PlaylistCollabRules.tileSuffix(true, false, "anna"))
        assertEquals("", PlaylistCollabRules.tileSuffix(false, false, ""))
        assertEquals(listOf(PlaylistCollabRules.Entry.MEMBERS, PlaylistCollabRules.Entry.LEAVE), PlaylistCollabRules.menuEntries(false, true, true))
        assertEquals(emptyList<PlaylistCollabRules.Entry>(), PlaylistCollabRules.menuEntries(true, false, false))
        assertEquals(listOf(PlaylistCollabRules.Entry.TURN_ON), PlaylistCollabRules.menuEntries(false, false, false))
        assertEquals(listOf(PlaylistCollabRules.Entry.INVITE, PlaylistCollabRules.Entry.TURN_OFF), PlaylistCollabRules.menuEntries(false, false, true))
        assertTrue(PlaylistCollabRules.isInviteLink("https://music.example/#/collab/AbCdEfGh12"))
        assertFalse(PlaylistCollabRules.isInviteLink("https://music.example/s/AbCdEfGh12"))
    }

    @Test
    fun membersAndInvitesAreRead() {
        val m = PlaylistCollabRules.Members.from(obj("""{"name":"Road trip","collaborative":true,"role":"owner","owner":"anna",
            "members":[{"user_id":2,"username":"ben","status":"joined"},{"user_id":3,"username":"cara","status":"invited"}],
            "invite_link":{"token":"t","path":"/#/collab/t","url":"https://music.example/#/collab/t"}}"""))
        assertTrue(m.isOwner)
        assertEquals(listOf(PlaylistCollabRules.Member(2, "ben", false), PlaylistCollabRules.Member(3, "cara", true)), m.members)
        assertEquals("https://music.example/#/collab/t", m.link)
        val noBase = PlaylistCollabRules.Members.from(obj("""{"role":"owner","owner":"anna","members":[],"invite_link":{"token":"t","path":"/#/collab/t","url":"/#/collab/t"}}"""))
        assertEquals("/#/collab/t", noBase.link)
        val member = PlaylistCollabRules.Members.from(obj("""{"role":"collaborator","owner":"anna","members":[],"invite_link":null}"""))
        assertFalse(member.isOwner)
        assertEquals("", member.link)
        val i = PlaylistCollabRules.Invite.from(obj("""{"playlist_id":7,"name":"Road trip","owner":"anna","count":12}"""))
        assertEquals("anna invited you to collaborate on Road trip · 12 tracks", PlaylistCollabRules.inviteText(i))
    }
}
