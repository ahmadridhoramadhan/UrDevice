package com.urdevice.app.host

import android.content.ComponentName
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.VolumeProvider
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.net.Uri
import android.os.SystemClock
import android.util.Log
import com.urdevice.core.FfiMediaCmd
import com.urdevice.core.FfiMediaSession
import com.urdevice.core.FfiMediaState
import com.urdevice.core.FfiPlayState
import com.urdevice.core.FfiVolumeChange
import com.urdevice.core.MediaHostFfi
import java.io.ByteArrayOutputStream
import kotlin.math.roundToInt

/**
 * This phone's media sessions and volume, for other devices (R11.2, R11.4, design §9.4).
 *
 * Sessions come from `MediaSessionManager.getActiveSessions`, which needs the notification-listener
 * access of [MediaListenerService]. Without it the state says `listener_permission`. Volume and mute
 * act on the music stream. The core calls this from worker threads and only after the permission
 * engine allowed the controller.
 */
class MediaHost(private val context: Context) : MediaHostFfi {
    private val manager = context.getSystemService(MediaSessionManager::class.java)
    private val audio = context.getSystemService(AudioManager::class.java)
    private val listener = ComponentName(context, MediaListenerService::class.java)

    /** The last artwork per session, so a track is encoded once. */
    private val art = HashMap<String, Pair<String, ByteArray>>()

    /** Active sessions with their ids, or null when notification access is off. */
    private fun controllers(): List<Pair<String, MediaController>>? {
        val list = try {
            manager.getActiveSessions(listener)
        } catch (_: SecurityException) {
            return null
        }
        val seen = HashMap<String, Int>()
        return list.map { c ->
            val n = (seen[c.packageName] ?: 0) + 1
            seen[c.packageName] = n
            (if (n == 1) c.packageName else "${c.packageName}#$n") to c
        }
    }

    @Synchronized
    override fun state(): FfiMediaState {
        val max = audio.getStreamMaxVolume(AudioManager.STREAM_MUSIC)
        val cur = audio.getStreamVolume(AudioManager.STREAM_MUSIC)
        val list = controllers()
        val ids = list?.map { it.first }?.toSet() ?: emptySet()
        art.keys.retainAll(ids)
        return FfiMediaState(
            sessions = list?.map { (id, c) -> session(id, c) } ?: emptyList(),
            volume = if (max > 0) cur.toFloat() / max else 0f,
            muted = audio.isStreamMute(AudioManager.STREAM_MUSIC),
            canVolume = max > 0,
            unavailable = if (list == null) "listener_permission" else "",
        )
    }

    private fun session(id: String, c: MediaController): FfiMediaSession {
        val md = c.metadata
        val pb = c.playbackState
        val title = md?.getString(MediaMetadata.METADATA_KEY_TITLE)
            ?: md?.getString(MediaMetadata.METADATA_KEY_DISPLAY_TITLE) ?: ""
        val artist = md?.getString(MediaMetadata.METADATA_KEY_ARTIST)
            ?: md?.getString(MediaMetadata.METADATA_KEY_ALBUM_ARTIST)
            ?: md?.getString(MediaMetadata.METADATA_KEY_DISPLAY_SUBTITLE) ?: ""
        val duration = md?.getLong(MediaMetadata.METADATA_KEY_DURATION)?.takeIf { it > 0 }
        val playing = pb?.state == PlaybackState.STATE_PLAYING || pb?.state == PlaybackState.STATE_BUFFERING
        val state = when {
            playing -> FfiPlayState.PLAYING
            pb?.state == PlaybackState.STATE_PAUSED -> FfiPlayState.PAUSED
            else -> FfiPlayState.STOPPED
        }
        var position: Long? = pb?.position?.takeIf { it != PlaybackState.PLAYBACK_POSITION_UNKNOWN && it >= 0 }
        if (position != null && playing && pb != null) {
            // The position is as of lastPositionUpdateTime; a playing session has moved on since.
            val elapsed = SystemClock.elapsedRealtime() - pb.lastPositionUpdateTime
            position += (elapsed * pb.playbackSpeed).toLong().coerceAtLeast(0)
        }
        if (position != null && duration != null) position = position.coerceAtMost(duration)
        val actions = pb?.actions ?: 0L
        fun can(a: Long) = actions and a != 0L
        return FfiMediaSession(
            id = id,
            appName = appName(c.packageName),
            title = title,
            artist = artist,
            artworkPng = artwork(id, title, artist, md),
            state = state,
            positionMs = position?.toULong(),
            durationMs = duration?.toULong(),
            canSeek = can(PlaybackState.ACTION_SEEK_TO),
            canNext = can(PlaybackState.ACTION_SKIP_TO_NEXT),
            canPrev = can(PlaybackState.ACTION_SKIP_TO_PREVIOUS),
            canPlayPause = can(PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE),
        )
    }

