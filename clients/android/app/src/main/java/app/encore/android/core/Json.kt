package app.encore.android.core

import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.KSerializer
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.descriptors.nullable
import kotlinx.serialization.builtins.serializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonEncoder
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

// The app's one lenient JSON layer. The server's rows come straight from SQLite (dynamic types: `year` may be text
// or a number, counts may be null, flags may be 0 / 1), other devices send their remote states, and older servers
// leave fields out, so everything read from the network is read leniently: by hand with the JsonObject readers
// below, or as `@Serializable` models whose fields use the `L…` types (a missing, null or odd value becomes the
// empty value instead of failing the whole answer).

/** The app's Json: unknown keys are ignored, and a null for a field with a default gives the default. */
val EncoreJson: Json = Json {
    ignoreUnknownKeys = true
    coerceInputValues = true
}

/** `text` parsed as JSON; null when it is not JSON. */
fun parseJson(text: String?): JsonElement? {
    if (text.isNullOrEmpty()) return null
    return try { EncoreJson.parseToJsonElement(text) } catch (_: Exception) { null }
}

/** `text` parsed as a JSON object; null when it is something else. */
fun parseJsonObject(text: String?): JsonObject? = parseJson(text) as? JsonObject

// ---- reading a document by hand -----------------------------------------------------------------------------

/** A string or a number as written; "" for a missing key or null; an object or array as its JSON text. */
fun JsonElement?.textOf(): String = when (this) {
    null, is JsonNull -> ""
    is JsonPrimitive -> content
    else -> toString()
}

/** [textOf] of `key`. */
fun JsonObject.str(key: String): String = this[key].textOf()

/** Only a JSON string ("" for numbers, booleans, null, objects and arrays). */
fun JsonObject.stringOnly(key: String): String = (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.content ?: ""

/** A number, also when sent as text; 0 for anything else. */
fun JsonElement?.numberOf(): Double {
    val v = this as? JsonPrimitive ?: return 0.0
    if (v is JsonNull) return 0.0
    return v.content.toDoubleOrNull() ?: 0.0
}

fun JsonObject.dbl(key: String): Double = this[key].numberOf()

fun JsonObject.long(key: String): Long = dbl(key).toLong()

fun JsonObject.int(key: String): Int = dbl(key).toInt()

/** `true`, or a number other than 0 (SQLite's 1); false for anything else. */
fun JsonElement?.flagOf(): Boolean {
    val v = this as? JsonPrimitive ?: return false
    if (v is JsonNull) return false
    val c = v.content
    return c == "true" || (c.toDoubleOrNull() ?: 0.0) != 0.0
}

fun JsonObject.bool(key: String): Boolean = this[key].flagOf()

fun JsonObject.obj(key: String): JsonObject? = this[key] as? JsonObject

fun JsonObject.arr(key: String): JsonArray? = this[key] as? JsonArray

/** A list response as its objects: a bare array, or `{items: [...]}` (albums); anything else is empty. */
fun JsonElement?.objects(): List<JsonObject> {
    val list: JsonArray = when (this) {
        is JsonArray -> this
        is JsonObject -> this.arr("items") ?: return emptyList()
        else -> return emptyList()
    }
    return list.objectsOf()
}

/** The objects of an array (possibly null); other elements are skipped. */
fun JsonArray?.objectsOf(): List<JsonObject> {
    if (this == null) return emptyList()
    val out = ArrayList<JsonObject>(size)
    for (e in this) if (e is JsonObject) out.add(e)
    return out
}

/** The strings and numbers of an array (possibly null), as text; null, objects and arrays are skipped. */
fun JsonArray?.strings(): List<String> {
    if (this == null) return emptyList()
    val out = ArrayList<String>(size)
    for (e in this) if (e is JsonPrimitive && e !is JsonNull) out.add(e.content)
    return out
}

// ---- @Serializable models -----------------------------------------------------------------------------------

/**
 * A model that can come out of decoding unusable (a row without its title) or in need of a last touch (a field
 * that falls back on another): [checked] gives what to keep, or null to leave it out. [decodeModel] and the [Items]
 * list serializer apply it.
 */
interface Checked<out T> {
    fun checked(): T?
}

@Suppress("UNCHECKED_CAST")
private fun <T> keep(v: T): T? = if (v is Checked<*>) v.checked() as T? else v

/** The object as a model ([Checked] applied); null when it doesn't decode or isn't usable. */
fun <T> JsonObject.decodeModel(serializer: KSerializer<T>): T? =
    try { keep(EncoreJson.decodeFromJsonElement(serializer, this)) } catch (_: Exception) { null }

/** The element as a model; null for anything but an object (or one that doesn't decode). */
fun <T> JsonElement?.decodeModelOrNull(serializer: KSerializer<T>): T? = (this as? JsonObject)?.decodeModel(serializer)

private fun Decoder.element(): JsonElement =
    (this as? JsonDecoder)?.decodeJsonElement() ?: JsonPrimitive(decodeString())

/** [textOf]: strings and numbers as written, "" for null. */
object LenientString : KSerializer<String> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("encore.LenientString", PrimitiveKind.STRING)
    override fun deserialize(decoder: Decoder): String = decoder.element().textOf()
    override fun serialize(encoder: Encoder, value: String) = encoder.encodeString(value)
}

