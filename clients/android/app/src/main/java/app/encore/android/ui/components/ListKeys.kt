package app.encore.android.ui.components

/**
 * Stable keys for lazy lists (`items(..., key = ...)`): with a key per item, Compose keeps each row's state and
 * layout when rows above it are added, removed or moved, instead of recomposing every row after the change, and a
 * removed or moved row can animate (`Modifier.animateItem()`).
 *
 * Plain functions with no Android or Compose types (unit-tested in ListKeysTest).
 */
object ListKeys {
    /**
     * A key for each of [items] from [keyOf], made unique by numbering repeats in list order: "a", "b", "a#2". For
     * lists whose items have no id of their own, or whose ids can repeat (a key must be unique, or the list throws).
     */
    fun <T> unique(items: List<T>, keyOf: (T) -> Any): List<String> {
        val seen = HashMap<String, Int>()
        return items.map { item ->
            val base = keyOf(item).toString()
            val n = (seen[base] ?: 0) + 1
            seen[base] = n
            if (n == 1) base else "$base#$n"
        }
    }

    /**
     * Keeps a per-entry key on each entry of a list that changes one edit at a time, such as the play queue, where the
     * same song can be queued twice ([ids] repeat) so the song's id can't be the key.
     *
     * Entries the old and new lists share at the start and at the end keep their keys (this covers a removal, an
     * insertion and songs added at the end exactly, duplicates included). In between, each new entry takes the key of
     * the first unused old entry with the same id (a moved song keeps its key), and anything else gets a new key from
     * [newKey].
     *
     * @param oldIds the ids of the list before the change.
     * @param oldKeys the keys of the list before the change (one per old id).
     * @param ids the ids of the list now.
     * @param newKey makes a key no entry has had before.
     * @return one key per entry of [ids], all different.
     */
    fun carry(oldIds: List<Long>, oldKeys: List<Long>, ids: List<Long>, newKey: () -> Long): List<Long> {
        require(oldIds.size == oldKeys.size) { "one key per old entry" }
        val max = minOf(oldIds.size, ids.size)
        var prefix = 0
        while (prefix < max && oldIds[prefix] == ids[prefix]) prefix++
        var suffix = 0
        while (suffix < max - prefix && oldIds[oldIds.size - 1 - suffix] == ids[ids.size - 1 - suffix]) suffix++
        // the old middle's unused keys by song id, in list order
        val free = HashMap<Long, ArrayDeque<Long>>()
        for (i in prefix until oldIds.size - suffix) free.getOrPut(oldIds[i]) { ArrayDeque() }.addLast(oldKeys[i])
        val keys = ArrayList<Long>(ids.size)
        for (i in ids.indices) {
            keys += when {
                i < prefix -> oldKeys[i]
                i >= ids.size - suffix -> oldKeys[oldKeys.size - (ids.size - i)]
                else -> free[ids[i]]?.removeFirstOrNull() ?: newKey()
            }
        }
        return keys
    }
}
