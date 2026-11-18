package com.urdevice.app

import android.app.Application

class UrDeviceApp : Application() {
    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)
    }
}