/** The first four characters (a year sent as "2019-05-01", 2019 or "2019"). */
object LenientYear : KSerializer<String> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("encore.LenientYear", PrimitiveKind.STRING)
    override fun deserialize(decoder: Decoder): String = decoder.element().textOf().take(4)
    override fun serialize(encoder: Encoder, value: String) = encoder.encodeString(value)
}

/** [numberOf] as an Int (a number sent as text, a fraction cut off, 0 for null). */
object LenientInt : KSerializer<Int> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("encore.LenientInt", PrimitiveKind.INT)
    override fun deserialize(decoder: Decoder): Int = decoder.element().numberOf().toInt()
    override fun serialize(encoder: Encoder, value: Int) = encoder.encodeInt(value)
}

object LenientLong : KSerializer<Long> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("encore.LenientLong", PrimitiveKind.LONG)
    override fun deserialize(decoder: Decoder): Long = decoder.element().numberOf().toLong()
    override fun serialize(encoder: Encoder, value: Long) = encoder.encodeLong(value)
}

object LenientDouble : KSerializer<Double> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("encore.LenientDouble", PrimitiveKind.DOUBLE)
    override fun deserialize(decoder: Decoder): Double = decoder.element().numberOf()
    override fun serialize(encoder: Encoder, value: Double) = encoder.encodeDouble(value)
}

/** [flagOf]: true, or a number other than 0. */
object LenientBool : KSerializer<Boolean> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("encore.LenientBool", PrimitiveKind.BOOLEAN)
    override fun deserialize(decoder: Decoder): Boolean = decoder.element().flagOf()
    override fun serialize(encoder: Encoder, value: Boolean) = encoder.encodeBoolean(value)
}

/** A flag that may be missing: null when the key is absent or null, else [flagOf]. */
@OptIn(ExperimentalSerializationApi::class)
object LenientBoolOrNull : KSerializer<Boolean?> {
    override val descriptor: SerialDescriptor = PrimitiveSerialDescriptor("encore.LenientBoolOrNull", PrimitiveKind.BOOLEAN).nullable
    override fun deserialize(decoder: Decoder): Boolean? = decoder.element().let { if (it is JsonNull) null else it.flagOf() }
    override fun serialize(encoder: Encoder, value: Boolean?) {
        if (value == null) encoder.encodeNull() else encoder.encodeBoolean(value)
    }
}

/** An object, or null for anything else. */
@OptIn(ExperimentalSerializationApi::class)
object ObjectOrNull : KSerializer<JsonObject?> {
    override val descriptor: SerialDescriptor = JsonElement.serializer().descriptor.nullable
    override fun deserialize(decoder: Decoder): JsonObject? = decoder.element() as? JsonObject
    override fun serialize(encoder: Encoder, value: JsonObject?) {
        val e = encoder as? JsonEncoder ?: throw IllegalStateException("JSON only")
        e.encodeJsonElement(value ?: JsonNull)
    }
}

/**
 * A nested model: decoded from an object ([Checked] applied), null for anything else or when it doesn't decode (a
 * broken part never costs the whole answer). Subclass it as an object per model: `object XOrNull : Nested<X>(X.serializer())`.
 */
@OptIn(ExperimentalSerializationApi::class)
open class Nested<T>(private val element: KSerializer<T>) : KSerializer<T?> {
    override val descriptor: SerialDescriptor = element.descriptor.nullable
    override fun deserialize(decoder: Decoder): T? = decoder.element().decodeModelOrNull(element)
    override fun serialize(encoder: Encoder, value: T?) {
        if (value == null) encoder.encodeNull() else element.serialize(encoder, value)
    }
}

/**
 * A list of models: the objects of an array, each decoded on its own; one that fails or that [Checked] leaves out
 * is skipped (a bad row never costs the whole answer). Anything but an array is an empty list.
 */
class Items<T>(private val element: KSerializer<T>) : KSerializer<List<T>> {
    private val list = ListSerializer(element)
    override val descriptor: SerialDescriptor = list.descriptor

    override fun deserialize(decoder: Decoder): List<T> {
        val arr = decoder.element() as? JsonArray ?: return emptyList()
        val out = ArrayList<T>(arr.size)
        for (e in arr) {
            val o = e as? JsonObject ?: continue
            o.decodeModel(element)?.let { out.add(it) }
        }
        return out
    }

    override fun serialize(encoder: Encoder, value: List<T>) = list.serialize(encoder, value)
}

/** The strings and numbers of an array as text ([strings]); anything but an array is an empty list. */
object TextList : KSerializer<List<String>> {
    private val list = ListSerializer(String.serializer())
    override val descriptor: SerialDescriptor = list.descriptor
    override fun deserialize(decoder: Decoder): List<String> = (decoder.element() as? JsonArray).strings()
    override fun serialize(encoder: Encoder, value: List<String>) = list.serialize(encoder, value)
}

/** Field types for lenient `@Serializable` models (see the serializers above). */
typealias LString = @Serializable(with = LenientString::class) String
typealias LYear = @Serializable(with = LenientYear::class) String
typealias LInt = @Serializable(with = LenientInt::class) Int
typealias LLong = @Serializable(with = LenientLong::class) Long
typealias LDouble = @Serializable(with = LenientDouble::class) Double
typealias LBool = @Serializable(with = LenientBool::class) Boolean
typealias LBoolOrNull = @Serializable(with = LenientBoolOrNull::class) Boolean?
typealias LObject = @Serializable(with = ObjectOrNull::class) JsonObject?
typealias LTexts = @Serializable(with = TextList::class) List<String>