    private fun appName(pkg: String): String = try {
        val pm = context.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
    } catch (_: Exception) {
        pkg.substringAfterLast('.').replaceFirstChar { it.uppercase() }
    }

    private fun artwork(id: String, title: String, artist: String, md: MediaMetadata?): ByteArray {
        if (md == null) return ByteArray(0)
        val bitmap = md.getBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART)
            ?: md.getBitmap(MediaMetadata.METADATA_KEY_ART)
            ?: md.getBitmap(MediaMetadata.METADATA_KEY_DISPLAY_ICON)
        val uri = md.getString(MediaMetadata.METADATA_KEY_ALBUM_ART_URI)
            ?: md.getString(MediaMetadata.METADATA_KEY_ART_URI)
            ?: md.getString(MediaMetadata.METADATA_KEY_DISPLAY_ICON_URI)
        val key = "$title\u0001$artist\u0001${bitmap?.generationId ?: 0}\u0001${uri ?: ""}"
        art[id]?.let { if (it.first == key) return it.second }
        val png = try {
            (bitmap ?: uri?.let { loadUri(it) })?.let { encode(it) } ?: ByteArray(0)
        } catch (e: Exception) {
            Log.w("UrDevice", "artwork failed: ${e.javaClass.simpleName}")
            ByteArray(0)
        }
        art[id] = key to png
        return png
    }

    /** Only local content is read: the phone never fetches a remote URL for a player. */
    private fun loadUri(s: String): Bitmap? {
        val uri = Uri.parse(s)
        if (uri.scheme != "content") return null
        return context.contentResolver.openInputStream(uri)?.use { BitmapFactory.decodeStream(it) }
    }

    /** A PNG of at most 256 px a side and 96 KiB. */
    private fun encode(src: Bitmap): ByteArray {
        var side = 256
        while (side >= 48) {
            val scale = minOf(1f, side.toFloat() / maxOf(src.width, src.height))
            val bmp = if (scale < 1f) {
                Bitmap.createScaledBitmap(
                    src,
                    (src.width * scale).roundToInt().coerceAtLeast(1),
                    (src.height * scale).roundToInt().coerceAtLeast(1),
                    true,
                )
            } else src
            val out = ByteArrayOutputStream()
            bmp.compress(Bitmap.CompressFormat.PNG, 100, out)
            if (bmp !== src) bmp.recycle()
            if (out.size() <= MAX_ARTWORK) return out.toByteArray()
            side = side * 3 / 4
        }
        return ByteArray(0)
    }

    @Synchronized
    override fun command(sessionId: String, cmd: FfiMediaCmd, seekMs: ULong): Boolean {
        val c = controllers()?.firstOrNull { it.first == sessionId }?.second ?: return false
        val tc = c.transportControls
        try {
            when (cmd) {
                FfiMediaCmd.PLAY_PAUSE ->
                    if (c.playbackState?.state == PlaybackState.STATE_PLAYING) tc.pause() else tc.play()
                FfiMediaCmd.NEXT -> tc.skipToNext()
                FfiMediaCmd.PREV -> tc.skipToPrevious()
                FfiMediaCmd.SEEK -> tc.seekTo(seekMs.toLong().coerceAtLeast(0))
            }
        } catch (e: Exception) {
            Log.w("UrDevice", "media command failed: ${e.javaClass.simpleName}")
            return false
        }
        return true
    }

    /** The music volume before a mute that had to be done by setting it to 0. */
    private var volumeBeforeMute = -1

    override fun volume(change: FfiVolumeChange): Boolean = try {
        val stream = AudioManager.STREAM_MUSIC
        when (change) {
            is FfiVolumeChange.Set -> {
                val max = audio.getStreamMaxVolume(stream)
                setMusicVolume((change.value.coerceIn(0f, 1f) * max).roundToInt())
            }
            is FfiVolumeChange.Step -> {
                // The protocol step is 2 % of the range; always move at least one system step.
                val max = audio.getStreamMaxVolume(stream)
                val now = audio.getStreamVolume(stream)
                var target = (now + change.steps * 0.02f * max).roundToInt()
                if (change.steps != 0 && target == now) target = now + Integer.signum(change.steps)
                setMusicVolume(target)
            }
            is FfiVolumeChange.Mute -> setMusicMute(change.muted)
        }
        true
    } catch (e: Exception) {
        // Not only SecurityException: a failed change must never take the core's worker down.
        Log.w("UrDevice", "volume change refused: ${e.javaClass.simpleName}")
        false
    }

    /**
     * Put the music volume at step [target]. Some phones (a vivo on Android 16 was seen) ignore
     * `setStreamVolume` and `adjustStreamVolume` and only follow what the volume keys do, so when
     * the volume did not move, step it the way the keys do (`adjustSuggestedStreamVolume`).
     */
    private fun setMusicVolume(target: Int) {
        val stream = AudioManager.STREAM_MUSIC
        val want = target.coerceIn(0, audio.getStreamMaxVolume(stream))
        audio.setStreamVolume(stream, want, 0)
        var guard = audio.getStreamMaxVolume(stream) + 1
        while (guard-- > 0) {
            val now = audio.getStreamVolume(stream)
            if (now == want) return
            val direction = if (want > now) AudioManager.ADJUST_RAISE else AudioManager.ADJUST_LOWER
            audio.adjustStreamVolume(stream, direction, 0)
            if (audio.getStreamVolume(stream) == now) {
                audio.adjustSuggestedStreamVolume(direction, stream, 0)
            }
            if (audio.getStreamVolume(stream) == now) {
                // Last resort: ask the playing app's media session. The system applies it itself.
                if (volumeViaSession(want, audio.getStreamMaxVolume(stream))) return
                Log.w("UrDevice", "the music volume does not move")
                return
            }
        }
    }

    /** Set the volume through the first media session that allows it (`MediaController.setVolumeTo`). */
    private fun volumeViaSession(want: Int, max: Int): Boolean {
        val list = controllers() ?: return false
        for ((_, c) in list) {
            val info = c.playbackInfo ?: continue
            if (info.volumeControl == VolumeProvider.VOLUME_CONTROL_FIXED || info.maxVolume <= 0) continue
            val index = if (max > 0) (want.toFloat() / max * info.maxVolume).roundToInt() else want
            c.setVolumeTo(index.coerceIn(0, info.maxVolume), 0)
            Log.i("UrDevice", "volume set through the media session")
            return true
        }
        return false
    }

    /** Mute or unmute through the first media session that allows it. */
    private fun muteViaSession(muted: Boolean): Boolean {
        val list = controllers() ?: return false
        for ((_, c) in list) {
            val info = c.playbackInfo ?: continue
            if (info.volumeControl == VolumeProvider.VOLUME_CONTROL_FIXED) continue
            c.adjustVolume(if (muted) AudioManager.ADJUST_MUTE else AudioManager.ADJUST_UNMUTE, 0)
            Log.i("UrDevice", "mute set through the media session")
            return true
        }
        return false
    }

    /**
     * Mute or unmute the music. If the system ignores that, mute by setting the volume to 0 and
     * unmute by going back to the volume from before.
     */
    private fun setMusicMute(muted: Boolean) {
        val stream = AudioManager.STREAM_MUSIC
        val adjust = if (muted) AudioManager.ADJUST_MUTE else AudioManager.ADJUST_UNMUTE
        audio.adjustStreamVolume(stream, adjust, 0)
        if (audio.isStreamMute(stream) == muted) return
        audio.adjustSuggestedStreamVolume(adjust, stream, 0)
        if (audio.isStreamMute(stream) == muted) return
        if (muteViaSession(muted)) return
        if (muted) {
            volumeBeforeMute = audio.getStreamVolume(stream)
            setMusicVolume(0)
        } else {
            val max = audio.getStreamMaxVolume(stream)
            setMusicVolume(if (volumeBeforeMute > 0) volumeBeforeMute else max / 2)
        }
    }

    private companion object {
        const val MAX_ARTWORK = 96 * 1024
    }
}
