package device.keystore

import android.app.Activity
import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@InvokeArg
class EntryArgs {
    lateinit var name: String
}

@InvokeArg
class WriteArgs {
    lateinit var name: String
    lateinit var value: String
}

/**
 * The app's secrets under one AES-256-GCM key held by the Android Keystore.
 *
 * The key never leaves the keystore; the preferences file holds only the
 * nonce and ciphertext of each entry, with the entry's name as associated
 * data so ciphertexts cannot be swapped between entries. A copy of the
 * file without the device's keystore (a backup restored elsewhere, a
 * filesystem copy) decrypts to nothing, and the app signs in again.
 */
@TauriPlugin
class KeystorePlugin(private val activity: Activity) : Plugin(activity) {
    private val preferences =
        activity.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)

    @Command
    fun get(invoke: Invoke) {
        val args = invoke.parseArgs(EntryArgs::class.java)
        try {
            val stored = preferences.getString(args.name, null)
            val result = JSObject()
            if (stored != null) {
                result.put("value", decrypt(args.name, stored))
            }
            invoke.resolve(result)
        } catch (error: Exception) {
            invoke.reject("the keystore entry could not be read: ${error.message}")
        }
    }

    @Command
    fun set(invoke: Invoke) {
        val args = invoke.parseArgs(WriteArgs::class.java)
        try {
            val sealed = encrypt(args.name, args.value)
            if (!preferences.edit().putString(args.name, sealed).commit()) {
                invoke.reject("the keystore entry could not be saved")
                return
            }
            invoke.resolve(JSObject())
        } catch (error: Exception) {
            invoke.reject("the keystore entry could not be saved: ${error.message}")
        }
    }

    @Command
    fun delete(invoke: Invoke) {
        val args = invoke.parseArgs(EntryArgs::class.java)
        if (!preferences.edit().remove(args.name).commit()) {
            invoke.reject("the keystore entry could not be removed")
            return
        }
        invoke.resolve(JSObject())
    }

    private fun encrypt(name: String, value: String): String {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        val sealed = cipher.doFinal(value.toByteArray(Charsets.UTF_8))
        return encode(cipher.iv) + ":" + encode(sealed)
    }

    private fun decrypt(name: String, stored: String): String {
        val parts = stored.split(":")
        require(parts.size == 2) { "the entry is malformed" }
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, decode(parts[0])))
        cipher.updateAAD(name.toByteArray(Charsets.UTF_8))
        return String(cipher.doFinal(decode(parts[1])), Charsets.UTF_8)
    }

    @Synchronized
    private fun key(): SecretKey {
        val keystore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (keystore.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    private fun encode(bytes: ByteArray): String = Base64.encodeToString(bytes, Base64.NO_WRAP)

    private fun decode(text: String): ByteArray = Base64.decode(text, Base64.NO_WRAP)

    private companion object {
        const val KEYSTORE = "AndroidKeyStore"
        const val ALIAS = "device-keystore"
        const val PREFERENCES = "device-keystore"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val TAG_BITS = 128
    }
}
