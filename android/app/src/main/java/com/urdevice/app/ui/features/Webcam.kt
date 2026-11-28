package com.urdevice.app.ui.features

import android.content.Context
import com.urdevice.app.host.CameraHost
import com.urdevice.app.host.CameraStreams
import com.urdevice.core.UrDeviceNode

/**
 * Phone as webcam (#28, R17). This phone is the host: a desktop that you allowed streams one of its
 * cameras ([CameraHost]: Camera2 and MediaCodec) and can switch the camera, the flash, and the zoom.
 * The phone does not show other devices' cameras, so there is no tile. The persistent notification
 * with Stop is the service notification, which names the device and the feature while it streams.
 */
object WebcamFeature : Feature {
    override val id = "webcam"

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        val app = context.applicationContext
        CameraStreams.channel(app)
        runCatching { node.cameraSetHost(CameraHost(app)) }
    }
}
