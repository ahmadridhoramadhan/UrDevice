package com.urdevice.app

import android.content.Context
import com.urdevice.core.FfiEvent

/** String resources for capability strings and core reason codes. */
object Labels {
    fun cap(cap: String): Int = when (cap) {
        "clipboard.v1" -> R.string.cap_clipboard
        "files.v1" -> R.string.cap_files
        "gallery.v1" -> R.string.cap_gallery
        "input.v1" -> R.string.cap_input
        "media.v1" -> R.string.cap_media
        "screen.h264.v1" -> R.string.cap_screen
        "tunnel.v1" -> R.string.cap_tunnel
        "term.v1" -> R.string.cap_term
        else -> R.string.cap_fswrite
    }

    fun capDesc(cap: String): Int = when (cap) {
        "clipboard.v1" -> R.string.cap_clipboard_desc
        "files.v1" -> R.string.cap_files_desc
        "gallery.v1" -> R.string.cap_gallery_desc
        "input.v1" -> R.string.cap_input_desc
        "media.v1" -> R.string.cap_media_desc
        "screen.h264.v1" -> R.string.cap_screen_desc
        "tunnel.v1" -> R.string.cap_tunnel_desc
        "term.v1" -> R.string.cap_term_desc
        else -> R.string.cap_fswrite_desc
    }

    fun capName(ctx: Context, cap: String) = ctx.getString(cap(cap))

    fun pairingFailed(reason: String): Int = when (reason) {
        "cancelled" -> R.string.pair_fail_cancelled
        "timeout" -> R.string.pair_fail_timeout
        "expired" -> R.string.pair_fail_expired
        "locked" -> R.string.pair_fail_locked
        "closed" -> R.string.pair_fail_closed
        "hidden" -> R.string.pair_fail_hidden
        "unreachable" -> R.string.pair_fail_unreachable
        "invalid" -> R.string.pair_fail_invalid
        else -> R.string.pair_fail_other
    }

    fun decision(d: String): Int = when (d) {
        "allowed" -> R.string.decision_allowed
        "timeout" -> R.string.decision_timeout
        "declined" -> R.string.decision_declined
        "stopped" -> R.string.decision_stopped
        else -> R.string.decision_denied
    }

    /** One-line text for an event, or null when the event needs none. */
    fun event(ctx: Context, e: FfiEvent): String? = when (e) {
        is FfiEvent.PairingFinished ->
            if (e.ok) ctx.getString(R.string.paired_with, e.name) else ctx.getString(pairingFailed(e.reason))
        is FfiEvent.ActivityResult -> {
            val f = capName(ctx, e.capability)
            when {
                e.granted -> ctx.getString(R.string.result_allowed, e.deviceName, f)
                e.reason == "timeout" -> ctx.getString(R.string.result_timeout, e.deviceName)
                e.reason == "declined" -> ctx.getString(R.string.result_declined, e.deviceName, f)
                e.reason == "offline" -> ctx.getString(R.string.result_offline, e.deviceName)
                e.reason == "unsupported" -> ctx.getString(R.string.result_unsupported, e.deviceName, f)
                else -> ctx.getString(R.string.result_denied, e.deviceName, f)
            }
        }
        is FfiEvent.Notice -> when (e.code) {
            "unpaired_by" -> ctx.getString(R.string.notice_unpaired, e.deviceName)
            "disconnected_by" -> ctx.getString(R.string.notice_disconnected, e.deviceName)
            "update_required" -> ctx.getString(R.string.notice_update, e.deviceName)
            "pairing_locked" -> ctx.getString(R.string.notice_locked)
            "stopped_by" -> ctx.getString(R.string.notice_stopped, e.deviceName)
            else -> null
        }
        else -> null
    }
}
